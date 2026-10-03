//! World generation: the height field, the biomes, the tiles and the generator
//! that turns a [`ChunkPos`] into a [`Chunk`].
//!
//! Everything here is a **pure function of `(seed, position)`**. There is no
//! shared mutable random number generator: each stage draws from its own
//! addressable [`RngStream`], so chunk `(900, -400)` is byte-identical whether
//! it is generated first, last, or on another machine entirely. That property is
//! what makes the world streamable and the golden-world tests meaningful.
//!
//! # The pipeline
//!
//! 1. **Height** — a domain-warped fBm relief, a low-frequency continent mask
//!    that produces real coastlines and inland seas, and ridged noise that only
//!    bites above a mountain threshold. Roads are graded into the field and town
//!    discs are flattened towards their plaza.
//! 2. **Biome** — temperature (a low-frequency gradient plus noise) and moisture
//!    (fBm) select from [`BiomeTable`]; below sea level water wins, above the
//!    mountain threshold rock wins.
//! 3. **Tiles** — water, then the road network, then rock on steep slopes, then
//!    beach sand, then the biome's base tile with an accent tile where a small
//!    noise crosses its threshold.
//! 4. **Towns and props** — towns are placed on a chunk lattice, their buildings
//!    instantiated from the prefab library (or the procedural fallback), and
//!    trees and rocks are scattered by density, never on a road, in water, on a
//!    steep slope or inside a building.
//!
//! # Missing tiles never panic
//!
//! Every tile name the generator wants is resolved **once**, at construction,
//! through a documented fallback chain: the named tile, then its aliases, then
//! the first tile carrying the matching flag, then `"grass"`/`"dirt"`, then the
//! tile set's first tile, then `0`. An empty tile set therefore produces a
//! valid world in which every tile is id `0`.
//!
//! ```
//! use std::sync::Arc;
//! use noxel_asset::format::TileSet;
//! use noxel_core::math::ChunkPos;
//! use noxel_world::{WorldConfig, WorldGenerator};
//!
//! let config = WorldConfig::new(7);
//! let set = Arc::new(TileSet {
//!     name: "ground".into(),
//!     texture: "ground.png".into(),
//!     tile_size: 16,
//!     tiles: Vec::new(),
//! });
//! let generator = WorldGenerator::new(config, set, Vec::new());
//! let chunk = generator.generate_chunk(ChunkPos::new(3, -2));
//! assert_eq!(chunk.pos, ChunkPos::new(3, -2));
//! assert_eq!(chunk.tiles.len(), 32 * 32);
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use noxel_asset::format::{Prefab, TileFlags, TileSet};
use noxel_core::math::{
    Aabb, ChunkPos, Vec2, Vec3, fbm_2d, ridged_2d, smoothstep, value_2d, warped_fbm_2d,
};
use noxel_core::rng::RngStream;

use crate::biome::{BiomeId, BiomeTable};
use crate::chunk::{Chunk, PropInstance};
use crate::road::{RoadNetwork, RoadSegment, nearest_lines};
use crate::town::{
    BuildingInstance, TownPlan, clip_xz, nearest_town_cell, town_centre, town_streets,
};

/// Continent-mask frequency: one feature every ~1200 m.
const CONTINENT_FREQ: f32 = 0.00085;
/// Base relief frequency: one feature every ~285 m.
const RELIEF_FREQ: f32 = 0.0035;
/// Ridge frequency: one mountain range every ~550 m.
const RIDGE_FREQ: f32 = 0.0018;
/// A tile is rock above this slope (rise over run), about 29 degrees.
pub const ROCK_SLOPE: f32 = 0.55;
/// Props are rejected above this slope, about 40 degrees.
pub const MAX_PROP_SLOPE: f32 = 0.84;
/// Height above sea level at which a shore tile becomes beach sand.
const BEACH_HEIGHT: f32 = 0.6;
/// Height above sea level at which the mountains biome takes over, in units of
/// `height_scale`.
const MOUNTAIN_THRESHOLD: f32 = 1.15;
/// Height above sea level at which the hills biome takes over, in units of
/// `height_scale`.
const HILL_THRESHOLD: f32 = 0.5;
/// Fraction of the town radius that is flattened to the plaza level.
const TOWN_INNER_FLATTEN: f32 = 0.55;
/// Scales `prop_density` into a per-tile probability.
const PROP_DENSITY_SCALE: f32 = 10.0;
/// Upper bound on `tiles_per_chunk`, so a hand-edited config cannot allocate the
/// machine to death.
pub const MAX_TILES_PER_CHUNK: u32 = 1024;

/// World coordinates are clamped to this magnitude before they reach the noise.
///
/// Ten thousand kilometres is far outside any streamable world, and staying
/// inside it keeps every intermediate lattice index exact — beyond it the
/// `f32`-to-`i32` conversions saturate, which is a panic in a debug build. The
/// clamp makes `sample_height(1e30, 0.0)` a defined, finite answer instead.
pub const MAX_WORLD_COORDINATE: f32 = 1.0e7;

/// Clamps a world coordinate into the usable range, mapping NaN to `0`.
#[must_use]
fn clamp_world(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-MAX_WORLD_COORDINATE, MAX_WORLD_COORDINATE)
    } else {
        0.0
    }
}

/// The world's configuration.
///
/// [`WorldConfig::default`] is a usable 32-tile-chunk world with a 6-chunk view
/// distance; [`WorldConfig::new`] takes only a seed and fills in the same
/// defaults.
#[derive(Clone, Debug)]
pub struct WorldConfig {
    /// The world seed. Every generated byte descends from it.
    pub seed: u64,
    /// Edge length of one tile, in metres.
    pub tile_size: f32,
    /// Tiles along one edge of a chunk.
    pub tiles_per_chunk: u32,
    /// World size of a chunk, in metres. Kept in sync with `tiles_per_chunk *
    /// tile_size` by [`WorldConfig::new`]; see [`WorldConfig::sync_sizes`] if you
    /// edit the tile count by hand.
    pub chunk_world_size: f32,
    /// Chunks kept generated around the streaming focus.
    pub view_distance_chunks: i32,
    /// Chunks kept cached around the streaming focus before eviction.
    pub keep_distance_chunks: i32,
    /// Hard cap on the number of cached chunks.
    pub max_cached_chunks: usize,
    /// The waterline. Terrain below it is water.
    pub sea_level: f32,
    /// Metres of relief in the base terrain.
    pub height_scale: f32,
    /// A macro road runs along every `road_grid_chunks`-th chunk boundary.
    pub road_grid_chunks: i32,
    /// How far a road's centre line may wander from its boundary, as a fraction
    /// of a chunk.
    pub road_jitter: f32,
    /// Paved width of a road, in tiles.
    pub road_width_tiles: u32,
    /// A town exists at every `town_spacing_chunks`-th chunk-lattice position.
    pub town_spacing_chunks: i32,
    /// Radius of a town's built-up area, in chunks.
    pub town_radius_chunks: i32,
    /// Buildings a town tries to place at full density.
    pub buildings_per_town: u32,
    /// Whether trees and rocks are scattered at all.
    pub generate_props: bool,
    /// Per-walkable-tile probability of a prop, scaled by the biome's density.
    pub prop_density: f32,
}

impl WorldConfig {
    /// A configuration for `seed` with the engine's tuned defaults: 32-tile
    /// chunks of 1 m tiles, a 6-chunk view distance, 8-chunk road spacing and a
    /// town every 10 chunks.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let tiles_per_chunk = 32;
        let tile_size = 1.0;
        let view_distance_chunks = 6;
        Self {
            seed,
            tile_size,
            tiles_per_chunk,
            chunk_world_size: tiles_per_chunk as f32 * tile_size,
            view_distance_chunks,
            keep_distance_chunks: view_distance_chunks + 2,
            max_cached_chunks: 256,
            sea_level: 0.0,
            height_scale: 8.0,
            road_grid_chunks: 8,
            road_jitter: 0.25,
            road_width_tiles: 3,
            town_spacing_chunks: 10,
            town_radius_chunks: 3,
            buildings_per_town: 18,
            generate_props: true,
            prop_density: 0.02,
        }
    }

    /// The effective world size of a chunk, in metres.
    ///
    /// The stored field wins when it is finite and positive; otherwise the value
    /// is derived from `tiles_per_chunk * tile_size` so a half-edited config
    /// still produces a usable world instead of dividing by zero.
    #[must_use]
    pub fn chunk_world_size(&self) -> f32 {
        if self.chunk_world_size.is_finite() && self.chunk_world_size > 1e-3 {
            self.chunk_world_size
        } else {
            let tiles = self.tiles_per_chunk.clamp(1, MAX_TILES_PER_CHUNK) as f32;
            let tile_size = self.tile_size();
            (tiles * tile_size).max(1e-3)
        }
    }

    /// The effective tile size: finite and strictly positive.
    #[must_use]
    pub fn tile_size(&self) -> f32 {
        if self.tile_size.is_finite() && self.tile_size > 1e-4 {
            self.tile_size
        } else {
            1.0
        }
    }

    /// The effective road spacing, in metres.
    #[must_use]
    pub fn road_spacing(&self) -> f32 {
        self.road_grid_chunks.max(1) as f32 * self.chunk_world_size()
    }

    /// The effective town spacing, in chunks.
    #[must_use]
    pub fn town_spacing(&self) -> i32 {
        self.town_spacing_chunks.max(1)
    }

    /// Re-derives `chunk_world_size` from `tiles_per_chunk * tile_size`.
    ///
    /// Call this after editing `tiles_per_chunk` or `tile_size` by hand.
    pub fn sync_sizes(&mut self) {
        let tiles = self.tiles_per_chunk.clamp(1, MAX_TILES_PER_CHUNK) as f32;
        self.chunk_world_size = (tiles * self.tile_size()).max(1e-3);
        self.keep_distance_chunks = self.keep_distance_chunks.max(self.view_distance_chunks);
    }

    /// Tiles per chunk, sanitised and clamped to [`MAX_TILES_PER_CHUNK`].
    #[must_use]
    pub fn safe_tiles_per_chunk(&self) -> u32 {
        self.tiles_per_chunk.clamp(1, MAX_TILES_PER_CHUNK)
    }
}

impl Default for WorldConfig {
    /// The default world: seed `0`, 32-tile chunks, 6-chunk view distance.
    fn default() -> Self {
        Self::new(0)
    }
}

/// Counters and timings for a [`WorldGenerator`].
///
/// The generator is shared immutably (and possibly across threads), so its
/// counters are atomics rather than fields a caller has to thread through.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenStats {
    /// Chunks generated since the generator was built.
    pub chunks_generated: u64,
    /// Tiles written since the generator was built.
    pub tiles_written: u64,
    /// Buildings placed into chunks since the generator was built.
    pub buildings_placed: u64,
    /// Props placed into chunks since the generator was built.
    pub props_placed: u64,
    /// Tiles stamped with a road tile since the generator was built.
    pub roads_carved: u64,
    /// Wall-clock time of the most recent chunk, in milliseconds.
    pub last_ms: f32,
    /// Mean wall-clock time per chunk, in milliseconds.
    pub mean_ms: f32,
}

/// Atomic backing store for [`GenStats`].
#[derive(Debug, Default)]
struct Counters {
    chunks: AtomicU64,
    tiles: AtomicU64,
    buildings: AtomicU64,
    props: AtomicU64,
    roads: AtomicU64,
    last_us: AtomicU32,
    total_us: AtomicU64,
}

/// Every tile id the generator needs, resolved once at construction.
#[derive(Clone, Copy, Debug)]
struct TileSlots {
    water: u32,
    sand: u32,
    road: u32,
    rock: u32,
    floor: u32,
    base: [u32; BiomeId::ALL.len()],
    accent: [u32; BiomeId::ALL.len()],
    biome_water: [Option<u32>; BiomeId::ALL.len()],
}

/// Resolves tile names against a tile set with a documented fallback chain.
struct TileResolver<'a> {
    set: &'a TileSet,
}

impl<'a> TileResolver<'a> {
    /// The first of `names` that the tile set actually has.
    fn named(&self, names: &[&str]) -> Option<u32> {
        names
            .iter()
            .find_map(|name| self.set.tile_by_name(name))
            .map(|tile| tile.id)
    }

    /// The first tile matching a flag.
    fn flagged(&self, predicate: impl Fn(TileFlags) -> bool) -> Option<u32> {
        self.set
            .tiles
            .iter()
            .find(|tile| predicate(tile.flags))
            .map(|tile| tile.id)
    }

    /// The last resort: the tile set's first tile, or `0` when it has none.
    fn global(&self) -> u32 {
        self.set.tiles.first().map_or(0, |tile| tile.id)
    }

    /// A slot's tile: names, then aliases, then a flag, then the global chain.
    fn resolve(&self, names: &[&str], predicate: impl Fn(TileFlags) -> bool) -> u32 {
        self.named(names)
            .or_else(|| self.flagged(predicate))
            .or_else(|| self.named(&["grass", "dirt", "floor", "stone", "sand", "water"]))
            .unwrap_or_else(|| self.global())
    }

    /// Resolves every slot the generator needs.
    fn slots(&self, table: &BiomeTable) -> TileSlots {
        let mut base = [0u32; BiomeId::ALL.len()];
        let mut accent = [0u32; BiomeId::ALL.len()];
        let mut biome_water = [None; BiomeId::ALL.len()];
        for (i, biome) in table.all().iter().enumerate() {
            base[i] = self.resolve(&[biome.base_tile], |flags| flags.walkable);
            accent[i] = self.resolve(&[biome.accent_tile], |flags| flags.walkable);
            biome_water[i] = biome
                .water_tile
                .and_then(|name| self.named(&[name]))
                .or_else(|| self.flagged(|flags| flags.water));
        }
        let water = self.resolve(&["water", "deep_water", "lake", "ocean", "sea"], |flags| {
            flags.water
        });
        let sand = self.resolve(&["sand", "beach", "gravel"], |flags| flags.buildable);
        let road = self.resolve(
            &["road", "path", "cobble", "cobblestone", "street"],
            |flags| flags.road,
        );
        let rock = self.resolve(&["rock", "stone", "cliff", "scree"], |flags| {
            flags.blocks_sight
        });
        let floor = self.resolve(&["floor", "planks", "wood_floor"], |flags| flags.walkable);
        // A completely empty tile set collapses every slot to the same id.
        TileSlots {
            water,
            sand,
            road,
            rock,
            floor,
            base,
            accent,
            biome_water,
        }
    }
}

/// Generates chunks, and answers world queries without generating anything.
#[derive(Debug)]
pub struct WorldGenerator {
    config: WorldConfig,
    tile_set: Arc<TileSet>,
    prefabs: Vec<Arc<Prefab>>,
    biomes: BiomeTable,
    slots: TileSlots,
    counters: Counters,
}

impl WorldGenerator {
    /// Creates a generator from a configuration, a tile set and a prefab
    /// library.
    ///
    /// `tile_set` and `prefabs` are shared with the asset database: the
    /// generator never mutates them, and it tolerates both being empty.
    #[must_use]
    pub fn new(config: WorldConfig, tile_set: Arc<TileSet>, prefabs: Vec<Arc<Prefab>>) -> Self {
        let biomes = BiomeTable::new();
        let slots = {
            let resolver = TileResolver { set: &tile_set };
            resolver.slots(&biomes)
        };
        Self {
            config,
            tile_set,
            prefabs,
            biomes,
            slots,
            counters: Counters::default(),
        }
    }

    /// The configuration this generator was built with.
    #[must_use]
    pub fn config(&self) -> &WorldConfig {
        &self.config
    }

    /// The tile set every tile id refers to.
    #[must_use]
    pub fn tile_set(&self) -> &TileSet {
        &self.tile_set
    }

    /// Mutable access to the configuration.
    ///
    /// The generator holds no derived state that depends on the configuration,
    /// so a change takes effect immediately — but a streamer caching chunks
    /// from the old configuration must be cleared.
    pub fn config_mut(&mut self) -> &mut WorldConfig {
        &mut self.config
    }

    /// The biome table the generator selects from.
    #[must_use]
    pub fn biome_table(&self) -> &BiomeTable {
        &self.biomes
    }

    /// The prefab library, in the order the generator picks from it.
    #[must_use]
    pub fn prefabs(&self) -> &[Arc<Prefab>] {
        &self.prefabs
    }

    /// Terrain height in metres at a world XZ position, from the same field the
    /// chunks use.
    ///
    /// This includes the road grading and the town flattening, so it agrees with
    /// [`Chunk::heights`] to within floating-point rounding everywhere.
    #[must_use]
    pub fn sample_height(&self, world_x: f32, world_z: f32) -> f32 {
        if !world_x.is_finite() || !world_z.is_finite() {
            return self.config.sea_level;
        }
        let h = self.base_height(world_x, world_z);
        let h = self.carve_roads(world_x, world_z, h);
        self.flatten_towns(world_x, world_z, h)
    }

    /// Slope magnitude (rise over run) at a world XZ position.
    ///
    /// A central difference of [`WorldGenerator::sample_height`] over one tile,
    /// so `atan(slope)` is the ground angle in radians.
    #[must_use]
    pub fn sample_slope(&self, world_x: f32, world_z: f32) -> f32 {
        if !world_x.is_finite() || !world_z.is_finite() {
            return 0.0;
        }
        let e = self.config.tile_size().max(1e-3);
        let dx = (self.sample_height(world_x + e, world_z)
            - self.sample_height(world_x - e, world_z))
            / (2.0 * e);
        let dz = (self.sample_height(world_x, world_z + e)
            - self.sample_height(world_x, world_z - e))
            / (2.0 * e);
        if dx.is_finite() && dz.is_finite() {
            (dx * dx + dz * dz).sqrt()
        } else {
            0.0
        }
    }

    /// The biome at a world XZ position.
    #[must_use]
    pub fn biome_at(&self, world_x: f32, world_z: f32) -> BiomeId {
        if !world_x.is_finite() || !world_z.is_finite() {
            return BiomeId::PLAINS;
        }
        let height = self.sample_height(world_x, world_z);
        self.biome_for(world_x, world_z, height)
    }

    /// True when a world position lies on a paved road.
    ///
    /// Covers the macro lattice, the nearest town's street grid and the
    /// connector between them — exactly the bands the chunk tiles are stamped
    /// from.
    #[must_use]
    pub fn is_road_at(&self, world_x: f32, world_z: f32) -> bool {
        self.road_band(world_x, world_z, None)
    }

    /// The town plan covering `chunk`, if any.
    ///
    /// Deterministic and side-effect free: two chunks that both touch the same
    /// town get identical plans, so they agree about where the walls are.
    #[must_use]
    pub fn town_at(&self, chunk: ChunkPos) -> Option<TownPlan> {
        let cell = self.town_cell_for_chunk(chunk)?;
        Some(self.plan_town(cell))
    }

    /// Current generation statistics.
    #[must_use]
    pub fn stats(&self) -> GenStats {
        let chunks = self.counters.chunks.load(Ordering::Relaxed);
        let total_us = self.counters.total_us.load(Ordering::Relaxed);
        GenStats {
            chunks_generated: chunks,
            tiles_written: self.counters.tiles.load(Ordering::Relaxed),
            buildings_placed: self.counters.buildings.load(Ordering::Relaxed),
            props_placed: self.counters.props.load(Ordering::Relaxed),
            roads_carved: self.counters.roads.load(Ordering::Relaxed),
            last_ms: f32::from_bits(self.counters.last_us.load(Ordering::Relaxed)) / 1000.0,
            mean_ms: if chunks == 0 {
                0.0
            } else {
                (total_us as f32 / chunks as f32) / 1000.0
            },
        }
    }

    /// Generates the chunk at `pos`.
    ///
    /// Pure: the result depends only on the configuration and `pos`.
    #[must_use]
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let started = Instant::now();
        let chunk = self.build_chunk(pos);
        let elapsed_us = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
        self.counters.chunks.fetch_add(1, Ordering::Relaxed);
        self.counters
            .tiles
            .fetch_add(chunk.chunk.tiles.len() as u64, Ordering::Relaxed);
        self.counters
            .buildings
            .fetch_add(chunk.chunk.buildings.len() as u64, Ordering::Relaxed);
        self.counters
            .props
            .fetch_add(chunk.chunk.props.len() as u64, Ordering::Relaxed);
        self.counters
            .roads
            .fetch_add(chunk.road_tiles as u64, Ordering::Relaxed);
        self.counters.last_us.store(elapsed_us, Ordering::Relaxed);
        self.counters
            .total_us
            .fetch_add(u64::from(elapsed_us), Ordering::Relaxed);
        let mut chunk = chunk.chunk;
        chunk.generated_ms = elapsed_us as f32 / 1000.0;
        chunk
    }

    // --- height field ------------------------------------------------------

    /// Terrain before roads and towns are carved.
    fn base_height(&self, x: f32, z: f32) -> f32 {
        let cfg = &self.config;
        let seed = cfg.seed;
        let x = clamp_world(x);
        let z = clamp_world(z);
        let scale = if cfg.height_scale.is_finite() {
            cfg.height_scale
        } else {
            0.0
        };
        // A low-frequency mask that decides ocean, coast or inland.
        let continent = fbm_2d(
            x * CONTINENT_FREQ,
            z * CONTINENT_FREQ,
            seed ^ 0x00C0_FFEE_0000_0001,
            4,
            2.0,
            0.5,
        );
        let land = smoothstep(-0.30, 0.35, continent);
        // Domain-warped relief: this is what makes bays and peninsulas.
        let relief = warped_fbm_2d(
            x * RELIEF_FREQ,
            z * RELIEF_FREQ,
            seed ^ 0x00BA_5E11_0000_0002,
            5,
            1.25,
        );
        // Ridges only bite once the range has built up.
        let ridge = ridged_2d(
            x * RIDGE_FREQ,
            z * RIDGE_FREQ,
            seed ^ 0x00D1_D6E5_0000_0003,
            5,
            2.0,
            0.5,
        );
        let mountain = smoothstep(0.55, 0.92, ridge * (0.55 + 0.45 * land));
        let mut h = cfg.sea_level;
        h += relief * scale * (0.20 + 0.80 * land);
        h += (land - 0.5) * scale * 0.9;
        h += mountain * scale * 2.1 * (0.45 + 0.55 * ridge);
        h
    }

    /// Temperature in `0..1`: a low-frequency gradient plus noise.
    fn temperature(&self, x: f32, z: f32) -> f32 {
        let x = clamp_world(x);
        let z = clamp_world(z);
        let noise = fbm_2d(
            x * 0.00035,
            z * 0.00035,
            self.config.seed ^ 0x007E_0000_0000_0004,
            4,
            2.0,
            0.5,
        );
        // North (+Z) is colder; the gradient is deliberately slow.
        smoothstep(-1.0, 1.0, 1.7 * noise - z * 0.00002)
    }

    /// Moisture in `0..1`, from fBm.
    fn moisture(&self, x: f32, z: f32) -> f32 {
        let x = clamp_world(x);
        let z = clamp_world(z);
        let noise = fbm_2d(
            x * 0.0005,
            z * 0.0005,
            self.config.seed ^ 0x000D_0000_0000_0005,
            5,
            2.0,
            0.5,
        );
        smoothstep(-1.0, 1.0, 1.5 * noise)
    }

    /// Biome selection from a height sample.
    fn biome_for(&self, x: f32, z: f32, height: f32) -> BiomeId {
        let cfg = &self.config;
        if height < cfg.sea_level {
            return BiomeId::LAKE;
        }
        let scale = cfg.height_scale.abs().max(1e-3);
        let above = height - cfg.sea_level;
        if above > MOUNTAIN_THRESHOLD * scale {
            return BiomeId::MOUNTAINS;
        }
        let temperature = self.temperature(x, z);
        let moisture = self.moisture(x, z);
        if temperature < 0.30 {
            return BiomeId::TUNDRA;
        }
        if above > HILL_THRESHOLD * scale {
            return BiomeId::HILLS;
        }
        if moisture > 0.66 && temperature > 0.45 {
            return BiomeId::SWAMP;
        }
        if temperature > 0.70 && moisture < 0.42 {
            return BiomeId::DESERT;
        }
        if moisture > 0.52 {
            return BiomeId::FOREST;
        }
        BiomeId::PLAINS
    }

    /// Grades the macro roads into the height field.
    ///
    /// The road surface is flat across its width and never below the waterline,
    /// so a road crossing a lake becomes a causeway instead of a hole in the
    /// network. The blend is a function of the distance to the nearest lattice
    /// line in each axis, which is continuous everywhere; the *selection* of a
    /// line jumps at the mid-point between two lines, but the blend weight is
    /// exactly zero there because the lines are 256 m apart and the shoulder is
    /// a few metres wide.
    fn carve_roads(&self, x: f32, z: f32, h: f32) -> f32 {
        let cfg = &self.config;
        let ts = cfg.tile_size();
        let half = cfg.road_width_tiles.max(1) as f32 * ts * 0.5;
        let shoulder = half + ts * 2.0;
        let (line_x, line_z) = nearest_lines(cfg, Vec3::new(x, 0.0, z));
        let weight_x = 1.0 - smoothstep(half, shoulder, (x - line_x).abs());
        let weight_z = 1.0 - smoothstep(half, shoulder, (z - line_z).abs());
        let total = weight_x + weight_z;
        if total <= 0.0 {
            return h;
        }
        let floor = cfg.sea_level + ts * 0.15;
        let target_x = self.base_height(line_x, z).max(floor);
        let target_z = self.base_height(x, line_z).max(floor);
        let target = (target_x * weight_x + target_z * weight_z) / total;
        lerp_f32(h, target, total.min(1.0))
    }

    /// The ground level of a town's plaza: the base field plus the road carve,
    /// never the town flattening, so there is no recursion.
    ///
    /// A site that would otherwise be under water is terraced up to just above
    /// the waterline: a town always exists at its lattice position, and a road
    /// crossing the disc stays walkable instead of being pulled under.
    fn plaza_ground(&self, cell: ChunkPos) -> f32 {
        let centre = town_centre(&self.config, cell);
        let raw = self.carve_roads(centre.x, centre.z, self.base_height(centre.x, centre.z));
        raw.max(self.config.sea_level + BEACH_HEIGHT * 0.5)
    }

    /// Flattens a town's disc towards its plaza height.
    fn flatten_towns(&self, x: f32, z: f32, h: f32) -> f32 {
        let cfg = &self.config;
        let cell = nearest_town_cell(cfg, x, z);
        let centre = town_centre(cfg, cell);
        let radius = cfg.town_radius_chunks.max(1) as f32 * cfg.chunk_world_size();
        let distance = Vec2::new(x - centre.x, z - centre.z).length();
        let weight = 1.0 - smoothstep(radius * TOWN_INNER_FLATTEN, radius, distance);
        if weight <= 0.0 {
            return h;
        }
        lerp_f32(h, self.plaza_ground(cell), weight)
    }

    // --- roads, towns ------------------------------------------------------

    /// The town lattice cell that covers `chunk`, if any.
    fn town_cell_for_chunk(&self, chunk: ChunkPos) -> Option<ChunkPos> {
        let spacing = self.config.town_spacing();
        let radius = self.config.town_radius_chunks.max(1);
        let i = round_div(chunk.x, spacing);
        let j = round_div(chunk.y, spacing);
        // The discs never overlap (radius < spacing / 2), so the nearest cell is
        // the only candidate; the neighbours are checked anyway so an unusual
        // configuration degrades to "no town" rather than to a wrong one.
        for (di, dj) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
            let cell = ChunkPos::new((i + di) * spacing, (j + dj) * spacing);
            if chunk.chebyshev_distance(cell) <= radius {
                return Some(cell);
            }
        }
        None
    }

    /// Builds the plan for the town at a lattice cell.
    fn plan_town(&self, cell: ChunkPos) -> TownPlan {
        TownPlan::generate(&self.config, cell, &self.prefabs, |x, z| {
            self.sample_height(x, z)
        })
    }

    /// True when a tile centre is paved.
    ///
    /// `plan` supplies the street grid when the caller already has it; when it
    /// is `None` the analytic grid is computed from the lattice, which is what
    /// [`WorldGenerator::is_road_at`] uses.
    fn road_band(&self, x: f32, z: f32, plan: Option<&TownPlan>) -> bool {
        if !x.is_finite() || !z.is_finite() {
            return false;
        }
        let (x, z) = (clamp_world(x), clamp_world(z));
        let cfg = &self.config;
        let ts = cfg.tile_size();
        let half = cfg.road_width_tiles.max(1) as f32 * ts * 0.5;
        let (line_x, line_z) = nearest_lines(cfg, Vec3::new(x, 0.0, z));
        if (x - line_x).abs() <= half || (z - line_z).abs() <= half {
            return true;
        }
        match plan {
            Some(plan) => plan
                .streets
                .iter()
                .any(|street| street.distance_to(Vec3::new(x, 0.0, z)) <= street.half_width()),
            None => {
                let cell = nearest_town_cell(cfg, x, z);
                town_streets(cfg, cell)
                    .iter()
                    .any(|street| street.distance_to(Vec3::new(x, 0.0, z)) <= street.half_width())
            }
        }
    }

    /// The road pieces crossing a chunk, clipped to it.
    fn chunk_roads(
        &self,
        pos: ChunkPos,
        plan: Option<&TownPlan>,
        footprint: &Aabb,
    ) -> Vec<RoadSegment> {
        let cfg = &self.config;
        let mut out = Vec::new();
        let network = RoadNetwork::generate(cfg, pos, pos);
        for segment in &network.segments {
            if let Some(piece) = segment.clip_to_aabb(footprint) {
                out.push(piece);
            }
        }
        if let Some(plan) = plan {
            for street in &plan.streets {
                if let Some(piece) = street.clip_to_aabb(footprint) {
                    out.push(piece);
                }
            }
        } else {
            let cell = nearest_town_cell(cfg, footprint.min.x, footprint.min.z);
            for street in town_streets(cfg, cell) {
                if let Some(piece) = street.clip_to_aabb(footprint) {
                    out.push(piece);
                }
            }
            // The connector can reach into a neighbouring chunk; check the cell
            // on the other corner too so a chunk straddling two cells sees it.
            let other = nearest_town_cell(cfg, footprint.max.x, footprint.max.z);
            if other != cell {
                for street in town_streets(cfg, other) {
                    if let Some(piece) = street.clip_to_aabb(footprint) {
                        out.push(piece);
                    }
                }
            }
        }
        out
    }

    // --- tiles -------------------------------------------------------------

    /// Chooses the tile for one tile centre.
    ///
    /// The order is water, road, building floor, rock, beach, accent, base: a
    /// road survives a shoreline, and a building's interior floor only appears
    /// where nothing more important already is.
    fn tile_for(
        &self,
        x: f32,
        z: f32,
        height: f32,
        slope: f32,
        biome: BiomeId,
        on_road: bool,
        in_building: bool,
    ) -> u32 {
        let index = biome.index();
        if height < self.config.sea_level {
            return self.slots.biome_water[index].unwrap_or(self.slots.water);
        }
        if on_road {
            return self.slots.road;
        }
        if in_building {
            return self.slots.floor;
        }
        if slope > ROCK_SLOPE {
            return self.slots.rock;
        }
        if height < self.config.sea_level + BEACH_HEIGHT {
            return self.slots.sand;
        }
        if self.is_accent(x, z) {
            return self.slots.accent[index];
        }
        self.slots.base[index]
    }

    /// True where the accent tile should show through.
    ///
    /// Two scales: a broad patch mask so accents clump into meadows rather than
    /// speckling uniformly, and a fine mask for the detail.
    fn is_accent(&self, x: f32, z: f32) -> bool {
        let seed = self.config.seed;
        let x = clamp_world(x);
        let z = clamp_world(z);
        let patch = value_2d(x * 0.09, z * 0.09, seed ^ 0x00AC_0000_0000_0006);
        if patch < 0.05 {
            return false;
        }
        value_2d(x * 0.45, z * 0.45, seed ^ 0x00AC_0000_0000_0007) > 0.30
    }

    // --- chunk generation --------------------------------------------------

    /// The chunk plus the counters the public entry point wants.
    fn build_chunk(&self, pos: ChunkPos) -> PendingChunk {
        let cfg = &self.config;
        let ts = cfg.tile_size();
        let side = cfg.safe_tiles_per_chunk();
        let centre = pos.to_world_center(cfg.chunk_world_size(), cfg.sea_level);
        let centre_biome = self.biome_at(centre.x, centre.z);
        if cfg.tiles_per_chunk == 0 {
            return PendingChunk {
                chunk: Chunk::empty(pos, 0, ts, centre_biome),
                road_tiles: 0,
            };
        }
        let side_usize = side as usize;
        let origin_x = pos.x as f32 * cfg.chunk_world_size();
        let origin_z = pos.y as f32 * cfg.chunk_world_size();
        // `from_footprint` takes (base_y, height): the chunk's XZ quad extruded
        // from far below to far above, so it is the XZ footprint that decides
        // every overlap test.
        let footprint = Aabb::from_footprint(
            origin_x,
            origin_z,
            origin_x + side as f32 * ts,
            origin_z + side as f32 * ts,
            -1000.0,
            2000.0,
        );

        // 1. Terrain on an (n+2)² halo, so slopes at the chunk edge are central
        //    differences like everywhere else.
        let halo = side_usize + 2;
        let mut heights = vec![0.0f32; halo * halo];
        for gy in 0..halo {
            for gx in 0..halo {
                let wx = origin_x + (gx as f32 - 0.5) * ts;
                let wz = origin_z + (gy as f32 - 0.5) * ts;
                heights[gy * halo + gx] = self.sample_height(wx, wz);
            }
        }

        // 2. Slopes and biomes for the chunk's own tiles.
        let count = side_usize * side_usize;
        let mut slopes = vec![0.0f32; count];
        let mut biomes = vec![BiomeId::PLAINS; count];
        for ty in 0..side_usize {
            for tx in 0..side_usize {
                let i = ty * side_usize + tx;
                let gx = tx + 1;
                let gy = ty + 1;
                let dx = (heights[gy * halo + gx + 1] - heights[gy * halo + gx - 1]) / (2.0 * ts);
                let dz =
                    (heights[(gy + 1) * halo + gx] - heights[(gy - 1) * halo + gx]) / (2.0 * ts);
                slopes[i] = if dx.is_finite() && dz.is_finite() {
                    (dx * dx + dz * dz).sqrt()
                } else {
                    0.0
                };
                let wx = origin_x + (tx as f32 + 0.5) * ts;
                let wz = origin_z + (ty as f32 + 0.5) * ts;
                biomes[i] = self.biome_for(wx, wz, heights[gy * halo + gx]);
            }
        }

        // 3. The town that touches this chunk, if any.
        let plan = self
            .town_cell_for_chunk(pos)
            .map(|cell| self.plan_town(cell));
        let mut buildings: Vec<BuildingInstance> = Vec::new();
        if let Some(plan) = &plan {
            for building in plan.instantiate(cfg, &self.prefabs, |x, z| self.sample_height(x, z)) {
                if building.bounds.intersects(&footprint) {
                    buildings.push(building);
                }
            }
        }

        // 4. Tiles.
        let mut tiles = vec![0u32; count];
        let mut road_tiles = 0u64;
        for ty in 0..side_usize {
            for tx in 0..side_usize {
                let i = ty * side_usize + tx;
                let gx = tx + 1;
                let gy = ty + 1;
                let wx = origin_x + (tx as f32 + 0.5) * ts;
                let wz = origin_z + (ty as f32 + 0.5) * ts;
                let height = heights[gy * halo + gx];
                let on_road = self.road_band(wx, wz, plan.as_ref());
                if on_road {
                    road_tiles += 1;
                }
                let point = Vec3::new(wx, height, wz);
                let in_building = buildings.iter().any(|b| b.contains_xz(point));
                tiles[i] =
                    self.tile_for(wx, wz, height, slopes[i], biomes[i], on_road, in_building);
            }
        }

        // 5. Props.
        let mut props = Vec::new();
        if cfg.generate_props {
            let field = PropField {
                origin_x,
                origin_z,
                ts,
                side: side_usize,
                halo,
                heights: &heights,
                slopes: &slopes,
                biomes: &biomes,
                tiles: &tiles,
                buildings: &buildings,
                plan: plan.as_ref(),
            };
            self.scatter_props(pos, &field, &mut props);
        }

        // 6. Colliders, roads and the final chunk.
        let mut colliders: Vec<(Aabb, u64)> = Vec::new();
        for building in &buildings {
            colliders.extend(building.colliders_in(&footprint));
        }
        for prop in &props {
            if let Some(bounds) = prop.collider_bounds() {
                if let Some(clipped) = clip_xz(&bounds, &footprint) {
                    colliders.push((clipped, prop.id()));
                }
            }
        }
        let roads = self.chunk_roads(pos, plan.as_ref(), &footprint);

        let inner_heights = (0..count)
            .map(|i| {
                let tx = i % side_usize;
                let ty = i / side_usize;
                heights[(ty + 1) * halo + tx + 1]
            })
            .collect();

        PendingChunk {
            chunk: Chunk {
                pos,
                tiles,
                heights: inner_heights,
                slopes,
                biome: centre_biome,
                colliders,
                props,
                buildings,
                roads,
                has_town: plan.is_some(),
                generated_ms: 0.0,
                tile_size: ts,
            },
            road_tiles,
        }
    }

    /// Scatters trees and rocks over a chunk's tiles.
    fn scatter_props(&self, pos: ChunkPos, field: &PropField<'_>, out: &mut Vec<PropInstance>) {
        let cfg = &self.config;
        let density = cfg.prop_density;
        if !density.is_finite() || density <= 0.0 {
            return;
        }
        let mut rng = RngStream::for_chunk(cfg.seed, "chunk/props", pos.x, pos.y).rng();
        let mut taken: Vec<(i32, i32)> = Vec::new();
        for ty in 0..field.side {
            for tx in 0..field.side {
                let i = ty * field.side + tx;
                let roll = rng.next_f32();
                let tree_p =
                    (density * self.biomes.tree_density(field.biomes[i]) * PROP_DENSITY_SCALE)
                        .clamp(0.0, 1.0);
                let rock_p =
                    (density * self.biomes.rock_density(field.biomes[i]) * PROP_DENSITY_SCALE)
                        .clamp(0.0, 1.0);
                let kind = if roll < tree_p {
                    "tree"
                } else if roll < tree_p + rock_p {
                    "rock"
                } else {
                    continue;
                };
                let height = field.heights[(ty + 1) * field.halo + tx + 1];
                if height < cfg.sea_level || field.slopes[i] > MAX_PROP_SLOPE {
                    continue;
                }
                let wx = field.origin_x + (tx as f32 + 0.5) * field.ts;
                let wz = field.origin_z + (ty as f32 + 0.5) * field.ts;
                let point = Vec3::new(wx, height, wz);
                let on_road =
                    field.tiles[i] == self.slots.road || self.road_band(wx, wz, field.plan);
                if on_road || field.tiles[i] == self.slots.water {
                    continue;
                }
                if field.buildings.iter().any(|b| b.contains_xz(point)) {
                    continue;
                }
                // Two metres of spacing keeps a forest from becoming a solid
                // wall of overlapping canopies.
                let cell = ((wx / 2.0).floor() as i32, (wz / 2.0).floor() as i32);
                if taken.contains(&cell) {
                    continue;
                }
                taken.push(cell);
                let yaw = rng.range_f32(0.0, core::f32::consts::TAU);
                let scale = rng.range_f32(0.85, 1.2);
                out.push(PropInstance::new(kind, point, yaw, scale));
            }
        }
    }
}

/// The per-chunk arrays the prop scatter reads.
struct PropField<'a> {
    origin_x: f32,
    origin_z: f32,
    ts: f32,
    side: usize,
    halo: usize,
    heights: &'a [f32],
    slopes: &'a [f32],
    biomes: &'a [BiomeId],
    tiles: &'a [u32],
    buildings: &'a [BuildingInstance],
    plan: Option<&'a TownPlan>,
}

/// A chunk plus the numbers the statistics need.
struct PendingChunk {
    chunk: Chunk,
    road_tiles: u64,
}

/// Linear interpolation, shared with the town layout.
#[must_use]
pub(crate) fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `value / divisor`, rounded to nearest, guarding degenerate divisors.
#[must_use]
fn round_div(value: i32, divisor: i32) -> i32 {
    if divisor <= 0 {
        return 0;
    }
    let q = value / divisor;
    let r = value % divisor;
    if r.abs() * 2 >= divisor {
        q + r.signum()
    } else {
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::BiomeId;
    use crate::road::macro_line_x;
    use noxel_asset::format::TileDef;

    fn set() -> Arc<TileSet> {
        let mut tiles = Vec::new();
        let names = [
            "grass",
            "grass_dark",
            "grass_tuft",
            "grass_rocky",
            "moss",
            "sand",
            "sand_rock",
            "snow",
            "ice",
            "marsh",
            "reeds",
            "stone",
            "scree",
            "water",
            "water_deep",
            "road",
            "dirt",
            "floor",
        ];
        for (i, name) in names.iter().enumerate() {
            tiles.push(TileDef {
                id: i as u32,
                name: (*name).to_string(),
                texture: String::new(),
                uv: [0, 0, 16, 16],
                flags: TileFlags {
                    walkable: !matches!(*name, "water" | "water_deep"),
                    blocks_sight: matches!(*name, "stone" | "scree"),
                    occluder: false,
                    water: matches!(*name, "water" | "water_deep"),
                    road: *name == "road",
                    buildable: matches!(*name, "sand" | "dirt"),
                },
                height: 0.0,
                layer: 0,
            });
        }
        Arc::new(TileSet {
            name: "ground".into(),
            texture: "ground.png".into(),
            tile_size: 16,
            tiles,
        })
    }

    fn generator(seed: u64) -> WorldGenerator {
        WorldGenerator::new(WorldConfig::new(seed), set(), Vec::new())
    }

    #[test]
    fn defaults_match_the_contract() {
        let c = WorldConfig::default();
        assert_eq!(c.tile_size, 1.0);
        assert_eq!(c.tiles_per_chunk, 32);
        assert_eq!(c.chunk_world_size, 32.0);
        assert_eq!(c.chunk_world_size(), 32.0);
        assert_eq!(c.view_distance_chunks, 6);
        assert_eq!(c.keep_distance_chunks, 8);
        assert_eq!(c.max_cached_chunks, 256);
        assert_eq!(c.sea_level, 0.0);
        assert_eq!(c.height_scale, 8.0);
        assert_eq!(c.road_grid_chunks, 8);
        assert_eq!(c.road_jitter, 0.25);
        assert_eq!(c.road_width_tiles, 3);
        assert_eq!(c.town_spacing_chunks, 10);
        assert_eq!(c.town_radius_chunks, 3);
        assert_eq!(c.buildings_per_town, 18);
        assert!(c.generate_props);
        assert_eq!(c.prop_density, 0.02);
        assert_eq!(WorldConfig::default().seed, 0);
        assert_eq!(WorldConfig::new(9).seed, 9);
    }

    #[test]
    fn sync_sizes_rebuilds_the_chunk_size() {
        let mut c = WorldConfig::new(1);
        c.tiles_per_chunk = 16;
        c.tile_size = 2.0;
        c.sync_sizes();
        assert_eq!(c.chunk_world_size, 32.0);
        assert_eq!(c.chunk_world_size(), 32.0);
    }

    #[test]
    fn degenerate_configs_are_sanitised() {
        let mut c = WorldConfig::new(1);
        c.chunk_world_size = 0.0;
        assert!(c.chunk_world_size() > 0.0);
        c.chunk_world_size = f32::NAN;
        assert!(c.chunk_world_size() > 0.0);
        c.tile_size = 0.0;
        assert_eq!(c.tile_size(), 1.0);
        c.tiles_per_chunk = u32::MAX;
        assert_eq!(c.safe_tiles_per_chunk(), MAX_TILES_PER_CHUNK);
        c.tiles_per_chunk = 0;
        assert_eq!(c.safe_tiles_per_chunk(), 1);
        c.road_grid_chunks = 0;
        assert!(c.road_spacing() > 0.0);
        c.town_spacing_chunks = 0;
        assert_eq!(c.town_spacing(), 1);
    }

    #[test]
    fn samples_are_finite_and_deterministic() {
        let g = generator(5);
        for (x, z) in [(0.0, 0.0), (123.5, -77.25), (9000.0, -4000.0)] {
            let h = g.sample_height(x, z);
            assert!(h.is_finite(), "{x},{z} -> {h}");
            assert_eq!(h, g.sample_height(x, z));
            assert!(g.sample_slope(x, z).is_finite());
        }
        assert_eq!(
            generator(5).sample_height(12.0, 34.0),
            g.sample_height(12.0, 34.0)
        );
        assert_ne!(
            generator(6).sample_height(12.0, 34.0),
            g.sample_height(12.0, 34.0)
        );
    }

    #[test]
    fn non_finite_queries_do_not_panic() {
        let g = generator(3);
        assert_eq!(g.sample_height(f32::NAN, 0.0), 0.0);
        assert_eq!(g.sample_slope(f32::INFINITY, 0.0), 0.0);
        assert_eq!(g.biome_at(f32::NAN, f32::NAN), BiomeId::PLAINS);
        assert!(!g.is_road_at(f32::NAN, 0.0));
    }

    #[test]
    fn generate_chunk_has_the_documented_shape() {
        let g = generator(11);
        let chunk = g.generate_chunk(ChunkPos::new(2, -3));
        assert_eq!(chunk.pos, ChunkPos::new(2, -3));
        assert_eq!(chunk.tiles.len(), 32 * 32);
        assert_eq!(chunk.heights.len(), 32 * 32);
        assert_eq!(chunk.slopes.len(), 32 * 32);
        assert_eq!(chunk.tile_size, 1.0);
        assert!(chunk.generated_ms >= 0.0);
        assert!(chunk.bounds().is_finite());
    }

    #[test]
    fn chunk_heights_match_sample_height_at_tile_centres() {
        let g = generator(21);
        for pos in [ChunkPos::new(0, 0), ChunkPos::new(-4, 5)] {
            let chunk = g.generate_chunk(pos);
            let cs = g.config().chunk_world_size();
            for y in 0..chunk.tiles() {
                for x in 0..chunk.tiles() {
                    let wx = pos.x as f32 * cs + (x as f32 + 0.5) * chunk.tile_size;
                    let wz = pos.y as f32 * cs + (y as f32 + 0.5) * chunk.tile_size;
                    let expected = g.sample_height(wx, wz);
                    assert!(
                        (chunk.height(x, y) - expected).abs() < 1e-3,
                        "tile {x},{y}: {} vs {expected}",
                        chunk.height(x, y)
                    );
                }
            }
        }
    }

    #[test]
    fn every_tile_id_resolves_in_a_rich_tile_set() {
        let g = generator(2);
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        for y in 0..chunk.tiles() {
            for x in 0..chunk.tiles() {
                assert!(
                    g.tile_set().tile(chunk.tile(x, y)).is_some(),
                    "tile id {} is not in the set",
                    chunk.tile(x, y)
                );
            }
        }
    }

    #[test]
    fn empty_tile_set_falls_back_to_zero() {
        let g = WorldGenerator::new(
            WorldConfig::new(2),
            Arc::new(TileSet {
                name: "empty".into(),
                texture: String::new(),
                tile_size: 16,
                tiles: Vec::new(),
            }),
            Vec::new(),
        );
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        assert!(chunk.tiles.iter().all(|t| *t == 0));
        assert!(chunk.bounds().is_finite());
        assert!(g.sample_height(3.0, 4.0).is_finite());
    }

    #[test]
    fn a_tile_set_missing_every_named_tile_still_generates() {
        let g = WorldGenerator::new(
            WorldConfig::new(4),
            Arc::new(TileSet {
                name: "odd".into(),
                texture: String::new(),
                tile_size: 16,
                tiles: vec![TileDef {
                    id: 77,
                    name: "unobtainium".into(),
                    texture: String::new(),
                    uv: [0, 0, 4, 4],
                    flags: TileFlags {
                        walkable: true,
                        ..TileFlags::default()
                    },
                    height: 0.0,
                    layer: 0,
                }],
            }),
            Vec::new(),
        );
        let chunk = g.generate_chunk(ChunkPos::new(1, 1));
        assert!(chunk.tiles.iter().all(|t| *t == 77));
    }

    #[test]
    fn zero_sized_tile_set_does_not_panic() {
        let g = WorldGenerator::new(
            WorldConfig::new(0),
            Arc::new(TileSet {
                name: String::new(),
                texture: String::new(),
                tile_size: 0,
                tiles: Vec::new(),
            }),
            Vec::new(),
        );
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        assert_eq!(chunk.tiles.len(), 32 * 32);
        assert!(!chunk.is_walkable(0, 0, g.tile_set()));
    }

    #[test]
    fn zero_tiles_per_chunk_generates_an_empty_chunk() {
        let mut c = WorldConfig::new(1);
        c.tiles_per_chunk = 0;
        let g = WorldGenerator::new(c, set(), Vec::new());
        let chunk = g.generate_chunk(ChunkPos::new(3, 3));
        assert_eq!(chunk.tiles(), 0);
        assert!(chunk.bounds().is_finite());
    }

    #[test]
    fn tiles_per_chunk_is_clamped() {
        let mut c = WorldConfig::new(1);
        c.tiles_per_chunk = u32::MAX;
        c.sync_sizes();
        let g = WorldGenerator::new(c, set(), Vec::new());
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        assert_eq!(chunk.tiles(), MAX_TILES_PER_CHUNK);
    }

    #[test]
    fn stats_accumulate() {
        let g = generator(1);
        let before = g.stats();
        assert_eq!(before.chunks_generated, 0);
        assert_eq!(before.mean_ms, 0.0);
        let _ = g.generate_chunk(ChunkPos::new(0, 0));
        let _ = g.generate_chunk(ChunkPos::new(1, 0));
        let after = g.stats();
        assert_eq!(after.chunks_generated, 2);
        assert_eq!(after.tiles_written, 2 * 32 * 32);
        assert!(after.last_ms >= 0.0);
        assert!(after.mean_ms >= 0.0);
        assert!(after.props_placed > 0, "the default world has props");
    }

    #[test]
    fn water_is_below_sea_level_and_land_is_not() {
        let g = generator(17);
        let mut water = 0;
        let mut land = 0;
        for i in 0..200 {
            for j in 0..200 {
                let x = i as f32 * 7.0 - 700.0;
                let z = j as f32 * 7.0 - 700.0;
                let h = g.sample_height(x, z);
                if h < g.config().sea_level {
                    water += 1;
                } else {
                    land += 1;
                }
                if h < g.config().sea_level {
                    assert_eq!(g.biome_at(x, z), BiomeId::LAKE);
                }
            }
        }
        assert!(water > 100, "expected real water, got {water}");
        assert!(land > 100, "expected real land, got {land}");
    }

    #[test]
    fn mountains_are_the_highest_biome() {
        let g = generator(23);
        let mut best = std::collections::HashMap::new();
        for i in 0..160 {
            for j in 0..160 {
                let x = i as f32 * 11.0 - 880.0;
                let z = j as f32 * 11.0 - 880.0;
                let h = g.sample_height(x, z);
                let b = g.biome_at(x, z);
                let entry = best.entry(b).or_insert(f32::NEG_INFINITY);
                if h > *entry {
                    *entry = h;
                }
            }
        }
        let mountains = *best.get(&BiomeId::MOUNTAINS).unwrap_or(&f32::NEG_INFINITY);
        if mountains.is_finite() {
            for (biome, top) in &best {
                if *biome == BiomeId::MOUNTAINS {
                    continue;
                }
                assert!(
                    *top <= mountains + 1e-3,
                    "{biome:?} at {top} beats mountains at {mountains}"
                );
            }
        }
    }

    #[test]
    fn roads_are_stamped_and_are_not_props() {
        let g = generator(31);
        let config = g.config().clone();
        // Find a chunk the lattice runs through.
        let cell = config.chunk_world_size() * config.road_grid_chunks as f32;
        let pos = ChunkPos::new((cell / config.chunk_world_size()) as i32, 0);
        let chunk = g.generate_chunk(pos);
        assert!(chunk.roads.len() > 0 || chunk.tiles.iter().any(|t| *t == 15));
        for prop in &chunk.props {
            assert!(!g.is_road_at(prop.position.x, prop.position.z));
        }
    }

    /// The largest change produced by walking `step` metres along a set of
    /// scan lines, plus the largest slope the field reports along the way.
    fn walk_delta(g: &WorldGenerator, step: f32) -> (f32, f32) {
        let mut worst: f32 = 0.0;
        let mut worst_slope: f32 = 0.0;
        for k in 0..12 {
            let z = k as f32 * 37.0 - 200.0;
            let mut previous = g.sample_height(0.0, z);
            let mut x = step;
            while x < 60.0 {
                let h = g.sample_height(x, z);
                worst = worst.max((h - previous).abs());
                worst_slope = worst_slope.max(g.sample_slope(x, z));
                previous = h;
                x += step;
            }
        }
        (worst, worst_slope)
    }

    #[test]
    fn heights_are_continuous_over_a_small_step() {
        let g = generator(41);
        let (worst, slack) = walk_delta(&g, 0.1);
        // A genuine discontinuity — a chunk seam, a mis-blended road shoulder —
        // shows up as a step of metres. 5 m/m is far above anything the noise
        // composition can produce and far below a real break.
        let absolute = 0.5;
        assert!(
            worst <= absolute,
            "a 0.1 m step moved the surface {worst} m"
        );
        assert!(slack > 0.0, "the field must not be flat");

        // Derived check: halving the step must roughly halve the largest change,
        // which is what "continuous" means. A jump would keep it constant.
        let (coarse, _) = walk_delta(&g, 0.4);
        let (half, _) = walk_delta(&g, 0.2);
        let (quarter, _) = walk_delta(&g, 0.1);
        assert!(half < coarse * 0.75, "{coarse} -> {half}");
        assert!(quarter < half * 0.75, "{half} -> {quarter}");
    }

    #[test]
    fn slope_is_smaller_than_the_step_bound() {
        let g = generator(43);
        let mut max = 0.0f32;
        for i in 0..400 {
            let x = i as f32 * 3.0 - 600.0;
            max = max.max(g.sample_slope(x, 12.0));
        }
        assert!(max < 6.0, "slope {max} suggests a discontinuity");
    }

    #[test]
    fn roads_never_sit_below_the_waterline() {
        let g = generator(51);
        let config = g.config();
        let spacing = config.road_spacing();
        for i in -3..3 {
            let x = macro_line_x(config, i);
            for k in 0..30 {
                let z = k as f32 * 21.0;
                let h = g.sample_height(x, z);
                assert!(
                    h >= config.sea_level,
                    "road at {x},{z} is at {h}, below sea level"
                );
                assert!(g.is_road_at(x, z));
            }
        }
        assert!(spacing > 0.0);
    }

    #[test]
    fn town_lattice_positions_have_towns_and_the_gaps_do_not() {
        let g = generator(61);
        let spacing = g.config().town_spacing();
        let town = g.town_at(ChunkPos::new(spacing, -spacing));
        assert!(town.is_some(), "a town must exist at a lattice position");
        let plan = town.unwrap();
        assert_eq!(plan.center_chunk, ChunkPos::new(spacing, -spacing));
        assert_eq!(plan.radius_chunks, g.config().town_radius_chunks);
        // A chunk halfway between two towns is not part of one.
        let gap = ChunkPos::new(spacing + spacing / 2, spacing / 2);
        assert!(g.town_at(gap).is_none(), "{gap:?} should be wilderness");
    }

    #[test]
    fn town_chunks_are_flat_and_flagged() {
        let g = generator(71);
        let spacing = g.config().town_spacing();
        let pos = ChunkPos::new(spacing, spacing);
        let chunk = g.generate_chunk(pos);
        assert!(chunk.has_town);
        assert!(!chunk.buildings.is_empty(), "a town must have buildings");
        assert!(!chunk.colliders.is_empty(), "buildings are solid");
        let centre = g.town_at(pos).unwrap().plaza_center;
        let plaza_h = g.sample_height(centre.x, centre.z);
        assert!((chunk.height(chunk.tiles() / 2, chunk.tiles() / 2) - plaza_h).abs() < 2.0);
        // The plaza is genuinely flat.
        assert!(g.sample_slope(centre.x, centre.z) < 0.05);
    }

    #[test]
    fn props_respect_their_rejection_rules() {
        let g = generator(81);
        let mut checked = 0;
        for i in -2..3 {
            for j in -2..3 {
                let chunk = g.generate_chunk(ChunkPos::new(i, j));
                for prop in &chunk.props {
                    let p = prop.position;
                    assert!(p.y >= g.config().sea_level, "prop in water at {p:?}");
                    assert!(!g.is_road_at(p.x, p.z), "prop on a road at {p:?}");
                    let (tx, ty) = chunk.world_to_tile(p).expect("prop is on the chunk");
                    assert!(
                        chunk.slope(tx, ty) <= MAX_PROP_SLOPE,
                        "prop on a slope at {p:?}"
                    );
                    for building in &chunk.buildings {
                        assert!(!building.contains_xz(p), "prop inside a building");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 20, "expected props to place, got {checked}");
    }

    #[test]
    fn chunk_roads_are_clipped_pieces_of_the_lattice() {
        let g = generator(91);
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        let bounds = chunk.bounds();
        for road in &chunk.roads {
            assert!(road.aabb(0.0).intersects(&bounds));
            assert!(road.length() <= g.config().chunk_world_size() * 2.0);
        }
    }

    #[test]
    fn colliders_have_stable_ids_and_finite_bounds() {
        let g = generator(101);
        let a = g.generate_chunk(ChunkPos::new(0, 0));
        let b = g.generate_chunk(ChunkPos::new(0, 0));
        assert_eq!(a.colliders.len(), b.colliders.len());
        for ((ba, ia), (bb, ib)) in a.colliders.iter().zip(b.colliders.iter()) {
            assert_eq!(ia, ib);
            assert_eq!(ba, bb);
            assert!(ba.is_finite());
            assert!(!ba.is_empty());
        }
    }

    #[test]
    fn extreme_coordinates_are_clamped_not_panicked() {
        let g = generator(3);
        for (x, z) in [(1e30, -1e30), (f32::MAX, f32::MIN), (1e9, 1e9)] {
            assert!(g.sample_height(x, z).is_finite());
            assert!(g.sample_slope(x, z).is_finite());
            let _ = g.biome_at(x, z);
            let _ = g.is_road_at(x, z);
        }
        assert_eq!(
            g.sample_height(1e30, 40.0),
            g.sample_height(MAX_WORLD_COORDINATE, 40.0)
        );
    }

    #[test]
    fn round_div_handles_negatives() {
        assert_eq!(round_div(0, 10), 0);
        assert_eq!(round_div(4, 10), 0);
        assert_eq!(round_div(5, 10), 1);
        assert_eq!(round_div(-5, 10), -1);
        assert_eq!(round_div(-4, 10), 0);
        assert_eq!(round_div(10, 0), 0);
    }
}
