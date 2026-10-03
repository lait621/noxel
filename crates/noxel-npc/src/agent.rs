//! One NPC: identity, kind, behaviour state and the statistics a frame reports.
//!
//! [`NpcAgent`] is deliberately a **plain data** record. Every field is public
//! because the renderer, the debug overlay and gameplay code all read it, and
//! because the crowd system writes it in place without any accessor overhead:
//! a thousand agents must cost a thousand cheap struct updates per frame.
//!
//! The record is `Send + Sync + 'static`, so it is also a valid
//! [`noxel_ecs::Component`] — a game that wants the ECS to own its NPCs can put
//! one in a [`noxel_ecs::World`] and query it with the usual `for_each` family
//! without any adapter:
//!
//! ```
//! use noxel_core::math::Vec3;
//! use noxel_ecs::World;
//! use noxel_npc::{NpcAgent, NpcId, NpcKind, NpcState};
//!
//! let mut world = World::new();
//! let entity = world.spawn_named("innkeeper");
//! world.insert(entity, NpcAgent::new(NpcId(0), NpcKind::Merchant, Vec3::ZERO));
//! let mut seen = 0;
//! world.for_each::<NpcAgent, _>(|_, agent| {
//!     assert_eq!(agent.state, NpcState::Idle);
//!     seen += 1;
//! });
//! assert_eq!(seen, 1);
//! ```

use noxel_core::math::{Quat, Transform, Vec3};

use crate::crowd::CrowdTier;

/// Below this speed (metres per second) an agent counts as standing still.
///
/// It is comfortably under one centimetre per frame at 60 Hz, so floating-point
/// noise in the steering integrator can never make an idle agent look busy.
pub const MOVING_SPEED: f32 = 0.05;

/// A stable identifier for one agent.
///
/// Ids are **monotonic and never recycled**: despawning an agent retires its id
/// for the lifetime of the [`crate::CrowdManager`], so a stale id can never
/// resolve to a different agent later. `NpcId(3)` therefore always means the
/// fourth agent ever spawned by that manager, which is what makes save games,
/// dialogue targets and the debug overlay safe to hold across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NpcId(pub u32);

impl NpcId {
    /// The id that never refers to an agent.
    ///
    /// Returned by [`crate::CrowdManager::spawn`] when the crowd is full, and
    /// accepted by every method that takes an [`NpcId`] as "no agent".
    pub const INVALID: Self = Self(u32::MAX);

    /// The slot index behind this id.
    ///
    /// Used by the crowd system to index its parallel arrays; `INVALID` maps to
    /// a very large index that no array ever reaches.
    #[inline]
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// True when this id can refer to an agent.
    #[inline]
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

impl Default for NpcId {
    /// [`NpcId::INVALID`].
    fn default() -> Self {
        Self::INVALID
    }
}

/// What an agent is.
///
/// The kind picks the walking speed, the health pool and the shape of the daily
/// schedule. It is fixed at spawn time; a villager does not become a guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NpcKind {
    /// An ordinary resident: the baseline 1.0 speed scale.
    Villager,
    /// A town guard: slightly faster, tougher, and outdoors at dawn and dusk.
    Guard,
    /// A shopkeeper or trader: slower (they carry stock) and tied to a stall.
    Merchant,
    /// A child: quicker than an adult over short distances, but fragile.
    Child,
    /// Livestock, a dog or a wild animal: slow, low health, no fixed home.
    Animal,
}

impl NpcKind {
    /// Every kind, in a fixed order.
    pub const ALL: [Self; 5] = [
        Self::Villager,
        Self::Guard,
        Self::Merchant,
        Self::Child,
        Self::Animal,
    ];

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Villager => "villager",
            Self::Guard => "guard",
            Self::Merchant => "merchant",
            Self::Child => "child",
            Self::Animal => "animal",
        }
    }

    /// The multiplier applied to the configured walk and run speeds.
    ///
    /// A child hurries, a guard keeps pace, a loaded merchant dawdles and an
    /// animal ambles; the values are the contract's and are asserted by a test.
    #[must_use]
    pub fn speed_scale(self) -> f32 {
        match self {
            Self::Child => 1.1,
            Self::Guard => 1.05,
            Self::Merchant => 0.9,
            Self::Animal => 0.7,
            Self::Villager => 1.0,
        }
    }

    /// The kind's starting (and maximum) health.
    #[must_use]
    pub fn max_health(self) -> f32 {
        match self {
            Self::Guard => 140.0,
            Self::Child => 60.0,
            Self::Animal => 40.0,
            Self::Merchant | Self::Villager => 100.0,
        }
    }

    /// The index of this kind in [`NpcKind::ALL`], for array lookups.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Villager => 0,
            Self::Guard => 1,
            Self::Merchant => 2,
            Self::Child => 3,
            Self::Animal => 4,
        }
    }
}

/// What an agent is doing right now.
///
/// The state is a *presentation* of the agent's situation, not its driver: the
/// scheduler decides where the agent wants to be, the pathfinder gets it there
/// and the state is derived from what actually happened. That ordering is what
/// keeps an agent walking into a wall from reporting `Working`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NpcState {
    /// Standing still with nowhere to be.
    Idle,
    /// Following a path.
    Walking,
    /// At a workplace, doing the job.
    Working,
    /// Standing near another agent, talking.
    Talking,
    /// Asleep at home.
    Sleeping,
    /// Running away: health is low and the agent wants distance.
    Fleeing,
    /// Blocked for too long and unable to reach the destination.
    Stuck,
}

impl NpcState {
    /// Every state, in a fixed order.
    pub const ALL: [Self; 7] = [
        Self::Idle,
        Self::Walking,
        Self::Working,
        Self::Talking,
        Self::Sleeping,
        Self::Fleeing,
        Self::Stuck,
    ];

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Walking => "walking",
            Self::Working => "working",
            Self::Talking => "talking",
            Self::Sleeping => "sleeping",
            Self::Fleeing => "fleeing",
            Self::Stuck => "stuck",
        }
    }

    /// True when the state implies the agent should be moving.
    #[must_use]
    pub fn is_moving(self) -> bool {
        matches!(self, Self::Walking | Self::Fleeing)
    }
}

/// One simulated NPC.
///
/// Positions are the agent's **feet**, on the ground plane: `to_transform`
/// therefore places a render mesh of a 1.7 m villager directly on the terrain,
/// and the physics capsule for a tier-0 agent is that same point raised by half
/// the agent height.
#[derive(Clone, Debug)]
pub struct NpcAgent {
    /// Stable identity, as handed out by [`crate::CrowdManager::spawn`].
    pub id: NpcId,
    /// The agent's species/role.
    pub kind: NpcKind,
    /// What the agent is doing right now.
    pub state: NpcState,
    /// Feet position in world space, in metres.
    pub position: Vec3,
    /// Current velocity in metres per second, including its vertical part.
    pub velocity: Vec3,
    /// Facing, in radians, following ADR 0001: `0` looks along `-Z`.
    pub yaw: f32,
    /// The simulation level of detail this agent is currently on.
    pub tier: CrowdTier,
    /// Health in `0..=kind.max_health()`.
    pub health: f32,
    /// Index into the agent's current path, or `path.len()` when it is finished.
    pub path_cursor: usize,
    /// Seconds until the next decision. Tier 0 uses `0`, i.e. every step.
    pub decision_timer: f32,
    /// World-space destination, if any.
    pub destination: Option<Vec3>,
    /// True when the agent could not make progress last step.
    pub blocked: bool,
    /// Seconds spent blocked, for the stuck detector.
    pub blocked_time: f32,
}

impl NpcAgent {
    /// A fresh, idle agent at `position`.
    ///
    /// The crowd manager uses this internally; it is public so a game can build
    /// an agent for a save file or an ECS component without going through the
    /// spawner.
    #[must_use]
    pub fn new(id: NpcId, kind: NpcKind, position: Vec3) -> Self {
        let position = if position.is_finite() {
            position
        } else {
            Vec3::ZERO
        };
        Self {
            id,
            kind,
            state: NpcState::Idle,
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            tier: CrowdTier::Near,
            health: kind.max_health(),
            path_cursor: 0,
            decision_timer: 0.0,
            destination: None,
            blocked: false,
            blocked_time: 0.0,
        }
    }

    /// True when the agent is moving faster than [`MOVING_SPEED`].
    #[inline]
    #[must_use]
    pub fn is_moving(&self) -> bool {
        self.speed() > MOVING_SPEED
    }

    /// Horizontal speed in metres per second.
    ///
    /// Vertical motion is ignored: a top-down game cares about ground speed, and
    /// a character controller reports a large `y` component while settling.
    #[inline]
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.velocity.length_xz()
    }

    /// The unit vector the agent faces, derived from [`NpcAgent::yaw`].
    ///
    /// This is `Vec3::from_yaw(yaw)`, i.e. `(-sin yaw, 0, -cos yaw)`: at yaw `0`
    /// the agent looks along `-Z`, exactly as ADR 0001 specifies.
    #[inline]
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        Vec3::from_yaw(self.yaw)
    }

    /// Straight-line distance from the agent to `p`, in metres.
    #[inline]
    #[must_use]
    pub fn distance_to(&self, p: Vec3) -> f32 {
        self.position.distance(p)
    }

    /// The render/scene transform for this agent.
    ///
    /// The translation is the agent's feet, the rotation is the yaw about `+Y`
    /// and the scale is uniform, so a mesh authored with its origin on the
    /// ground needs no further adjustment.
    #[must_use]
    pub fn to_transform(&self) -> Transform {
        Transform::new(self.position, Quat::from_rotation_y(self.yaw), Vec3::ONE)
    }

    /// The centre of the agent's collision capsule: the feet raised by half the
    /// agent height.
    #[must_use]
    pub fn capsule_center(&self, height: f32) -> Vec3 {
        self.position + Vec3::Y * (height * 0.5)
    }

    /// Writes the agent's yaw so that it faces `direction`, if that direction
    /// has a usable horizontal component.
    ///
    /// `Vec3::from_yaw(0) == -Z`, so the inverse is `atan2(-x, -z)`, the same
    /// sign convention `Quat::to_yaw` uses.
    pub fn face(&mut self, direction: Vec3) {
        let flat = Vec3::new(direction.x, 0.0, direction.z);
        if flat.length_squared() > 1e-8 {
            self.yaw = wrap_angle((-flat.x).atan2(-flat.z));
        }
    }
}

/// Wraps an angle into `(-PI, PI]`.
#[inline]
#[must_use]
pub fn wrap_angle(angle: f32) -> f32 {
    if !angle.is_finite() {
        return 0.0;
    }
    let tau = core::f32::consts::TAU;
    let mut a = angle % tau;
    if a > core::f32::consts::PI {
        a -= tau;
    } else if a <= -core::f32::consts::PI {
        a += tau;
    }
    a
}

/// Counters and timings for one crowd step.
///
/// The split between *instantaneous* and *cumulative* fields matters:
///
/// | Field | Meaning |
/// |---|---|
/// | `active`, `tier_counts`, `blocked`, `flow_fields_cached` | a snapshot of the current state |
/// | `spawned_this_step`, `despawned_this_step`, `last_step_ms` | the most recent step only |
/// | `paths_computed`, `path_nodes_expanded` | cumulative for the lifetime of the pathfinder |
/// | `mean_step_ms`, `us_per_agent` | running means since the system was built |
///
/// `us_per_agent` is the number that decides how many NPCs fit in a frame: at a
/// 4 ms budget, everything up to `4000 / us_per_agent` agents is free.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NpcStats {
    /// Agents currently resident.
    pub active: usize,
    /// Agents created by the most recent step.
    pub spawned_this_step: usize,
    /// Agents retired by the most recent step.
    pub despawned_this_step: usize,
    /// Residents per crowd tier, indexed by [`CrowdTier::index`].
    pub tier_counts: [usize; 4],
    /// A\* searches run since the system was built (cumulative).
    pub paths_computed: u64,
    /// Grid nodes expanded by those searches (cumulative).
    pub path_nodes_expanded: u64,
    /// Flow fields currently cached.
    pub flow_fields_cached: usize,
    /// Agents that reported no progress on the most recent step.
    pub blocked: usize,
    /// Wall-clock duration of the most recent step, in milliseconds.
    pub last_step_ms: f32,
    /// Mean wall-clock duration of a step, in milliseconds.
    pub mean_step_ms: f32,
    /// Mean milliseconds per agent for the most recent step, times 1000.
    pub us_per_agent: f32,
}

impl NpcStats {
    /// Adds the cumulative pathfinding counters of `other` to `self`.
    #[must_use]
    pub fn with_paths(mut self, other: &Self) -> Self {
        self.paths_computed = other.paths_computed;
        self.path_nodes_expanded = other.path_nodes_expanded;
        self
    }

    /// Records a step of `ms` milliseconds over `active` agents, updating
    /// `last_step_ms`, `mean_step_ms` and `us_per_agent`.
    pub fn record_step(&mut self, ms: f32, active: usize) {
        let ms = if ms.is_finite() && ms >= 0.0 { ms } else { 0.0 };
        self.last_step_ms = ms;
        self.active = active;
        self.us_per_agent = if active == 0 {
            0.0
        } else {
            ms * 1000.0 / active as f32
        };
        // A running mean is enough for a HUD and keeps the system free of a
        // sample buffer; `last_step_ms` is the value an assertion should use.
        self.mean_step_ms = if self.mean_step_ms.is_finite() && self.mean_step_ms > 0.0 {
            self.mean_step_ms + (ms - self.mean_step_ms) * 0.05
        } else {
            ms
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_id_is_not_valid_and_maps_high() {
        assert!(!NpcId::INVALID.is_valid());
        assert!(NpcId(0).is_valid());
        assert_eq!(NpcId::default(), NpcId::INVALID);
        assert!(NpcId::INVALID.index() > 1_000_000);
        assert_eq!(NpcId(7).index(), 7);
    }

    #[test]
    fn kind_speed_scales_match_the_contract() {
        assert_eq!(NpcKind::Villager.speed_scale(), 1.0);
        assert_eq!(NpcKind::Guard.speed_scale(), 1.05);
        assert_eq!(NpcKind::Merchant.speed_scale(), 0.9);
        assert_eq!(NpcKind::Child.speed_scale(), 1.1);
        assert_eq!(NpcKind::Animal.speed_scale(), 0.7);
        for (i, kind) in NpcKind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), i);
            assert!(!kind.name().is_empty());
            assert!(kind.max_health() > 0.0);
        }
    }

    #[test]
    fn state_names_are_stable() {
        assert_eq!(NpcState::Walking.name(), "walking");
        assert!(NpcState::Walking.is_moving());
        assert!(NpcState::Fleeing.is_moving());
        assert!(!NpcState::Sleeping.is_moving());
        for state in NpcState::ALL {
            assert!(!state.name().is_empty());
        }
    }

    #[test]
    fn forward_follows_adr_0001() {
        let mut agent = NpcAgent::new(NpcId(0), NpcKind::Villager, Vec3::ZERO);
        agent.yaw = 0.0;
        assert!(agent.forward().approx_eq(Vec3::new(0.0, 0.0, -1.0), 1e-6));
        agent.yaw = core::f32::consts::FRAC_PI_2;
        assert!(agent.forward().approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-6));
    }

    #[test]
    fn face_is_the_inverse_of_forward() {
        let mut agent = NpcAgent::new(NpcId(0), NpcKind::Guard, Vec3::ZERO);
        for dir in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(3.0, 12.0, -0.5),
        ] {
            agent.face(dir);
            let f = agent.forward();
            let want = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
            assert!(
                f.approx_eq(want, 1e-5),
                "face({dir:?}) gave {f:?}, wanted {want:?}"
            );
        }
    }

    #[test]
    fn nan_positions_are_replaced_not_propagated() {
        let agent = NpcAgent::new(NpcId(3), NpcKind::Animal, Vec3::new(f32::NAN, 0.0, 1.0));
        assert!(agent.position.is_finite());
        assert_eq!(agent.position, Vec3::ZERO);
    }

    #[test]
    fn transform_places_the_feet_on_the_ground() {
        let mut agent = NpcAgent::new(NpcId(1), NpcKind::Merchant, Vec3::new(2.0, 1.5, -3.0));
        agent.yaw = 0.25;
        let t = agent.to_transform();
        assert_eq!(t.translation, agent.position);
        assert!(t.scale.approx_eq(Vec3::ONE, 1e-6));
        let rotated = t.rotation * Vec3::new(0.0, 0.0, -1.0);
        assert!(rotated.approx_eq(agent.forward(), 1e-5));
    }

    #[test]
    fn is_moving_uses_horizontal_speed() {
        let mut agent = NpcAgent::new(NpcId(0), NpcKind::Villager, Vec3::ZERO);
        assert!(!agent.is_moving());
        agent.velocity = Vec3::new(0.0, 9.0, 0.0);
        assert!(!agent.is_moving(), "vertical motion is not walking");
        agent.velocity = Vec3::new(1.0, 0.0, 0.0);
        assert!(agent.is_moving());
        assert!((agent.speed() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn distance_and_capsule_center() {
        let agent = NpcAgent::new(NpcId(0), NpcKind::Villager, Vec3::new(1.0, 0.0, 1.0));
        assert!((agent.distance_to(Vec3::new(4.0, 0.0, 5.0)) - 5.0).abs() < 1e-5);
        assert!(
            agent
                .capsule_center(1.7)
                .approx_eq(Vec3::new(1.0, 0.85, 1.0), 1e-6)
        );
    }

    #[test]
    fn stats_track_means_and_microseconds() {
        let mut stats = NpcStats::default();
        stats.record_step(4.0, 1000);
        assert_eq!(stats.last_step_ms, 4.0);
        assert!((stats.us_per_agent - 4.0).abs() < 1e-4);
        stats.record_step(f32::NAN, 0);
        assert_eq!(stats.last_step_ms, 0.0);
        assert_eq!(stats.us_per_agent, 0.0);
        let paths = NpcStats {
            paths_computed: 3,
            path_nodes_expanded: 30,
            ..NpcStats::default()
        };
        let merged = stats.with_paths(&paths);
        assert_eq!(merged.paths_computed, 3);
        assert_eq!(merged.path_nodes_expanded, 30);
    }

    #[test]
    fn wrap_angle_is_stable() {
        assert!((wrap_angle(0.0)).abs() < 1e-6);
        assert!((wrap_angle(core::f32::consts::TAU) - 0.0).abs() < 1e-6);
        assert!(wrap_angle(f32::NAN) == 0.0);
        let a = wrap_angle(core::f32::consts::PI + 0.5);
        assert!(a < 0.0 && a > -core::f32::consts::PI);
    }
}
