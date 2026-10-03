//! Spawning: where a new agent appears, and what it should be.
//!
//! Two rules make a town feel inhabited rather than sprinkled:
//!
//! 1. **A spawn point is somewhere an agent can stand.** It is inside a loaded
//!    chunk, on a walkable tile, above the waterline, and not inside a wall or
//!    a prop.
//! 2. **The place picks the kind.** A point in the middle of a town produces
//!    villagers, merchants, guards and children; a point on a road produces
//!    travellers and patrols; open country produces animals. A road therefore
//!    never fills up with merchants who never leave the plaza.
//!
//! Every choice is a pure function of the world seed and the caller's `index`,
//! so a town repopulated from the same seed is repopulated with the same people
//! in the same places.

use noxel_core::math::{Aabb, Vec3};
use noxel_core::rng::{Pcg32, RngStream};
use noxel_world::WorldStreamer;
use noxel_world::town;

use crate::NpcConfig;
use crate::agent::NpcKind;

/// Where a spawn point is, in the world's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnKind {
    /// Inside a town's built-up area.
    Town,
    /// On a paved road, street or connector.
    Road,
    /// In open country, away from any town.
    Wilderness,
    /// Under a roof: inside a building's footprint, on its floor.
    Interior,
}

impl SpawnKind {
    /// Every place, in a fixed order.
    pub const ALL: [Self; 4] = [Self::Town, Self::Road, Self::Wilderness, Self::Interior];

    /// A short human-readable name, for debug views and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Town => "town",
            Self::Road => "road",
            Self::Wilderness => "wilderness",
            Self::Interior => "interior",
        }
    }
}

/// How many sample points a single spawn attempt may try before giving up.
const MAX_ATTEMPTS: u32 = 48;

/// How many extra points to try when the chosen one is inside geometry.
const MAX_GEOMETRY_RETRIES: u32 = 4;

/// The chance of preferring each place, in [`SpawnKind::ALL`] order.
///
/// Towns dominate because that is where a player meets people; interiors are
/// rare because most buildings are homes the crowd only enters at night.
const PLACE_WEIGHTS: [f32; 4] = [0.40, 0.22, 0.30, 0.08];

/// Chooses where agents appear near a moving crowd centre.
#[derive(Clone, Debug)]
pub struct NpcSpawner {
    config: NpcConfig,
}

impl NpcSpawner {
    /// A spawner driven by `config`.
    #[must_use]
    pub fn new(config: NpcConfig) -> Self {
        Self { config }
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> &NpcConfig {
        &self.config
    }

    /// Replaces the configuration.
    pub fn set_config(&mut self, config: NpcConfig) {
        self.config = config;
    }

    /// Chooses a spawn point near `center`.
    ///
    /// Returns the kind that suits the place and a ground position within
    /// [`NpcConfig::spawn_radius`] of `center`. Deterministic per
    /// `(seed, index)`: the same index always produces the same point, so a
    /// crowd can be rebuilt identically after a load.
    ///
    /// Returns `None` when nothing usable could be found — no resident chunk,
    /// no walkable ground, or a crowd centre outside the streamed world.
    pub fn choose_spawn(
        &mut self,
        streamer: &WorldStreamer,
        center: Vec3,
        index: u64,
    ) -> Option<(NpcKind, Vec3)> {
        if !center.is_finite() {
            return None;
        }
        let seed = streamer.generator().config().seed;
        let mut rng = RngStream::indexed(seed, "npc/spawn", index).rng();
        let radius = if self.config.spawn_radius.is_finite() {
            self.config.spawn_radius.max(1.0)
        } else {
            70.0
        } * 0.9;
        let preferred = preferred_place(&mut rng);
        let mut fallback: Option<(NpcKind, Vec3)> = None;
        let mut geometry_retries = 0;
        for _ in 0..MAX_ATTEMPTS {
            let angle = rng.range_f32(0.0, core::f32::consts::TAU);
            let distance = radius * rng.range_f32(0.15, 1.0).sqrt();
            let candidate = Vec3::new(
                center.x + angle.cos() * distance,
                0.0,
                center.z + angle.sin() * distance,
            );
            let Some(point) = usable_point(streamer, candidate) else {
                continue;
            };
            let kind = Self::kind_for(streamer, point, index);
            if Self::classify(streamer, point) != preferred && fallback.is_some() {
                continue;
            }
            if blocked_by_geometry(streamer, point) {
                geometry_retries += 1;
                if geometry_retries > MAX_GEOMETRY_RETRIES {
                    return None;
                }
                continue;
            }
            if Self::classify(streamer, point) == preferred {
                return Some((kind, point));
            }
            if fallback.is_none() {
                fallback = Some((kind, point));
            }
        }
        fallback.or_else(|| {
            Self::spiral_fallback(streamer, center, radius, index)
                .map(|point| (Self::kind_for(streamer, point, index), point))
        })
    }

    /// The kind that suits a place.
    ///
    /// The classification is the same one [`NpcSpawner::choose_spawn`] uses, so
    /// a caller can ask "would this be a guard?" about a position it already
    /// has — the debug overlay and the spawner cannot disagree.
    #[must_use]
    pub fn kind_for(streamer: &WorldStreamer, position: Vec3, index: u64) -> NpcKind {
        let seed = streamer.generator().config().seed;
        let roll = RngStream::indexed(seed, "npc/kind", index).rng().next_f32();
        match Self::classify(streamer, position) {
            SpawnKind::Town => {
                if roll < 0.60 {
                    NpcKind::Villager
                } else if roll < 0.78 {
                    NpcKind::Merchant
                } else if roll < 0.90 {
                    NpcKind::Guard
                } else {
                    NpcKind::Child
                }
            }
            SpawnKind::Road => {
                if roll < 0.45 {
                    NpcKind::Merchant
                } else if roll < 0.80 {
                    NpcKind::Guard
                } else {
                    NpcKind::Villager
                }
            }
            SpawnKind::Wilderness => {
                if roll < 0.65 {
                    NpcKind::Animal
                } else if roll < 0.90 {
                    NpcKind::Villager
                } else {
                    NpcKind::Merchant
                }
            }
            SpawnKind::Interior => {
                if roll < 0.6 {
                    NpcKind::Merchant
                } else {
                    NpcKind::Villager
                }
            }
        }
    }

    /// Where a position is, in the world's terms.
    ///
    /// A position in a chunk that is not resident is reported as
    /// [`SpawnKind::Wilderness`], the answer that claims the least.
    #[must_use]
    pub fn classify(streamer: &WorldStreamer, position: Vec3) -> SpawnKind {
        if !position.is_finite() {
            return SpawnKind::Wilderness;
        }
        let Some(chunk) = streamer.chunk_at(position) else {
            return SpawnKind::Wilderness;
        };
        if chunk
            .buildings
            .iter()
            .any(|building| building.contains_xz(position))
        {
            return SpawnKind::Interior;
        }
        let generator = streamer.generator();
        if generator.is_road_at(position.x, position.z) {
            return SpawnKind::Road;
        }
        if in_town(generator.config(), position) {
            return SpawnKind::Town;
        }
        SpawnKind::Wilderness
    }

    /// The last resort: walk a golden-angle spiral outwards from `center`
    /// looking for any standable point.
    fn spiral_fallback(
        streamer: &WorldStreamer,
        center: Vec3,
        radius: f32,
        index: u64,
    ) -> Option<Vec3> {
        let seed = streamer.generator().config().seed;
        let mut rng = RngStream::indexed(seed, "npc/spawn/spiral", index).rng();
        let phase = rng.range_f32(0.0, core::f32::consts::TAU);
        for step in 0..MAX_ATTEMPTS * 2 {
            let t = step as f32 / (MAX_ATTEMPTS * 2) as f32;
            let distance = radius * (0.05 + 0.95 * t);
            let angle = phase + step as f32 * 2.399_963;
            let candidate = Vec3::new(
                center.x + angle.cos() * distance,
                0.0,
                center.z + angle.sin() * distance,
            );
            if let Some(point) = usable_point(streamer, candidate) {
                return Some(point);
            }
        }
        None
    }
}

/// Picks a preferred place from [`PLACE_WEIGHTS`].
fn preferred_place(rng: &mut Pcg32) -> SpawnKind {
    match rng.weighted_index(&PLACE_WEIGHTS) {
        Some(0) => SpawnKind::Town,
        Some(1) => SpawnKind::Road,
        Some(2) => SpawnKind::Wilderness,
        _ => SpawnKind::Interior,
    }
}

/// True when `p` is inside a town's built-up disc.
fn in_town(config: &noxel_world::WorldConfig, p: Vec3) -> bool {
    if !config.town_radius_chunks.is_positive() {
        return false;
    }
    let cell = town::nearest_town_cell(config, p.x, p.z);
    let centre = town::town_centre(config, cell);
    let radius = config.town_radius_chunks as f32 * config.chunk_world_size();
    let dx = p.x - centre.x;
    let dz = p.z - centre.z;
    (dx * dx + dz * dz).sqrt() <= radius
}

/// Turns a candidate XZ position into a standable ground position, or `None`.
fn usable_point(streamer: &WorldStreamer, candidate: Vec3) -> Option<Vec3> {
    if !candidate.is_finite() || streamer.chunk_at(candidate).is_none() {
        return None;
    }
    let ground = Vec3::new(candidate.x, streamer.height_at(candidate), candidate.z);
    if !ground.is_finite() || !streamer.is_walkable(ground) {
        return None;
    }
    Some(ground)
}

/// True when a standing agent at `p` would be inside a prop, a wall or a tree.
fn blocked_by_geometry(streamer: &WorldStreamer, p: Vec3) -> bool {
    let region = Aabb::new(
        Vec3::new(p.x - 0.4, p.y + 0.1, p.z - 0.4),
        Vec3::new(p.x + 0.4, p.y + 1.7, p.z + 0.4),
    );
    let mut found: Vec<(Aabb, u64)> = Vec::new();
    streamer.colliders_in(region, &mut found);
    !found.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_weights_cover_every_kind() {
        assert_eq!(PLACE_WEIGHTS.len(), SpawnKind::ALL.len());
        let total: f32 = PLACE_WEIGHTS.iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "weights must sum to one: {total}"
        );
        for (i, kind) in SpawnKind::ALL.iter().enumerate() {
            assert!(!kind.name().is_empty());
            assert!(
                PLACE_WEIGHTS[i] > 0.0,
                "{} can never be chosen",
                kind.name()
            );
        }
    }

    #[test]
    fn preferred_places_are_reachable_and_deterministic() {
        let mut counts = [0usize; 4];
        for index in 0..500u64 {
            let mut rng = RngStream::indexed(3, "npc/spawn", index).rng();
            let place = preferred_place(&mut rng);
            counts[SpawnKind::ALL.iter().position(|k| *k == place).unwrap_or(0)] += 1;
            let mut again = RngStream::indexed(3, "npc/spawn", index).rng();
            assert_eq!(preferred_place(&mut again), place);
        }
        for count in counts {
            assert!(count > 10, "weighting is badly skewed: {counts:?}");
        }
    }

    #[test]
    fn spawner_keeps_its_config() {
        let mut spawner = NpcSpawner::new(NpcConfig {
            seed: 5,
            ..NpcConfig::default()
        });
        assert_eq!(spawner.config().seed, 5);
        let other = NpcConfig {
            seed: 6,
            ..NpcConfig::default()
        };
        spawner.set_config(other);
        assert_eq!(spawner.config().seed, 6);
        assert!(format!("{spawner:?}").contains("NpcSpawner"));
    }

    #[test]
    fn in_town_rejects_absurd_configs() {
        let mut world = noxel_world::WorldConfig::new(1);
        world.town_radius_chunks = 0;
        assert!(!in_town(&world, Vec3::new(10.0, 0.0, 10.0)));
        world.town_radius_chunks = 3;
        let centre = town::town_centre(&world, town::nearest_town_cell(&world, 0.0, 0.0));
        assert!(in_town(&world, centre));
        // Half the town spacing away from any centre is outside every disc.
        let spacing = world.town_spacing() as f32 * world.chunk_world_size();
        assert!(!in_town(
            &world,
            centre + Vec3::new(spacing * 0.5, 0.0, 0.0)
        ));
    }
}
