//! Cross-module tests: determinism, continuity, biomes, roads, towns, props,
//! streaming and the error paths.
//!
//! These exercise the crate the way another crate does — through the public API
//! only — so they also act as a compile-time check that the surface is usable
//! without reaching into internals.

use std::sync::Arc;

use noxel_asset::format::{Prefab, PrefabVoxel, TileDef, TileFlags, TileSet};
use noxel_core::math::{Aabb, ChunkPos, Vec2, Vec3};
use noxel_world::{
    BiomeId, Chunk, Facing, RoadNetwork, TownPlan, WorldConfig, WorldGenerator, WorldStreamer,
    chunk_to_string,
};

// --- harness ---------------------------------------------------------------

/// A tile set with every name the biome table asks for.
fn rich_tile_set() -> Arc<TileSet> {
    const NAMES: [&str; 18] = [
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
    let tiles = NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| TileDef {
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
        })
        .collect();
    Arc::new(TileSet {
        name: "ground".into(),
        texture: "ground.png".into(),
        tile_size: 16,
        tiles,
    })
}

/// An empty tile set: every named lookup fails.
fn empty_tile_set() -> Arc<TileSet> {
    Arc::new(TileSet {
        name: "empty".into(),
        texture: String::new(),
        tile_size: 16,
        tiles: Vec::new(),
    })
}

/// A library with one small house and one long barn.
fn house_prefabs() -> Vec<Arc<Prefab>> {
    let solid = |name: &str, w: u8, h: u8, d: u8| {
        let mut voxels = Vec::new();
        for x in 0..w {
            for y in 0..h {
                for z in 0..d {
                    voxels.push(PrefabVoxel { x, y, z, tile: 0 });
                }
            }
        }
        Arc::new(Prefab {
            name: name.to_string(),
            size: [w, h, d],
            tags: vec!["building".to_string()],
            voxels,
            props: Vec::new(),
            spawns: Vec::new(),
            occluders: Vec::new(),
        })
    };
    vec![solid("house_small", 5, 3, 4), solid("barn_long", 9, 4, 5)]
}

fn generator(seed: u64) -> WorldGenerator {
    WorldGenerator::new(WorldConfig::new(seed), rich_tile_set(), house_prefabs())
}

fn road_tile_id(set: &TileSet) -> u32 {
    set.tile_by_name("road").expect("the harness has a road").id
}

fn water_tile_ids(set: &TileSet) -> Vec<u32> {
    set.tiles
        .iter()
        .filter(|t| t.flags.water)
        .map(|t| t.id)
        .collect()
}

/// A coarse but complete fingerprint of a chunk, for equality checks.
fn fingerprint(chunk: &Chunk) -> String {
    let mut out = format!(
        "{:?}|{:?}|{}|{}|{}|{}|{}|{}",
        chunk.pos,
        chunk.biome,
        chunk.has_town,
        chunk.tiles.len(),
        chunk.colliders.len(),
        chunk.props.len(),
        chunk.buildings.len(),
        chunk.roads.len()
    );
    for i in 0..chunk.tiles.len() {
        out.push_str(&format!(
            ",{}:{:.4}:{:.4}",
            chunk.tiles[i], chunk.heights[i], chunk.slopes[i]
        ));
    }
    for (bounds, id) in &chunk.colliders {
        out.push_str(&format!(",c{id}:{:?}", bounds));
    }
    for prop in &chunk.props {
        out.push_str(&format!(",p{}:{:?}", prop.name, prop.position));
    }
    for building in &chunk.buildings {
        out.push_str(&format!(",b{}:{:?}", building.prefab, building.origin));
    }
    for road in &chunk.roads {
        out.push_str(&format!(",r{:?}", road));
    }
    out
}

fn chunk_bytes(chunk: &Chunk) -> Vec<u8> {
    fingerprint(chunk).into_bytes()
}

// --- determinism -----------------------------------------------------------

#[test]
fn a_chunk_is_a_pure_function_of_seed_and_position() {
    let g = generator(2024);
    let target = ChunkPos::new(900, -400);

    // Order A: straight to the target.
    let a = g.generate_chunk(target);

    // Order B: a long, meandering walk that revisits chunks and crosses towns.
    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(-7, 3),
        ChunkPos::new(10, 10),
        ChunkPos::new(900, -400),
        ChunkPos::new(-1, -1),
        ChunkPos::new(20, -30),
    ] {
        let _ = g.generate_chunk(pos);
    }
    let b = g.generate_chunk(target);

    assert_eq!(chunk_bytes(&a), chunk_bytes(&b));
    assert_eq!(a.heights, b.heights);
    assert_eq!(a.tiles, b.tiles);
}

#[test]
fn neighbouring_chunks_generate_in_any_order() {
    let g = generator(5);
    let order_a = [
        ChunkPos::new(3, 4),
        ChunkPos::new(4, 4),
        ChunkPos::new(3, 5),
    ];
    let order_b = [
        ChunkPos::new(3, 5),
        ChunkPos::new(3, 4),
        ChunkPos::new(4, 4),
    ];
    let first: Vec<Vec<u8>> = order_a
        .iter()
        .map(|p| chunk_bytes(&g.generate_chunk(*p)))
        .collect();
    let second: Vec<Vec<u8>> = order_b
        .iter()
        .map(|p| chunk_bytes(&g.generate_chunk(*p)))
        .collect();
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a, b);
    }
}

#[test]
fn different_seeds_generate_different_worlds() {
    let a = generator(1);
    let b = generator(2);
    let pos = ChunkPos::new(4, -4);
    let ca = a.generate_chunk(pos);
    let cb = b.generate_chunk(pos);
    assert_ne!(ca.heights, cb.heights);
    assert_ne!(ca.tiles, cb.tiles);
    assert_ne!(
        a.sample_height(10.0, 10.0),
        b.sample_height(10.0, 10.0),
        "heights must depend on the seed"
    );
}

#[test]
fn the_generator_survives_being_shared_across_threads() {
    let g = Arc::new(generator(9));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let g = Arc::clone(&g);
            std::thread::spawn(move || g.generate_chunk(ChunkPos::new(i, i)).tiles)
        })
        .collect();
    let from_threads: Vec<Vec<u32>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (i, tiles) in from_threads.iter().enumerate() {
        let i = i as i32;
        assert_eq!(*tiles, g.generate_chunk(ChunkPos::new(i, i)).tiles);
    }
}

#[test]
fn sample_height_agrees_with_the_generated_chunk() {
    let g = generator(17);
    let cs = g.config().chunk_world_size();
    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(-3, 6),
        ChunkPos::new(25, -25),
    ] {
        let chunk = g.generate_chunk(pos);
        let mut worst: f32 = 0.0;
        for y in 0..chunk.tiles() {
            for x in 0..chunk.tiles() {
                let wx = pos.x as f32 * cs + (x as f32 + 0.5) * g.config().tile_size;
                let wz = pos.y as f32 * cs + (y as f32 + 0.5) * g.config().tile_size;
                worst = worst.max((chunk.height(x, y) - g.sample_height(wx, wz)).abs());
            }
        }
        assert!(worst < 1e-3, "worst disagreement {worst} m");
    }
}

#[test]
fn sample_slope_agrees_with_the_chunk_slope_field() {
    let g = generator(19);
    let cs = g.config().chunk_world_size();
    let chunk = g.generate_chunk(ChunkPos::new(2, 2));
    let mut worst: f32 = 0.0;
    for y in 1..chunk.tiles() - 1 {
        for x in 1..chunk.tiles() - 1 {
            let wx = 2.0 * cs + (x as f32 + 0.5) * g.config().tile_size;
            let wz = 2.0 * cs + (y as f32 + 0.5) * g.config().tile_size;
            worst = worst.max((chunk.slope(x, y) - g.sample_slope(wx, wz)).abs());
        }
    }
    // The chunk uses the tile grid's central difference; `sample_slope` uses the
    // same step on the analytic field. They agree except where the two grids
    // straddle a break in slope, which is a genuinely steep tile.
    assert!(worst < 0.6, "worst slope disagreement {worst}");
}

// --- continuity ------------------------------------------------------------

/// The largest height change produced by walking `step` metres along a fan of
/// scan lines across a large area.
fn largest_step(g: &WorldGenerator, step: f32, lines: i32, length: f32) -> f32 {
    let mut worst: f32 = 0.0;
    for k in 0..lines {
        let z = k as f32 * 53.0 - (lines as f32 * 26.5);
        let mut previous = g.sample_height(-length * 0.5, z);
        let mut x = -length * 0.5 + step;
        while x < length * 0.5 {
            let h = g.sample_height(x, z);
            worst = worst.max((h - previous).abs());
            previous = h;
            x += step;
        }
    }
    worst
}

#[test]
fn height_is_continuous_across_a_large_area() {
    let g = generator(31);
    let worst = largest_step(&g, 0.1, 24, 400.0);
    // 0.1 m at 5 m/m would be a break of half a metre; the field's own slope
    // never exceeds a small fraction of that, so anything larger is a seam.
    assert!(worst < 0.5, "a 0.1 m step moved the surface {worst} m");
    assert!(worst > 1e-5, "the field must not be flat");

    // Continuity means the change shrinks with the step. A jump would not.
    let coarse = largest_step(&g, 0.4, 8, 200.0);
    let half = largest_step(&g, 0.2, 8, 200.0);
    let quarter = largest_step(&g, 0.1, 8, 200.0);
    assert!(half < coarse * 0.8, "{coarse} -> {half}");
    assert!(quarter < half * 0.8, "{half} -> {quarter}");
}

#[test]
fn height_is_continuous_across_chunk_seams() {
    let g = generator(37);
    let cs = g.config().chunk_world_size();
    let mut worst: f32 = 0.0;
    for boundary in [-2.0f32, -1.0, 0.0, 1.0, 3.0] {
        let x = boundary * cs;
        let mut previous = g.sample_height(x - 0.05, 12.0);
        let after = g.sample_height(x + 0.05, 12.0);
        worst = worst.max((after - previous).abs());
        previous = after;
    }
    assert!(worst < 0.05, "chunk seams show a {worst} m jump");
}

#[test]
fn roads_do_not_punch_holes_in_the_height_field() {
    let g = generator(41);
    let spacing = g.config().road_spacing();
    let mut worst: f32 = 0.0;
    for i in -2..2 {
        let x = noxel_world::road::macro_line_x(&g.config().clone(), i);
        for k in 0..20 {
            let z = k as f32 * 17.0;
            worst = worst.max(largest_step_at(&g, x, z));
        }
    }
    assert!(spacing > 0.0);
    assert!(
        worst < 0.2,
        "the road shoulder breaks the surface by {worst}"
    );
}

/// The largest change in a 0.1 m walk along Z through `(x, z)`.
fn largest_step_at(g: &WorldGenerator, x: f32, z: f32) -> f32 {
    let mut worst: f32 = 0.0;
    let mut previous = g.sample_height(x, z - 5.0);
    let mut t = -5.0 + 0.1;
    while t < 5.0 {
        let h = g.sample_height(x, z + t);
        worst = worst.max((h - previous).abs());
        previous = h;
        t += 0.1;
    }
    worst
}

// --- biomes ----------------------------------------------------------------

/// Samples biome ids over a large area.
fn biome_census(g: &WorldGenerator, extent: f32, spacing: f32) -> Vec<(BiomeId, f32, f32)> {
    let steps = (extent / spacing) as i32;
    let mut out = Vec::with_capacity((steps * steps) as usize);
    for i in 0..steps {
        for j in 0..steps {
            let x = i as f32 * spacing - extent * 0.5;
            let z = j as f32 * spacing - extent * 0.5;
            out.push((g.biome_at(x, z), g.sample_height(x, z), x));
        }
    }
    out
}

#[test]
fn every_biome_appears_somewhere() {
    let g = generator(1234);
    let census = biome_census(&g, 8000.0, 20.0);
    let mut seen = [0usize; 8];
    for (biome, _, _) in &census {
        seen[biome.index()] += 1;
    }
    for (i, id) in BiomeId::ALL.iter().enumerate() {
        assert!(
            seen[i] > 0,
            "biome {} never appeared in {} samples ({seen:?})",
            id.name(),
            census.len()
        );
    }
}

#[test]
fn biomes_are_spatially_coherent_not_speckled() {
    let g = generator(4321);
    let spacing = 8.0;
    let extent = 1600.0;
    let steps = (extent / spacing) as i32;
    let mut same = 0;
    let mut total = 0;
    for i in 0..steps {
        for j in 0..steps {
            let x = i as f32 * spacing;
            let z = j as f32 * spacing;
            let a = g.biome_at(x, z);
            let b = g.biome_at(x + spacing, z);
            total += 1;
            if a == b {
                same += 1;
            }
        }
    }
    let ratio = same as f32 / total as f32;
    assert!(ratio > 0.85, "biomes change too fast: {ratio}");
}

#[test]
fn water_is_below_sea_level_and_mountains_are_high() {
    let g = generator(99);
    let sea = g.config().sea_level;
    let mut heights: std::collections::HashMap<BiomeId, (f32, f32)> =
        std::collections::HashMap::new();
    for (biome, height, _) in biome_census(&g, 6000.0, 25.0) {
        let entry = heights
            .entry(biome)
            .or_insert((f32::INFINITY, f32::NEG_INFINITY));
        entry.0 = entry.0.min(height);
        entry.1 = entry.1.max(height);
    }
    if let Some((_, top)) = heights.get(&BiomeId::LAKE) {
        assert!(*top <= sea, "a lake tile was above sea level at {top}");
    } else {
        panic!("no water in a 6 km sample");
    }
    let mountains = heights
        .get(&BiomeId::MOUNTAINS)
        .expect("no mountains in a 6 km sample");
    for (biome, (_, top)) in &heights {
        if *biome == BiomeId::MOUNTAINS {
            continue;
        }
        assert!(
            *top <= mountains.1 + 1e-3,
            "{} reaches {} m, above the mountains' {} m",
            biome.name(),
            top,
            mountains.1
        );
    }
    assert!(mountains.0 > sea, "mountains must be dry land");
}

#[test]
fn biome_selection_matches_the_documented_rules() {
    let g = generator(7);
    let sea = g.config().sea_level;
    for (biome, height, x) in biome_census(&g, 3000.0, 30.0) {
        if height < sea {
            assert_eq!(biome, BiomeId::LAKE, "below sea level at x={x}");
        } else {
            assert_ne!(biome, BiomeId::LAKE, "above water at x={x}");
        }
    }
}

// --- roads -----------------------------------------------------------------

#[test]
fn the_macro_lattice_is_connected_through_its_junctions() {
    let g = generator(11);
    let config = g.config();
    let net = RoadNetwork::generate(config, ChunkPos::new(-8, -8), ChunkPos::new(8, 8));
    assert!(net.len() >= 8);

    // Every point along every segment is on a road, and walking to the segment's
    // end (a junction with the perpendicular family) stays on roads the whole
    // way: that is what "connected by construction" means.
    for segment in &net.segments {
        let steps = (segment.length() / 2.0).max(2.0) as i32;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let p = segment.from.lerp(segment.to, t);
            assert!(net.is_on_road(p, 1e-3), "gap at {p:?}");
            assert!(
                g.is_road_at(p.x, p.z),
                "the generator does not stamp the tile at {p:?}"
            );
        }
    }
}

#[test]
fn lattice_junctions_are_shared_between_two_roads() {
    let g = generator(13);
    let config = g.config();
    let net = RoadNetwork::generate(config, ChunkPos::new(-3, -3), ChunkPos::new(3, 3));
    for i in -3..=3 {
        for j in -3..=3 {
            let x = noxel_world::road::macro_line_x(config, i);
            let z = noxel_world::road::macro_line_z(config, j);
            let junction = Vec3::new(x, 0.0, z);
            let touching = net
                .segments
                .iter()
                .filter(|s| s.distance_to(junction) < 1e-3)
                .count();
            assert!(touching >= 2, "junction {i},{j} touches {touching} roads");
        }
    }
}

#[test]
fn a_chunk_on_a_lattice_boundary_contains_road_tiles() {
    let g = generator(23);
    let config = g.config();
    let cs = config.chunk_world_size();
    let road = road_tile_id(g.tile_set());
    let mut checked = 0;
    for i in -2..=2 {
        let x = noxel_world::road::macro_line_x(config, i);
        let chunk_x = (x / cs).floor() as i32;
        for chunk_y in [-1, 0, 1] {
            let chunk = g.generate_chunk(ChunkPos::new(chunk_x, chunk_y));
            let road_tiles = chunk.tiles.iter().filter(|t| **t == road).count();
            assert!(
                road_tiles > 0,
                "chunk ({chunk_x},{chunk_y}) holds the road at x={x} but has none"
            );
            checked += 1;
        }
    }
    assert!(checked >= 10);
}

#[test]
fn roads_are_stamped_continuously_along_their_length() {
    let g = generator(29);
    let config = g.config();
    let cs = config.chunk_world_size();
    let road = road_tile_id(g.tile_set());
    let x = noxel_world::road::macro_line_x(config, 1);
    let chunk_x = (x / cs).floor() as i32;
    // Walk north through five chunks and check every chunk has road tiles near
    // the line: a road that stops at a chunk border is the classic streaming bug.
    for chunk_y in -2..3 {
        let chunk = g.generate_chunk(ChunkPos::new(chunk_x, chunk_y));
        let origin = Vec3::new(chunk_x as f32 * cs, 0.0, chunk_y as f32 * cs);
        let local_x = ((x - origin.x) / g.config().tile_size).floor() as u32;
        if local_x >= chunk.tiles() {
            continue;
        }
        let has_road = (0..chunk.tiles()).any(|y| {
            chunk.tile(local_x, y) == road || chunk.tile(local_x.saturating_sub(1), y) == road
        });
        assert!(has_road, "the road vanishes in chunk ({chunk_x},{chunk_y})");
    }
}

#[test]
fn road_queries_agree_between_the_network_and_the_generator() {
    let g = generator(31);
    let config = g.config();
    let net = RoadNetwork::generate(config, ChunkPos::new(-4, -4), ChunkPos::new(4, 4));
    for segment in &net.segments {
        for k in 0..8 {
            let t = k as f32 / 7.0;
            let p = segment.from.lerp(segment.to, t);
            assert!(net.is_on_road(p, 0.0));
            assert!(g.is_road_at(p.x, p.z), "the generator disagrees at {p:?}");
        }
    }
    // And a point far from every line is on nothing.
    let spacing = config.road_spacing();
    let p = Vec3::new(
        noxel_world::road::macro_line_x(config, 0) + spacing * 0.5,
        0.0,
        noxel_world::road::macro_line_z(config, 0) + spacing * 0.5,
    );
    assert!(!net.is_on_road(p, 0.0));
    assert!(!g.is_road_at(p.x, p.z));
}

// --- towns -----------------------------------------------------------------

#[test]
fn towns_exist_at_the_lattice_positions() {
    let g = generator(51);
    let spacing = g.config().town_spacing_chunks;
    for (i, j) in [(0, 0), (1, 0), (-1, 2), (3, -3)] {
        let cell = ChunkPos::new(i * spacing, j * spacing);
        let plan = g
            .town_at(cell)
            .unwrap_or_else(|| panic!("no town at {cell:?}"));
        assert_eq!(plan.center_chunk, cell);
        assert!(!plan.name.is_empty());
        assert!(!plan.streets.is_empty());
        assert!(!plan.building_plots.is_empty());
    }
}

#[test]
fn a_town_plan_is_the_same_from_every_chunk_that_sees_it() {
    let g = generator(53);
    let spacing = g.config().town_spacing_chunks;
    let cell = ChunkPos::new(spacing, 0);
    let plan = g.town_at(cell).unwrap();
    let radius = plan.radius_chunks;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let probe = ChunkPos::new(cell.x + dx, cell.y + dy);
            let other = g.town_at(probe).expect("the disc covers this chunk");
            assert_eq!(other.center_chunk, cell);
            assert_eq!(other.name, plan.name);
            assert_eq!(other.style, plan.style);
            assert_eq!(other.building_plots.len(), plan.building_plots.len());
        }
    }
}

#[test]
fn no_two_buildings_in_a_town_overlap() {
    let g = generator(59);
    let spacing = g.config().town_spacing_chunks;
    for cell in [
        ChunkPos::new(0, 0),
        ChunkPos::new(spacing, spacing),
        ChunkPos::new(-spacing, spacing),
    ] {
        let plan = g.town_at(cell).unwrap();
        let buildings = plan.instantiate(g.config(), g.prefabs(), |x, z| g.sample_height(x, z));
        assert!(!buildings.is_empty(), "{} has no buildings", plan.name);
        for a in 0..buildings.len() {
            for b in (a + 1)..buildings.len() {
                let (x, y) = (&buildings[a], &buildings[b]);
                assert!(
                    !x.bounds.intersects(&y.bounds),
                    "{} and {} overlap in {}",
                    x.prefab,
                    y.prefab,
                    plan.name
                );
            }
        }
        assert_eq!(
            buildings.len(),
            plan.building_plots.iter().filter(|p| p.taken).count()
        );
    }
}

#[test]
fn every_building_plot_is_inside_the_town_radius() {
    let g = generator(61);
    let spacing = g.config().town_spacing_chunks;
    let config = g.config();
    let cell = ChunkPos::new(spacing, 0);
    let plan = g.town_at(cell).unwrap();
    let radius = plan.radius_world(config);
    let plaza = Vec2::new(plan.plaza_center.x, plan.plaza_center.z);
    let reach = noxel_world::town::plaza_radius(config);
    for plot in &plan.building_plots {
        let rect = plot.rect(config.tile_size);
        let centre = rect.center();
        let d = Vec2::new(centre.x - plaza.x, centre.y - plaza.y).length();
        assert!(d <= radius, "plot at {d} m > {radius} m");
        let half = Vec2::new(rect.width() * 0.5, rect.height() * 0.5).length();
        assert!(
            !noxel_world::town::in_plaza(plaza, centre, half, reach),
            "plot overlaps the plaza"
        );
        assert!(
            plan.contains_world(config, Vec3::new(centre.x, 0.0, centre.y)),
            "plot outside the built-up disc"
        );
    }
}

#[test]
fn every_building_faces_a_street() {
    let g = generator(67);
    let spacing = g.config().town_spacing_chunks;
    for cell in [ChunkPos::new(0, 0), ChunkPos::new(spacing, -spacing)] {
        let plan = g.town_at(cell).unwrap();
        let buildings = plan.instantiate(g.config(), g.prefabs(), |x, z| g.sample_height(x, z));
        for building in &buildings {
            let mid = building.bounds.center();
            let front = Vec3::new(
                mid.x + building.facing.vector().x * building.size_tiles.0 as f32 * 0.5,
                building.origin.y,
                mid.z + building.facing.vector().z * building.size_tiles.1 as f32 * 0.5,
            );
            let distance = plan.distance_to_street(front);
            assert!(
                distance < g.config().chunk_world_size() * 0.5,
                "{} faces {distance} m from a street in {}",
                building.prefab,
                plan.name
            );
            assert_eq!(building.facing, building.facing);
            assert!(Facing::ALL.contains(&building.facing));
        }
    }
}

#[test]
fn town_chunks_are_flat_and_flagged() {
    let g = generator(71);
    let spacing = g.config().town_spacing_chunks;
    let cell = ChunkPos::new(spacing, spacing);
    let plan = g.town_at(cell).unwrap();
    let chunk = g.generate_chunk(cell);
    assert!(chunk.has_town);
    assert!(
        !chunk.buildings.is_empty(),
        "{} has no buildings",
        plan.name
    );
    assert!(
        !chunk.colliders.is_empty(),
        "{} has no colliders",
        plan.name
    );
    assert!(
        !chunk.roads.is_empty(),
        "{} has no streets in its centre chunk",
        plan.name
    );
    let plaza = plan.plaza_center;
    assert!(
        g.sample_slope(plaza.x, plaza.z) < 0.05,
        "the plaza is not flat"
    );
    for y in 0..chunk.tiles() {
        for x in 0..chunk.tiles() {
            let p = chunk.tile_world_center(x, y);
            let d = Vec2::new(p.x - plaza.x, p.z - plaza.z).length();
            if d < g.config().chunk_world_size() * 0.4 {
                assert!(
                    (chunk.height(x, y) - plaza.y).abs() < 1.0,
                    "tile {x},{y} is not on the plaza level"
                );
            }
        }
    }
}

#[test]
fn a_town_survives_an_empty_prefab_library() {
    let g = WorldGenerator::new(WorldConfig::new(73), rich_tile_set(), Vec::new());
    let spacing = g.config().town_spacing_chunks;
    let cell = ChunkPos::new(spacing, 0);
    let chunk = g.generate_chunk(cell);
    assert!(chunk.has_town);
    assert!(
        !chunk.buildings.is_empty(),
        "the fallback house must appear"
    );
    for building in &chunk.buildings {
        assert_eq!(building.prefab, noxel_world::town::PROCEDURAL_PREFAB);
        assert_eq!(building.occluders.len(), 2);
    }
}

#[test]
fn a_building_straddling_a_border_is_reported_by_both_chunks() {
    let g = generator(79);
    let spacing = g.config().town_spacing_chunks;
    let cell = ChunkPos::new(spacing, 0);
    let plan = g.town_at(cell).unwrap();
    let buildings = plan.instantiate(g.config(), g.prefabs(), |x, z| g.sample_height(x, z));
    let cs = g.config().chunk_world_size();
    let mut straddling = 0;
    for building in &buildings {
        let min = ChunkPos::from_world(building.bounds.min, cs);
        let max = ChunkPos::from_world(building.bounds.max, cs);
        if min != max {
            straddling += 1;
            let a = g.generate_chunk(min);
            let b = g.generate_chunk(max);
            let in_a = a.buildings.iter().any(|x| x.origin == building.origin);
            let in_b = b.buildings.iter().any(|x| x.origin == building.origin);
            assert!(in_a, "{min:?} misses a building it overlaps");
            assert!(in_b, "{max:?} misses a building it overlaps");
        }
    }
    assert!(straddling > 0, "the test needs a building on a border");
}

// --- props, colliders and occluders ----------------------------------------

#[test]
fn props_are_never_on_roads_in_water_or_in_buildings() {
    let g = generator(83);
    let mut checked = 0;
    for i in -3..=3 {
        for j in -3..=3 {
            let chunk = g.generate_chunk(ChunkPos::new(i, j));
            for prop in &chunk.props {
                let p = prop.position;
                assert!(p.y >= g.config().sea_level, "prop in water at {p:?}");
                assert!(!g.is_road_at(p.x, p.z), "prop on a road at {p:?}");
                let (tx, ty) = chunk.world_to_tile(p).expect("prop inside its chunk");
                assert!(
                    chunk.slope(tx, ty) <= noxel_world::r#gen::MAX_PROP_SLOPE,
                    "prop on a steep slope at {p:?}"
                );
                for building in &chunk.buildings {
                    assert!(!building.contains_xz(p), "prop inside a building");
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 50, "expected props to place, got {checked}");
}

#[test]
fn trees_are_flagged_as_occluders_and_rocks_are_not() {
    let g = generator(89);
    let mut trees = 0;
    let mut rocks = 0;
    for j in -2..=2 {
        for i in -2..=2 {
            let chunk = g.generate_chunk(ChunkPos::new(i, j));
            for prop in &chunk.props {
                match prop.name.as_str() {
                    "tree" => {
                        assert!(prop.occluder);
                        assert!(prop.occluder_bounds().is_some());
                        trees += 1;
                    }
                    "rock" => {
                        assert!(!prop.occluder);
                        assert!(prop.occluder_bounds().is_none());
                        assert!(prop.collider_bounds().is_some());
                        rocks += 1;
                    }
                    other => panic!("unexpected prop kind {other}"),
                }
            }
        }
    }
    assert!(trees > 10 && rocks > 0, "trees {trees}, rocks {rocks}");
}

#[test]
fn colliders_and_occluders_are_finite_and_well_formed() {
    let g = generator(97);
    let spacing = g.config().town_spacing_chunks;
    for cell in [ChunkPos::new(0, 0), ChunkPos::new(spacing, spacing)] {
        let chunk = g.generate_chunk(cell);
        for (bounds, id) in &chunk.colliders {
            assert!(bounds.is_finite(), "{bounds:?}");
            assert!(!bounds.is_empty(), "empty collider {bounds:?}");
            assert_ne!(*id, 0);
        }
        let mut occluders = 0;
        for (bounds, _) in chunk.occluder_boxes() {
            assert!(bounds.is_finite());
            assert!(!bounds.is_empty());
            occluders += 1;
        }
        assert!(occluders > 0, "a town needs occluders");
    }
}

#[test]
fn building_occluders_are_merged_not_per_voxel() {
    let g = generator(101);
    let plan = g.town_at(ChunkPos::new(0, 0)).unwrap();
    let buildings = plan.instantiate(g.config(), g.prefabs(), |x, z| g.sample_height(x, z));
    let mut checked = 0;
    for building in &buildings {
        if building.prefab == noxel_world::town::PROCEDURAL_PREFAB {
            continue;
        }
        let prefab = g
            .prefabs()
            .iter()
            .find(|p| p.name == building.prefab)
            .expect("the building came from the library");
        assert!(
            building.occluders.len() < prefab.voxels.len(),
            "{}: {} occluders for {} voxels",
            prefab.name,
            building.occluders.len(),
            prefab.voxels.len()
        );
        checked += 1;
    }
    assert!(checked > 0, "no prefab buildings were placed");
}

#[test]
fn collider_ids_are_stable_across_regeneration() {
    let g = generator(103);
    let first = g.generate_chunk(ChunkPos::new(0, 0));
    let second = g.generate_chunk(ChunkPos::new(0, 0));
    assert_eq!(first.colliders, second.colliders);
    let mut ids: Vec<u64> = first.colliders.iter().map(|(_, id)| *id).collect();
    ids.sort_unstable();
    let unique = ids.len();
    ids.dedup();
    assert_eq!(unique, ids.len(), "collider ids must be distinct");
}

// --- streaming -------------------------------------------------------------

#[test]
fn update_loads_a_square_and_evicts_the_far_chunks() {
    let mut streamer = WorldStreamer::new(generator(107));
    let view = streamer.generator().config().view_distance_chunks;
    let keep = streamer.generator().config().keep_distance_chunks;
    let cs = streamer.generator().config().chunk_world_size();

    let stats = streamer.update(Vec3::ZERO);
    let expected = ((view * 2 + 1) * (view * 2 + 1)) as usize;
    assert_eq!(streamer.loaded_chunks(), expected);
    assert_eq!(stats.generated_this_update, expected);
    assert_eq!(streamer.loaded_chunks(), streamer.iter_loaded().count());

    // A second update in the same chunk does nothing at all.
    let idle = streamer.update(Vec3::new(1.0, 0.0, 1.0));
    assert_eq!(idle.generated_this_update, 0);
    assert_eq!(idle.evicted_this_update, 0);

    // Moving far away evicts everything outside the keep ring.
    let target = ChunkPos::new(20, 20);
    let moved = streamer.update(target.to_world_center(cs, 0.0));
    assert!(moved.evicted_this_update > 0);
    for (pos, _) in streamer.iter_loaded() {
        assert!(pos.chebyshev_distance(target) <= keep, "{pos:?} survived");
    }
    assert!(streamer.chunk(ChunkPos::ZERO).is_none());
    for pos in target.in_radius(view) {
        assert!(streamer.chunk(pos).is_some(), "{pos:?} missing");
    }
}

#[test]
fn memory_stays_bounded_over_a_long_journey() {
    let mut streamer = WorldStreamer::new(generator(109));
    let cs = streamer.generator().config().chunk_world_size();
    let cap = streamer.generator().config().max_cached_chunks;
    let view = streamer.generator().config().view_distance_chunks;
    let ring = ((view * 2 + 1) * (view * 2 + 1)) as usize;
    let mut peak = 0usize;
    for i in 0..60 {
        let angle = i as f32 * 0.3;
        let radius = i as f32 * cs * 3.0;
        streamer.update(Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius));
        peak = peak.max(streamer.loaded_chunks());
        assert!(
            streamer.loaded_chunks() <= cap.max(ring),
            "cache grew to {}",
            streamer.loaded_chunks()
        );
        assert!(streamer.memory_bytes() < ring.max(1) * 64 * 1024 + 1_000_000);
    }
    assert!(peak > 0);
}

#[test]
fn streamed_queries_match_the_chunks_they_came_from() {
    let mut streamer = WorldStreamer::new(generator(113));
    streamer.update(Vec3::ZERO);
    let set = streamer.generator().tile_set().clone();
    let water = water_tile_ids(&set);
    let mut samples = 0;
    for (_, chunk) in streamer.iter_loaded() {
        for y in (0..chunk.tiles()).step_by(7) {
            for x in (0..chunk.tiles()).step_by(7) {
                let p = chunk.tile_world_center(x, y);
                let tile = streamer.tile_at(p).expect("loaded");
                assert_eq!(tile, chunk.tile(x, y));
                assert_eq!(streamer.height_at(p), chunk.height(x, y));
                assert_eq!(streamer.is_walkable(p), chunk.is_walkable(x, y, &set));
                assert_eq!(streamer.blocks_sight(p), chunk.blocks_sight(x, y, &set));
                if water.contains(&tile) {
                    assert!(p.y < streamer.generator().config().sea_level);
                }
                samples += 1;
            }
        }
    }
    assert!(samples > 1000);
}

#[test]
fn colliders_in_agrees_with_a_brute_force_scan() {
    let mut streamer = WorldStreamer::new(generator(127));
    let spacing = streamer.generator().config().town_spacing_chunks;
    let cs = streamer.generator().config().chunk_world_size();
    let centre = ChunkPos::new(spacing, spacing);
    streamer.update(centre.to_world_center(cs, 0.0));

    let region = Aabb::new(
        Vec3::new(
            centre.x as f32 * cs - 1.0,
            -100.0,
            centre.y as f32 * cs - 1.0,
        ),
        Vec3::new(
            (centre.x + 1) as f32 * cs + 1.0,
            100.0,
            (centre.y + 1) as f32 * cs + 1.0,
        ),
    );

    let mut brute: Vec<(Aabb, u64)> = Vec::new();
    for (_, chunk) in streamer.iter_loaded() {
        for (bounds, id) in &chunk.colliders {
            if bounds.intersects(&region) {
                brute.push((*bounds, *id));
            }
        }
    }
    let mut fast = Vec::new();
    streamer.colliders_in(region, &mut fast);

    assert!(!brute.is_empty(), "the town chunk has no colliders");
    brute.sort_by(|a, b| a.1.cmp(&b.1));
    fast.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(fast, brute);
}

#[test]
fn occluders_in_only_returns_boxes_that_overlap() {
    let mut streamer = WorldStreamer::new(generator(131));
    streamer.update(Vec3::ZERO);
    let region = Aabb::new(Vec3::new(-64.0, -20.0, -64.0), Vec3::new(64.0, 40.0, 64.0));
    let mut out = Vec::new();
    streamer.occluders_in(region, &mut out);
    for (bounds, id) in &out {
        assert!(bounds.intersects(&region));
        assert_ne!(*id, 0);
    }
    // Everything that is close to the region is found.
    let expected: usize = streamer
        .iter_loaded()
        .map(|(_, chunk)| {
            chunk
                .occluder_boxes()
                .filter(|(b, _)| b.intersects(&region))
                .count()
        })
        .sum();
    assert_eq!(out.len(), expected);
}

// --- error paths -----------------------------------------------------------

#[test]
fn an_empty_tile_set_still_generates_a_world() {
    let g = WorldGenerator::new(WorldConfig::new(137), empty_tile_set(), Vec::new());
    let chunk = g.generate_chunk(ChunkPos::new(0, 0));
    assert_eq!(chunk.tiles.len(), 32 * 32);
    assert!(chunk.tiles.iter().all(|t| *t == 0));
    assert!(chunk.bounds().is_finite());
    assert!(g.sample_height(5.0, 5.0).is_finite());
    let text = chunk_to_string(&chunk, g.tile_set());
    assert!(text.contains("0:?"));
}

#[test]
fn a_zero_sized_tile_set_does_not_panic() {
    let set = Arc::new(TileSet {
        name: String::new(),
        texture: String::new(),
        tile_size: 0,
        tiles: Vec::new(),
    });
    let g = WorldGenerator::new(WorldConfig::new(139), set, Vec::new());
    let chunk = g.generate_chunk(ChunkPos::new(1, 1));
    assert_eq!(chunk.tiles(), 32);
    assert!(!chunk.is_walkable(0, 0, g.tile_set()));
    assert!(chunk.blocks_sight(0, 0, g.tile_set()));
    assert!(chunk_to_string(&chunk, g.tile_set()).contains("chunk 1 1"));
}

#[test]
fn a_tile_set_with_unknown_names_falls_back() {
    let set = Arc::new(TileSet {
        name: "odd".into(),
        texture: String::new(),
        tile_size: 8,
        tiles: vec![TileDef {
            id: 42,
            name: "unobtainium".into(),
            texture: String::new(),
            uv: [0, 0, 8, 8],
            flags: TileFlags {
                walkable: true,
                ..TileFlags::default()
            },
            height: 0.0,
            layer: 0,
        }],
    });
    let g = WorldGenerator::new(WorldConfig::new(149), set, Vec::new());
    let chunk = g.generate_chunk(ChunkPos::new(0, 0));
    assert!(chunk.tiles.iter().all(|t| *t == 42));
}

#[test]
fn degenerate_configs_do_not_panic() {
    let mut config = WorldConfig::new(151);
    config.tiles_per_chunk = 0;
    config.chunk_world_size = 0.0;
    config.height_scale = 0.0;
    config.tile_size = 0.0;
    config.road_grid_chunks = 0;
    config.town_spacing_chunks = 0;
    config.town_radius_chunks = 0;
    config.buildings_per_town = 0;
    config.view_distance_chunks = 0;
    config.keep_distance_chunks = 0;
    config.max_cached_chunks = 0;
    config.prop_density = f32::NAN;
    let g = WorldGenerator::new(config, rich_tile_set(), Vec::new());
    assert!(g.sample_height(3.0, 4.0).is_finite());
    assert!(g.sample_slope(3.0, 4.0).is_finite());
    let chunk = g.generate_chunk(ChunkPos::new(2, 2));
    assert!(chunk.bounds().is_finite());
    let mut streamer = WorldStreamer::new(g);
    let stats = streamer.update(Vec3::ZERO);
    assert_eq!(stats.loaded, 1);
}

#[test]
fn extreme_coordinates_do_not_panic() {
    let g = generator(157);
    for (x, z) in [(1e9, -1e9), (-1e9, 1e9), (1e30, 1e30), (f32::MAX, f32::MIN)] {
        let h = g.sample_height(x, z);
        assert!(h.is_finite(), "{x},{z} -> {h}");
        let _ = g.biome_at(x, z);
        let _ = g.is_road_at(x, z);
    }
    let chunk = g.generate_chunk(ChunkPos::new(1_000_000, -1_000_000));
    assert!(chunk.bounds().is_finite());
    assert_eq!(chunk.tiles.len(), 32 * 32);
}

// --- text dump -------------------------------------------------------------

#[test]
fn chunk_to_string_is_stable_and_diffable() {
    let g = generator(163);
    let chunk = g.generate_chunk(ChunkPos::new(-2, 3));
    let first = chunk_to_string(&chunk, g.tile_set());
    let second = chunk_to_string(&g.generate_chunk(ChunkPos::new(-2, 3)), g.tile_set());
    assert_eq!(first, second);
    assert!(first.contains("chunk -2 3 biome="));
    assert!(first.contains("\nlegend "));
    assert!(first.contains("\ntiles\n"));
    assert!(first.contains("\nheights\n"));
    let lines: Vec<&str> = first.lines().collect();
    let body: Vec<&&str> = lines.iter().filter(|l| l.len() == 32).collect();
    assert_eq!(body.len(), 64, "32 tile rows plus 32 height rows");
}

#[test]
fn a_golden_chunk_is_byte_identical_across_generators() {
    // Two independently constructed generators, same seed: the dump must match
    // exactly. This is the check a golden-world test in CI relies on.
    let a = generator(167).generate_chunk(ChunkPos::new(7, -7));
    let b = generator(167).generate_chunk(ChunkPos::new(7, -7));
    let set = rich_tile_set();
    assert_eq!(chunk_to_string(&a, &set), chunk_to_string(&b, &set));
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn town_and_wilderness_chunks_dump_differently() {
    let g = generator(173);
    let spacing = g.config().town_spacing_chunks;
    let town = g.generate_chunk(ChunkPos::new(spacing, spacing));
    let wild = g.generate_chunk(ChunkPos::new(spacing / 2, spacing / 2));
    let set = g.tile_set();
    let town_text = chunk_to_string(&town, set);
    let wild_text = chunk_to_string(&wild, set);
    assert!(town_text.contains("town=1"));
    assert!(wild_text.contains("town=0"));
    assert_ne!(town_text, wild_text);
}

#[test]
fn town_plans_are_usable_without_the_generator() {
    // The plan carries everything a UI or a pathfinder needs: names, streets and
    // plots, with no back-reference to the generator.
    let g = generator(179);
    let plan: TownPlan = g.town_at(ChunkPos::new(0, 0)).unwrap();
    assert!(plan.memory_bytes() > 0);
    assert!(plan.streets.iter().any(|s| s.is_main));
    assert!(plan.distance_to_street(plan.plaza_center) < 5.0);
    assert!(plan.is_on_street(plan.plaza_center));
    let free = plan.free_plots().count();
    let taken = plan.building_plots.iter().filter(|p| p.taken).count();
    assert_eq!(free + taken, plan.building_plots.len());
}
