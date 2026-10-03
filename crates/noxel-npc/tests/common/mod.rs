//! Shared test worlds.
//!
//! The crowd cannot be tested against a bare world: the generator's tiles are
//! only walkable when the tile set says so, and `TileSet` lives in
//! `noxel-asset`. This module is the one place the tests build a real tile set
//! (grass, water, road, stone, building floor, sand) and a real streamed world,
//! so every other test file can ask for "a town at seed 7" in one line.

#![allow(dead_code)]

use std::sync::Arc;

use noxel_asset::format::{TileDef, TileFlags, TileSet};
use noxel_core::math::{Aabb, ChunkPos, Vec3};
use noxel_npc::path::SHALLOW_WATER_DEPTH;
use noxel_physics::{PhysicsConfig, PhysicsWorld};
use noxel_world::stream::WorldStreamer;
use noxel_world::town;
use noxel_world::{WorldConfig, WorldGenerator};

/// Test tile ids, resolved by the generator by name.
pub const GRASS: u32 = 0;
/// Deep or shallow water: not walkable, carries the `water` flag.
pub const WATER: u32 = 1;
/// Paved: walkable and flagged as a road.
pub const ROAD: u32 = 2;
/// Rock: not walkable, blocks sight.
pub const STONE: u32 = 3;
/// A building's indoor floor: walkable, no road.
pub const FLOOR: u32 = 4;
/// Beach: walkable and buildable.
pub const SAND: u32 = 5;

/// Builds a tile definition.
fn tile(id: u32, name: &str, flags: TileFlags) -> TileDef {
    TileDef {
        id,
        name: name.to_string(),
        texture: String::new(),
        uv: [0, 0, 16, 16],
        flags,
        height: 0.0,
        layer: 0,
    }
}

/// A tile set with every tile the generator looks for by name.
pub fn tile_set() -> Arc<TileSet> {
    Arc::new(TileSet {
        name: "test-ground".into(),
        texture: String::new(),
        tile_size: 16,
        tiles: vec![
            tile(
                GRASS,
                "grass",
                TileFlags {
                    walkable: true,
                    buildable: true,
                    ..TileFlags::default()
                },
            ),
            tile(
                WATER,
                "water",
                TileFlags {
                    walkable: false,
                    water: true,
                    ..TileFlags::default()
                },
            ),
            tile(
                ROAD,
                "road",
                TileFlags {
                    walkable: true,
                    road: true,
                    buildable: true,
                    ..TileFlags::default()
                },
            ),
            tile(
                STONE,
                "stone",
                TileFlags {
                    walkable: false,
                    blocks_sight: true,
                    ..TileFlags::default()
                },
            ),
            tile(
                FLOOR,
                "floor",
                TileFlags {
                    walkable: true,
                    ..TileFlags::default()
                },
            ),
            tile(
                SAND,
                "sand",
                TileFlags {
                    walkable: true,
                    buildable: true,
                    ..TileFlags::default()
                },
            ),
        ],
    })
}

/// A small, dense test world: towns every four chunks, roads every three.
pub fn world_config(seed: u64) -> WorldConfig {
    let mut config = WorldConfig::new(seed);
    config.view_distance_chunks = 3;
    config.keep_distance_chunks = 5;
    config.max_cached_chunks = 256;
    config.town_spacing_chunks = 4;
    config.town_radius_chunks = 2;
    config.road_grid_chunks = 3;
    config.buildings_per_town = 14;
    config.prop_density = 0.01;
    config
}

/// A streamer whose world configuration has been adjusted by `tweak`.
pub fn streamer_with(
    seed: u64,
    center: Vec3,
    tweak: impl FnOnce(&mut WorldConfig),
) -> WorldStreamer {
    let mut config = world_config(seed);
    tweak(&mut config);
    let mut streamer = WorldStreamer::new(WorldGenerator::new(config, tile_set(), Vec::new()));
    streamer.update(center);
    streamer
}

/// A streamer with nothing resident.
pub fn streamer(seed: u64) -> WorldStreamer {
    WorldStreamer::new(WorldGenerator::new(
        world_config(seed),
        tile_set(),
        Vec::new(),
    ))
}

/// A streamer with the world around `center` resident.
pub fn streamer_at(seed: u64, center: Vec3) -> WorldStreamer {
    let mut streamer = streamer(seed);
    streamer.update(center);
    streamer
}

/// A streamer centred on a town's plaza, so tests have buildings and streets.
pub fn town_streamer(seed: u64) -> (WorldStreamer, Vec3) {
    let config = world_config(seed);
    let cell = ChunkPos::new(config.town_spacing(), config.town_spacing());
    let mut streamer = streamer(seed);
    let centre = find_walkable(&streamer, town::town_centre(&config, cell), 96.0)
        .unwrap_or_else(|| town::town_centre(&config, cell));
    streamer.update(centre);
    let centre = find_walkable(&streamer, centre, 48.0).unwrap_or(centre);
    (streamer, centre)
}

/// A physics world with no geometry.
pub fn physics() -> PhysicsWorld {
    PhysicsWorld::new(PhysicsConfig::default())
}

/// A physics world with a flat floor under `center`.
///
/// The crowd's tier-0 agents are moved by the character controller, so a test
/// that wants them to walk needs something to stand on. The floor is a slab
/// whose top face sits at the terrain height, which is enough for behaviour
/// that does not depend on the terrain's shape.
pub fn physics_with_floor(center: Vec3, height: f32, radius: f32) -> PhysicsWorld {
    let mut world = physics();
    world.insert_static_aabb(
        Aabb::new(
            Vec3::new(center.x - radius, height - 4.0, center.z - radius),
            Vec3::new(center.x + radius, height, center.z + radius),
        ),
        0,
    );
    world
}

/// The nearest standable position to `preferred` within `radius`.
pub fn find_walkable(streamer: &WorldStreamer, preferred: Vec3, radius: f32) -> Option<Vec3> {
    if let Some(point) = ground_at(streamer, preferred) {
        return Some(point);
    }
    let steps = 128;
    for step in 0..steps {
        let t = step as f32 / steps as f32;
        let distance = radius * (0.05 + 0.95 * t);
        let angle = step as f32 * 2.399_963;
        let candidate = Vec3::new(
            preferred.x + angle.cos() * distance,
            0.0,
            preferred.z + angle.sin() * distance,
        );
        if let Some(point) = ground_at(streamer, candidate) {
            return Some(point);
        }
    }
    None
}

/// The ground position at `p` when the streamer considers it walkable.
pub fn ground_at(streamer: &WorldStreamer, p: Vec3) -> Option<Vec3> {
    if !p.is_finite() || streamer.chunk_at(p).is_none() {
        return None;
    }
    let ground = Vec3::new(p.x, streamer.height_at(p), p.z);
    if streamer.is_walkable(ground) {
        Some(ground)
    } else {
        None
    }
}

/// A flat, open, walkable area inside the loaded region.
///
/// "Flat" is a 16 m square whose height varies by less than 40 cm, which is
/// what the tests that need agents to actually walk somewhere use.
pub fn flat_area(streamer: &WorldStreamer) -> Option<Vec3> {
    let positions = streamer.loaded_positions();
    let first = *positions.first()?;
    let size = streamer.generator().config().chunk_world_size();
    let origin = Vec3::new(first.x as f32 * size, 0.0, first.y as f32 * size);
    // Stay a chunk inside the loaded border so a path never runs off the edge.
    let span = streamer.generator().config().view_distance_chunks.max(1) - 1;
    if span < 1 {
        return find_walkable(streamer, origin, 32.0);
    }
    let extent = span as f32 * size;
    let mut best: Option<(Vec3, f32)> = None;
    let mut z = 1.0;
    while z < extent {
        let mut x = 1.0;
        while x < extent {
            let candidate = Vec3::new(origin.x + x, 0.0, origin.z + z);
            if let Some(ground) = ground_at(streamer, candidate) {
                let mut low = f32::MAX;
                let mut high = f32::MIN;
                let mut open = true;
                for offset in [
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(8.0, 0.0, 0.0),
                    Vec3::new(-8.0, 0.0, 0.0),
                    Vec3::new(0.0, 0.0, 8.0),
                    Vec3::new(0.0, 0.0, -8.0),
                    Vec3::new(8.0, 0.0, 8.0),
                    Vec3::new(-8.0, 0.0, -8.0),
                ] {
                    match ground_at(streamer, candidate + offset) {
                        Some(point) => {
                            low = low.min(point.y);
                            high = high.max(point.y);
                        }
                        None => {
                            open = false;
                            break;
                        }
                    }
                }
                if open && high - low < 0.4 {
                    let flatness = high - low;
                    if best.is_none_or(|(_, b)| flatness < b) {
                        best = Some((ground, flatness));
                    }
                }
            }
            x += 4.0;
        }
        z += 4.0;
    }
    best.map(|(point, _)| point)
        .or_else(|| find_walkable(streamer, origin, extent))
}

/// A position in water deep enough that no agent may cross it.
pub fn deep_water(streamer: &WorldStreamer) -> Option<Vec3> {
    let positions = streamer.loaded_positions();
    let first = *positions.first()?;
    let size = streamer.generator().config().chunk_world_size();
    let sea_level = streamer.generator().config().sea_level;
    let set = streamer.generator().tile_set();
    let origin = Vec3::new(first.x as f32 * size, 0.0, first.y as f32 * size);
    let mut z = 0.5;
    while z < size * 3.0 {
        let mut x = 0.5;
        while x < size * 3.0 {
            let candidate = Vec3::new(origin.x + x, 0.0, origin.z + z);
            if let Some(chunk) = streamer.chunk_at(candidate)
                && let Some((tx, ty)) = chunk.world_to_tile(candidate)
            {
                let water = chunk
                    .tile_def(tx, ty, set)
                    .is_some_and(|def| def.flags.water);
                let depth = sea_level - chunk.height(tx, ty);
                if water && depth > SHALLOW_WATER_DEPTH + 0.5 {
                    return Some(Vec3::new(candidate.x, chunk.height(tx, ty), candidate.z));
                }
            }
            x += 2.0;
        }
        z += 2.0;
    }
    None
}

/// Two walkable positions on opposite sides of a body of deep water.
///
/// Scans along the `+X` axis from every deep-water sample for land on both
/// sides, which is what a "do not swim the lake" test needs.
pub fn shores_across_water(streamer: &WorldStreamer) -> Option<(Vec3, Vec3)> {
    let positions = streamer.loaded_positions();
    let first = *positions.first()?;
    let size = streamer.generator().config().chunk_world_size();
    let view = streamer.generator().config().view_distance_chunks.max(2);
    let origin = Vec3::new(first.x as f32 * size, 0.0, first.y as f32 * size);
    let limit = (view as f32 - 1.0) * size;
    let mut z = 8.0;
    while z < limit {
        let mut x = 8.0;
        while x < limit {
            let candidate = Vec3::new(origin.x + x, 0.0, origin.z + z);
            if is_water(streamer, candidate) && !streamer.is_walkable(candidate) {
                let west = walk_back(streamer, candidate, Vec3::new(-1.0, 0.0, 0.0), 48.0);
                let east = walk_back(streamer, candidate, Vec3::new(1.0, 0.0, 0.0), 48.0);
                if let (Some(a), Some(b)) = (west, east)
                    && a.distance(b) > 20.0
                {
                    return Some((a, b));
                }
            }
            x += 4.0;
        }
        z += 4.0;
    }
    None
}

/// True when the tile under `p` is water.
pub fn is_water(streamer: &WorldStreamer, p: Vec3) -> bool {
    let Some(chunk) = streamer.chunk_at(p) else {
        return false;
    };
    let Some((x, y)) = chunk.world_to_tile(p) else {
        return false;
    };
    chunk
        .tile_def(x, y, streamer.generator().tile_set())
        .is_some_and(|def| def.flags.water)
}

/// The first walkable position in `direction` from `start`, out to `limit`.
fn walk_back(streamer: &WorldStreamer, start: Vec3, direction: Vec3, limit: f32) -> Option<Vec3> {
    let mut distance = 1.0;
    while distance <= limit {
        let point = start + direction * distance;
        if let Some(ground) = ground_at(streamer, point)
            && ground.distance(start) > 0.0
        {
            return Some(ground);
        }
        distance += 1.0;
    }
    None
}

/// The axis-aligned bounds of the loaded chunks.
pub fn loaded_bounds(streamer: &WorldStreamer) -> Option<Aabb> {
    let positions = streamer.loaded_positions();
    if positions.is_empty() {
        return None;
    }
    let size = streamer.generator().config().chunk_world_size();
    let mut min = Vec3::new(f32::MAX, -1.0e6, f32::MAX);
    let mut max = Vec3::new(f32::MIN, 1.0e6, f32::MIN);
    for pos in positions {
        min.x = min.x.min(pos.x as f32 * size);
        min.z = min.z.min(pos.y as f32 * size);
        max.x = max.x.max((pos.x + 1) as f32 * size);
        max.z = max.z.max((pos.y + 1) as f32 * size);
    }
    Some(Aabb::new(min, max))
}

/// True when `p` is inside a chunk the streamer holds.
pub fn is_loaded(streamer: &WorldStreamer, p: Vec3) -> bool {
    streamer.chunk_at(p).is_some()
}

/// Walks every point along a segment at `step` metres, exclusive of `from`.
pub fn sample_segment(from: Vec3, to: Vec3, step: f32) -> Vec<Vec3> {
    let delta = to - from;
    let length = (delta.x * delta.x + delta.z * delta.z).sqrt();
    if length <= 1e-4 {
        return vec![to];
    }
    let count = (length / step).ceil().max(1.0) as usize;
    (0..=count)
        .map(|i| {
            let t = i as f32 / count as f32;
            Vec3::new(from.x + delta.x * t, 0.0, from.z + delta.z * t)
        })
        .collect()
}

/// Walks every point along a path's polyline.
pub fn sample_path(waypoints: &[Vec3], step: f32, from: Vec3) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut cursor = from;
    for waypoint in waypoints {
        out.extend(sample_segment(cursor, *waypoint, step));
        cursor = *waypoint;
    }
    out
}
