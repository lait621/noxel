//! The crowd: level-of-detail tiers and the agent container.
//!
//! A town needs hundreds of people and a frame has four milliseconds, so the
//! crowd is simulated in **tiers**. An agent's tier is a pure function of its
//! distance from the crowd's centre (usually the camera focus):
//!
//! | Tier | Radius | Decision interval | Steering | Collides |
//! |---|---|---|---|---|
//! | 0 — [`CrowdTier::Near`] | 25 m | every step | full | yes |
//! | 1 — [`CrowdTier::Mid`] | 60 m | 0.25 s | full | no |
//! | 2 — [`CrowdTier::Far`] | 90 m | 0.75 s | path only | no |
//! | 3 — [`CrowdTier::Frozen`] | beyond | 2 s | path only | no |
//!
//! Only tier 0 owns a physics capsule, so only the handful of agents the player
//! can actually touch pay for `move_character`; everyone else walks the same
//! path on a coarse clock. That is what keeps a thousand agents inside the
//! budget without making the near crowd look cheap.

use noxel_core::math::Vec3;
use noxel_core::rng::RngStream;

use crate::NpcConfig;
use crate::agent::{NpcAgent, NpcId, NpcKind, NpcStats};

/// The simulation level of detail an agent is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CrowdTier {
    /// Fully simulated, physics-backed, decided every step.
    Near = 0,
    /// Fully steered, but decided on a quarter-second clock and not collided.
    Mid = 1,
    /// Path following only, three-quarter-second clock.
    Far = 2,
    /// Outside every radius: barely updated, and normally despawned.
    Frozen = 3,
}

impl CrowdTier {
    /// Every tier, from nearest to furthest.
    pub const ALL: [Self; 4] = [Self::Near, Self::Mid, Self::Far, Self::Frozen];

    /// The tier's index, for [`TierConfig`] array lookups and statistics.
    #[inline]
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The tier with this index, clamped into range.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Near,
            1 => Self::Mid,
            2 => Self::Far,
            _ => Self::Frozen,
        }
    }

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Near => "near",
            Self::Mid => "mid",
            Self::Far => "far",
            Self::Frozen => "frozen",
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for CrowdTier {
    /// Tier 0: the assumption is that an agent starts near the camera, and the
    /// first tier pass corrects it.
    fn default() -> Self {
        Self::Near
    }
}

/// The radii, clocks and capabilities of the four crowd tiers.
///
/// The defaults are the town-sized crowd the engine targets: a 25 m fully
/// simulated bubble, a 60 m steered ring, a 90 m coarse ring and nothing
/// beyond. [`TierConfig::sanitised`] is applied by the crowd manager, so a
/// hand-edited config with `near > mid` degrades to something sensible instead
/// of producing agents that never move.
#[derive(Clone, Copy, Debug)]
pub struct TierConfig {
    /// Tier 0 radius in metres.
    pub near: f32,
    /// Tier 1 radius.
    pub mid: f32,
    /// Tier 2 radius; beyond it an agent is frozen or despawned.
    pub far: f32,
    /// Seconds between decisions for each tier, indexed by tier.
    pub interval: [f32; 4],
    /// Whether each tier runs full steering (false = path following only).
    pub full_steering: [bool; 4],
    /// Whether each tier collides with the world.
    pub collides: [bool; 4],
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            near: 25.0,
            mid: 60.0,
            far: 90.0,
            interval: [0.0, 0.25, 0.75, 2.0],
            full_steering: [true, true, false, false],
            collides: [true, false, false, false],
        }
    }
}

impl TierConfig {
    /// The same configuration with the radii ordered, finite and positive, and
    /// the clocks finite and non-negative.
    ///
    /// A caller that only ever edits one field of a default config cannot get
    /// this wrong; the sanitiser exists for configs that arrive from a file.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let near = finite_radius(self.near, 25.0);
        let mid = finite_radius(self.mid, near.max(60.0)).max(near);
        let far = finite_radius(self.far, mid.max(90.0)).max(mid);
        self.near = near;
        self.mid = mid;
        self.far = far;
        for i in 0..4 {
            let value = self.interval[i];
            self.interval[i] = if value.is_finite() && value > 0.0 {
                value
            } else {
                0.0
            };
        }
        self
    }

    /// The tier a distance from the crowd centre implies.
    #[must_use]
    pub fn tier_for(&self, distance: f32) -> CrowdTier {
        let d = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };
        if d < self.near {
            CrowdTier::Near
        } else if d < self.mid {
            CrowdTier::Mid
        } else if d < self.far {
            CrowdTier::Far
        } else {
            CrowdTier::Frozen
        }
    }

    /// Seconds between decisions for `tier`.
    #[inline]
    #[must_use]
    pub fn interval_of(&self, tier: CrowdTier) -> f32 {
        self.interval[tier.index()]
    }

    /// True when `tier` runs the full steering stack.
    #[inline]
    #[must_use]
    pub fn uses_full_steering(&self, tier: CrowdTier) -> bool {
        self.full_steering[tier.index()]
    }

    /// True when `tier` is backed by a physics capsule.
    #[inline]
    #[must_use]
    pub fn collides_in(&self, tier: CrowdTier) -> bool {
        self.collides[tier.index()]
    }
}

/// Makes a radius finite and at least a metre, falling back to `fallback`.
fn finite_radius(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 1.0 {
        value
    } else {
        fallback
    }
}

/// The agent container: a slot array indexed by [`NpcId`].
///
/// The manager owns *data*, not behaviour. It knows how to hand out ids, how to
/// look an agent up in constant time and how to report a snapshot; where agents
/// want to go and how they get there is [`crate::NpcSystem`]'s job. Keeping the
/// split means a game can drive a crowd itself (a cutscene, a test) without
/// pulling in pathfinding.
///
/// Iteration is always in **slot order**, never over a hash map, so two runs of
/// the same simulation visit agents in the same sequence.
pub struct CrowdManager {
    config: NpcConfig,
    slots: Vec<Option<NpcAgent>>,
    live: usize,
    next_id: u32,
    spawned_this_step: usize,
    despawned_this_step: usize,
}

impl CrowdManager {
    /// An empty crowd governed by `config`.
    #[must_use]
    pub fn new(config: NpcConfig) -> Self {
        let mut config = config;
        config.tier = config.tier.sanitised();
        Self {
            slots: Vec::new(),
            live: 0,
            next_id: 0,
            spawned_this_step: 0,
            despawned_this_step: 0,
            config,
        }
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> &NpcConfig {
        &self.config
    }

    /// Replaces the configuration. Existing agents are kept.
    pub(crate) fn set_config(&mut self, config: NpcConfig) {
        let mut config = config;
        config.tier = config.tier.sanitised();
        self.config = config;
    }

    /// How many agents are resident.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// True when no agent is resident.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// The length of the slot array, live agents or not.
    ///
    /// [`CrowdManager`] never recycles ids, so this is one more than the highest
    /// id ever issued. A game that streams a town for hours should call
    /// [`CrowdManager::clear`] when it changes map.
    #[inline]
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// A spawn position's worth of new agent.
    ///
    /// Returns [`NpcId::INVALID`] when the crowd already holds
    /// [`NpcConfig::max_agents`] agents, when `max_agents` is zero, or when
    /// `position` is not finite. The agent's facing is drawn from the crowd
    /// seed, so the same call in the same run always produces the same yaw.
    pub fn spawn(&mut self, kind: NpcKind, position: Vec3) -> NpcId {
        if self.config.max_agents == 0 || self.live >= self.config.max_agents {
            return NpcId::INVALID;
        }
        if !position.is_finite() || self.next_id == u32::MAX {
            return NpcId::INVALID;
        }
        let id = NpcId(self.next_id);
        let index = id.index();
        if index >= self.slots.len() {
            self.slots.resize_with(index + 1, || None);
        }
        let mut agent = NpcAgent::new(id, kind, position);
        agent.yaw = RngStream::indexed(self.config.seed, "npc/yaw", u64::from(id.0))
            .rng()
            .range_f32(-core::f32::consts::PI, core::f32::consts::PI);
        self.slots[index] = Some(agent);
        self.live += 1;
        self.spawned_this_step += 1;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Retires an agent, returning `true` when it existed.
    ///
    /// The id stays retired forever: a later [`CrowdManager::spawn`] gets a
    /// fresh id even when it reuses the slot's index space.
    pub fn despawn(&mut self, id: NpcId) -> bool {
        if !id.is_valid() || id.index() >= self.slots.len() {
            return false;
        }
        if self.slots[id.index()].take().is_some() {
            self.live = self.live.saturating_sub(1);
            self.despawned_this_step += 1;
            true
        } else {
            false
        }
    }

    /// The agent with this id, if it is resident.
    #[inline]
    #[must_use]
    pub fn agent(&self, id: NpcId) -> Option<&NpcAgent> {
        if !id.is_valid() {
            return None;
        }
        self.slots.get(id.index()).and_then(Option::as_ref)
    }

    /// Mutable access to the agent with this id, if it is resident.
    #[inline]
    #[must_use]
    pub fn agent_mut(&mut self, id: NpcId) -> Option<&mut NpcAgent> {
        if !id.is_valid() {
            return None;
        }
        self.slots.get_mut(id.index()).and_then(Option::as_mut)
    }

    /// The agent in slot `index`, if it is resident.
    ///
    /// This is the constant-time lookup the crowd hot loops use; it is the same
    /// as `agent(NpcId(index as u32))` without the id validity check.
    #[inline]
    #[must_use]
    pub(crate) fn agent_at(&self, index: usize) -> Option<&NpcAgent> {
        self.slots.get(index).and_then(Option::as_ref)
    }

    /// The raw slot array, for the crowd system's tight loops.
    #[inline]
    #[must_use]
    pub(crate) fn slots(&self) -> &[Option<NpcAgent>] {
        &self.slots
    }

    /// Mutable access to the raw slot array.
    #[inline]
    #[must_use]
    pub(crate) fn slots_mut(&mut self) -> &mut [Option<NpcAgent>] {
        &mut self.slots
    }

    /// Every resident agent, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &NpcAgent> {
        self.slots.iter().filter_map(Option::as_ref)
    }

    /// Every resident agent mutably, in slot order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut NpcAgent> {
        self.slots.iter_mut().filter_map(Option::as_mut)
    }

    /// Every resident id, in slot order.
    pub fn ids(&self) -> impl Iterator<Item = NpcId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| NpcId(i as u32)))
    }

    /// Removes every agent and forgets every id ever handed out.
    ///
    /// The next spawn starts again at [`NpcId`] `0`, so only call this when the
    /// world itself has been replaced (a new map, a new game).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.live = 0;
        self.next_id = 0;
        self.spawned_this_step = 0;
        self.despawned_this_step = 0;
    }

    /// A snapshot of the crowd: counts, tiers and the blocked agents.
    ///
    /// The pathfinding counters are zero here — they live in
    /// [`crate::Pathfinder`] — and [`crate::NpcSystem::stats`] merges the two.
    #[must_use]
    pub fn stats(&self) -> NpcStats {
        let mut stats = NpcStats {
            active: self.live,
            spawned_this_step: self.spawned_this_step,
            despawned_this_step: self.despawned_this_step,
            ..NpcStats::default()
        };
        for agent in self.iter() {
            stats.tier_counts[agent.tier.index()] += 1;
            if agent.blocked {
                stats.blocked += 1;
            }
        }
        stats
    }

    /// Approximate heap footprint of the crowd, in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.slots.capacity() * core::mem::size_of::<Option<NpcAgent>>()
            + core::mem::size_of::<Self>()
    }

    /// Clears the per-step counters. The crowd system calls this at the top of
    /// every update.
    pub(crate) fn begin_step(&mut self) {
        self.spawned_this_step = 0;
        self.despawned_this_step = 0;
    }
}

impl core::fmt::Debug for CrowdManager {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrowdManager")
            .field("live", &self.live)
            .field("slots", &self.slots.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> CrowdManager {
        CrowdManager::new(NpcConfig {
            seed: 11,
            max_agents: 8,
            ..NpcConfig::default()
        })
    }

    #[test]
    fn tiers_follow_the_radii() {
        let tier = TierConfig::default();
        assert_eq!(tier.tier_for(0.0), CrowdTier::Near);
        assert_eq!(tier.tier_for(24.9), CrowdTier::Near);
        assert_eq!(tier.tier_for(25.0), CrowdTier::Mid);
        assert_eq!(tier.tier_for(59.9), CrowdTier::Mid);
        assert_eq!(tier.tier_for(60.0), CrowdTier::Far);
        assert_eq!(tier.tier_for(89.9), CrowdTier::Far);
        assert_eq!(tier.tier_for(90.0), CrowdTier::Frozen);
        assert_eq!(tier.tier_for(f32::NAN), CrowdTier::Near);
        assert_eq!(tier.tier_for(-5.0), CrowdTier::Near);
        for (i, t) in CrowdTier::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(CrowdTier::from_index(i), *t);
            assert!(!t.name().is_empty());
        }
        assert_eq!(CrowdTier::from_index(99), CrowdTier::Frozen);
    }

    #[test]
    fn tier_capabilities_match_the_contract() {
        let tier = TierConfig::default();
        assert_eq!(tier.interval, [0.0, 0.25, 0.75, 2.0]);
        assert_eq!(tier.full_steering, [true, true, false, false]);
        assert_eq!(tier.collides, [true, false, false, false]);
        assert!(tier.collides_in(CrowdTier::Near));
        assert!(!tier.collides_in(CrowdTier::Far));
        assert_eq!(tier.interval_of(CrowdTier::Frozen), 2.0);
    }

    #[test]
    fn sanitise_orders_broken_radii() {
        let broken = TierConfig {
            near: f32::NAN,
            mid: -3.0,
            far: 0.0,
            interval: [f32::NAN, -1.0, 0.5, 2.0],
            ..TierConfig::default()
        }
        .sanitised();
        assert!(broken.near.is_finite() && broken.near > 0.0);
        assert!(broken.mid >= broken.near);
        assert!(broken.far >= broken.mid);
        assert_eq!(broken.interval[0], 0.0);
        assert_eq!(broken.interval[1], 0.0);
        assert_eq!(broken.interval[2], 0.5);
    }

    #[test]
    fn spawn_hands_out_monotonic_ids_and_reports_the_slots() {
        let mut crowd = manager();
        let a = crowd.spawn(NpcKind::Villager, Vec3::ZERO);
        let b = crowd.spawn(NpcKind::Guard, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(a, NpcId(0));
        assert_eq!(b, NpcId(1));
        assert_eq!(crowd.len(), 2);
        assert_eq!(crowd.slot_count(), 2);
        assert!(!crowd.is_empty());
        assert_eq!(crowd.ids().collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(crowd.iter().count(), 2);
        assert_eq!(crowd.agent(a).map(|x| x.kind), Some(NpcKind::Villager));
        assert!(crowd.agent_mut(b).is_some());
    }

    #[test]
    fn spawn_refuses_when_full_or_when_the_position_is_not_finite() {
        let mut crowd = manager();
        for i in 0..8 {
            assert!(
                crowd
                    .spawn(NpcKind::Villager, Vec3::splat(i as f32))
                    .is_valid()
            );
        }
        assert_eq!(crowd.spawn(NpcKind::Villager, Vec3::ZERO), NpcId::INVALID);
        assert_eq!(crowd.len(), 8);
        let mut zero = manager();
        zero.set_config(NpcConfig {
            max_agents: 0,
            ..NpcConfig::default()
        });
        assert_eq!(zero.spawn(NpcKind::Villager, Vec3::ZERO), NpcId::INVALID);
        let mut nan = manager();
        assert_eq!(
            nan.spawn(NpcKind::Villager, Vec3::new(f32::NAN, 0.0, 0.0)),
            NpcId::INVALID
        );
    }

    #[test]
    fn despawn_retires_the_id_forever() {
        let mut crowd = manager();
        let a = crowd.spawn(NpcKind::Merchant, Vec3::ZERO);
        assert!(crowd.despawn(a));
        assert!(!crowd.despawn(a), "double despawn is a no-op");
        assert!(crowd.agent(a).is_none());
        assert!(crowd.agent(NpcId::INVALID).is_none());
        assert!(crowd.agent(NpcId(9_999)).is_none());
        let b = crowd.spawn(NpcKind::Merchant, Vec3::ZERO);
        assert_ne!(a, b, "ids are never recycled");
        assert_eq!(crowd.len(), 1);
        assert_eq!(crowd.slot_count(), 2);
    }

    #[test]
    fn ids_iterate_in_slot_order_after_a_hole_appears() {
        let mut crowd = manager();
        let a = crowd.spawn(NpcKind::Villager, Vec3::ZERO);
        let b = crowd.spawn(NpcKind::Villager, Vec3::X);
        let c = crowd.spawn(NpcKind::Villager, Vec3::Z);
        crowd.despawn(b);
        assert_eq!(crowd.ids().collect::<Vec<_>>(), vec![a, c]);
        assert_eq!(crowd.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, c]);
    }

    #[test]
    fn clear_resets_the_id_counter() {
        let mut crowd = manager();
        crowd.spawn(NpcKind::Animal, Vec3::ZERO);
        crowd.spawn(NpcKind::Animal, Vec3::X);
        crowd.clear();
        assert!(crowd.is_empty());
        assert_eq!(crowd.slot_count(), 0);
        assert_eq!(crowd.spawn(NpcKind::Animal, Vec3::ZERO), NpcId(0));
    }

    #[test]
    fn stats_count_tiers_and_the_step_window() {
        let mut crowd = manager();
        let a = crowd.spawn(NpcKind::Villager, Vec3::ZERO);
        let b = crowd.spawn(NpcKind::Villager, Vec3::X);
        if let Some(agent) = crowd.agent_mut(a) {
            agent.tier = CrowdTier::Far;
            agent.blocked = true;
        }
        let stats = crowd.stats();
        assert_eq!(stats.active, 2);
        assert_eq!(stats.spawned_this_step, 2);
        assert_eq!(stats.tier_counts, [1, 0, 1, 0]);
        assert_eq!(stats.blocked, 1);
        crowd.begin_step();
        assert_eq!(crowd.stats().spawned_this_step, 0);
        crowd.despawn(b);
        assert_eq!(crowd.stats().despawned_this_step, 1);
        assert_eq!(crowd.stats().active, 1);
    }

    #[test]
    fn memory_is_reported_and_finite() {
        let mut crowd = manager();
        for i in 0..8 {
            crowd.spawn(NpcKind::Villager, Vec3::splat(i as f32));
        }
        assert!(crowd.memory_bytes() > 0);
        assert!(format!("{crowd:?}").contains("CrowdManager"));
    }
}
