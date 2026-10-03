//! Flow fields against a real streamed world: one search, many agents.

mod common;

use common::*;
use noxel_core::math::Vec3;
use noxel_npc::path::PathConfig;
use noxel_npc::{FlowField, FlowFieldCache, Pathfinder};

const CELL: f32 = 2.0;
const EXTENT: f32 = 128.0;

/// A crowd of walkable start positions within `EXTENT / 2` of `goal`.
///
/// Rings are swept outwards until enough standable spots are found, so the test
/// does not depend on the goal happening to sit in the middle of a plain.
fn crowd_starts(streamer: &noxel_world::WorldStreamer, goal: Vec3, count: usize) -> Vec<Vec3> {
    let mut starts = Vec::new();
    let inner = 8.0;
    let outer = EXTENT * 0.45;
    // A deterministic scatter rather than a spiral: a spiral lands on a
    // sub-lattice, and two agents sharing a grid node would share a path.
    let mut rng =
        noxel_core::rng::RngStream::indexed(0x4E50_435F_4E50_435F, "test/flow-starts", 0).rng();
    let mut attempt = 0usize;
    while starts.len() < count && attempt < count * 60 {
        let angle = rng.range_f32(0.0, core::f32::consts::TAU);
        let radius = inner + (outer - inner) * rng.next_f32().sqrt();
        let candidate = goal + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
        if let Some(point) = ground_at(streamer, candidate) {
            starts.push(point);
        }
        attempt += 1;
    }
    starts
}

/// Moves `positions` along a field's directions and reports how many arrive.
fn follow_field(field: &FlowField, positions: &mut [Vec3], goal: Vec3, steps: usize) -> usize {
    let dt = 0.1;
    let speed = 1.6;
    for _ in 0..steps {
        for position in positions.iter_mut() {
            if let Some(direction) = field.direction_at(*position) {
                *position += direction * (speed * dt);
            }
        }
    }
    positions
        .iter()
        .filter(|p| distance_xz(**p, goal) < 8.0)
        .count()
}

fn distance_xz(a: Vec3, b: Vec3) -> f32 {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    (dx * dx + dz * dz).sqrt()
}

#[test]
fn directions_point_towards_the_goal() {
    let streamer = streamer_at(2, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let field = FlowField::build(&streamer, goal, EXTENT, CELL).expect("a field");
    assert_eq!(field.goal(), goal);
    assert_eq!(field.cell_size(), CELL);
    assert!(field.side() >= 2);
    assert!(!field.is_empty());
    assert!(field.len() > 100, "a 64 m field should be mostly usable");
    assert!(field.memory_bytes() > 0);

    let mut checked = 0;
    for index in 0..96 {
        let angle = index as f32 / 96.0 * core::f32::consts::TAU;
        let radius = 6.0 + (index % 4) as f32 * 4.0;
        let sample = goal + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
        let Some(direction) = field.direction_at(sample) else {
            continue;
        };
        assert!(direction.is_finite());
        assert!((direction.length() - 1.0).abs() < 1e-4, "a unit direction");
        let before = field.cost_at(sample).expect("a reachable sample");
        let moved = sample + direction * CELL;
        let Some(after) = field.cost_at(moved) else {
            continue;
        };
        // Following a flow field is monotone in the field's own metric, which is
        // what guarantees an agent that keeps walking gets there. Straight-line
        // distance is *not* monotone: a route around a house briefly moves away.
        assert!(
            after < before,
            "a step along {direction:?} from {sample:?} did not approach the goal ({before} -> {after})"
        );
        checked += 1;
    }
    assert!(
        checked > 20,
        "expected directions around the goal, got {checked}"
    );
}

#[test]
fn following_the_field_actually_arrives() {
    let streamer = streamer_at(2, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let field = FlowField::build(&streamer, goal, EXTENT, CELL).expect("a field");
    let mut starts = crowd_starts(&streamer, goal, 60);
    assert!(starts.len() > 30);
    let before: f32 =
        starts.iter().map(|p| distance_xz(*p, goal)).sum::<f32>() / starts.len() as f32;
    let arrived = follow_field(&field, &mut starts, goal, 500);
    let after: f32 =
        starts.iter().map(|p| distance_xz(*p, goal)).sum::<f32>() / starts.len() as f32;
    assert!(
        after < before * 0.6,
        "the field did not close the distance: {before} -> {after}"
    );
    assert!(
        arrived * 2 >= starts.len(),
        "only {arrived} of {} agents arrived",
        starts.len()
    );
}

#[test]
fn a_shared_field_costs_far_less_than_per_agent_astar() {
    let streamer = streamer_at(19, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let starts = crowd_starts(&streamer, goal, 200);
    assert!(starts.len() >= 150, "need a crowd: {}", starts.len());
    // Group A: every agent runs its own A*.
    let mut pathfinder = Pathfinder::new(PathConfig::default());
    let mut a_star_arrivals = 0;
    for from in &starts {
        if pathfinder.find_path(&streamer, *from, goal).is_some() {
            a_star_arrivals += 1;
        }
    }
    let per_agent = pathfinder.stats();

    // Group B: one field serves everybody.
    let mut cache = FlowFieldCache::new(4);
    let built = cache.stats().1;
    let field = cache
        .get_or_build(&streamer, goal, EXTENT, CELL)
        .expect("a field");
    let field_cells = field.len();
    let mut positions = starts.clone();
    let arrived = follow_field(field, &mut positions, goal, 500);

    assert!(
        per_agent.paths_computed * 4 >= starts.len() as u64 * 3,
        "per-agent A* ran {} searches for {} agents",
        per_agent.paths_computed,
        starts.len()
    );
    assert_eq!(cache.stats().1 - built, 1, "the crowd builds one field");
    assert_eq!(cache.stats().0, 1, "and every later request is a cache hit");
    // The honest comparison: one build of `field_cells` cells against a search
    // per agent. The field is built once and then read by everybody, so a
    // hundred agents cost the same as one.
    let per_agent_nodes = per_agent.path_nodes_expanded / starts.len() as u64;
    assert!(
        per_agent_nodes >= 20,
        "each A* should do real work, got {per_agent_nodes} nodes"
    );
    assert!(
        (field_cells as u64) * 3 < per_agent.path_nodes_expanded,
        "the whole field ({field_cells} cells) should cost less than a third of \
         {} per-agent A* nodes",
        per_agent.path_nodes_expanded
    );
    assert!(
        per_agent.paths_computed * 4 >= starts.len() as u64 * 3,
        "the crowd should pay about one search per agent, got {} for {}",
        per_agent.paths_computed,
        starts.len()
    );
    assert!(
        arrived * 2 >= starts.len(),
        "only {arrived} of {} agents reached the goal on the field",
        starts.len()
    );
    assert!(
        a_star_arrivals * 2 >= starts.len(),
        "the A* group could not reach the goal either"
    );
}

#[test]
fn a_field_outside_the_streamed_region_is_refused() {
    let streamer = streamer_at(3, Vec3::ZERO);
    let far = Vec3::new(50_000.0, 0.0, 50_000.0);
    assert!(FlowField::build(&streamer, far, EXTENT, CELL).is_none());
    let mut cache = FlowFieldCache::new(2);
    assert!(cache.get_or_build(&streamer, far, EXTENT, CELL).is_none());
    assert!(cache.is_empty());
    assert_eq!(cache.stats().1, 0, "a refused field is not a build");
    assert!(FlowField::build(&streamer, Vec3::new(f32::NAN, 0.0, 0.0), EXTENT, CELL).is_none());
}

#[test]
fn a_field_whose_goal_is_underwater_is_refused() {
    let streamer = streamer_at(21, Vec3::ZERO);
    let Some(water) = deep_water(&streamer) else {
        return;
    };
    assert!(
        FlowField::build(&streamer, water, 24.0, CELL).is_none(),
        "a field may not point at deep water"
    );
}

#[test]
fn contains_and_direction_at_agree_with_the_region() {
    let streamer = streamer_at(2, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let field = FlowField::build(&streamer, goal, 32.0, CELL).expect("a field");
    let origin = field.origin();
    let span = field.side() as f32 * field.cell_size();
    assert!(field.contains(goal));
    assert!(field.contains(origin + Vec3::splat(0.1)));
    assert!(!field.contains(origin - Vec3::splat(1.0)));
    assert!(!field.contains(origin + Vec3::new(span + 1.0, 0.0, 1.0)));
    assert!(!field.contains(Vec3::new(f32::NAN, 0.0, 0.0)));
    assert!(field.direction_at(origin - Vec3::splat(5.0)).is_none());
    assert!(field.direction_at(Vec3::new(f32::NAN, 0.0, 0.0)).is_none());
    assert!(field.cost_at(goal).is_some());
    assert!(field.cost_at(origin - Vec3::splat(5.0)).is_none());
}

#[test]
fn the_cache_evicts_the_least_recently_used_goal() {
    let streamer = streamer_at(6, Vec3::ZERO);
    let base = flat_area(&streamer).expect("a flat area");
    let a = base;
    let b = base + Vec3::new(24.0, 0.0, 0.0);
    let c = base + Vec3::new(0.0, 0.0, 24.0);
    let mut cache = FlowFieldCache::new(2);
    assert!(cache.get_or_build(&streamer, a, 32.0, CELL).is_some());
    assert!(cache.get_or_build(&streamer, b, 32.0, CELL).is_some());
    assert_eq!(cache.len(), 2);
    // Touch `a` so `b` becomes the oldest.
    assert!(cache.get_or_build(&streamer, a, 32.0, CELL).is_some());
    assert!(cache.get_or_build(&streamer, c, 32.0, CELL).is_some());
    assert_eq!(cache.len(), 2, "the cache obeys its capacity");
    assert!(
        cache.find(a, CELL).is_some(),
        "the recently used field stays"
    );
    assert!(cache.find(b, CELL).is_none(), "the oldest field is evicted");
    assert!(cache.find(c, CELL).is_some());
}

#[test]
fn cache_stats_track_builds_hits_and_clears() {
    let streamer = streamer_at(8, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let mut cache = FlowFieldCache::new(4);
    assert!(cache.is_empty());
    assert_eq!(cache.stats(), (0, 0));
    assert_eq!(cache.hits(), 0);
    assert!(cache.get_or_build(&streamer, goal, 32.0, CELL).is_some());
    assert_eq!(cache.stats(), (1, 1));
    assert!(cache.get_or_build(&streamer, goal, 32.0, CELL).is_some());
    assert_eq!(cache.stats(), (1, 1), "the second request is a hit");
    assert_eq!(cache.hits(), 1);
    assert_eq!(cache.capacity(), 4);
    assert!(cache.memory_bytes() > 0);
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.stats(), (0, 1), "clearing does not forget the builds");
    assert!(cache.find(goal, CELL).is_none());
}

#[test]
fn nearby_goals_share_one_field() {
    let streamer = streamer_at(8, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let mut cache = FlowFieldCache::new(4);
    assert!(cache.get_or_build(&streamer, goal, 32.0, CELL).is_some());
    // Nudge inside the goal's own quantisation bucket.
    let bucket_x = (goal.x / CELL).round() * CELL;
    let bucket_z = (goal.z / CELL).round() * CELL;
    let nudged = Vec3::new(bucket_x + 0.2, 0.0, bucket_z + 0.2);
    assert!(
        cache.find(nudged, CELL).is_some(),
        "a goal a few centimetres away must reuse the field"
    );
    assert_eq!(cache.stats().1, 1);
}

#[test]
fn a_degenerate_extent_still_builds_one_cell() {
    let streamer = streamer_at(8, Vec3::ZERO);
    let goal = flat_area(&streamer).expect("a flat area");
    let field = FlowField::build(&streamer, goal, 0.0, 0.0).expect("a minimal field");
    assert_eq!(field.cell_size(), 2.0, "a broken cell size falls back");
    assert!(field.side() >= 1);
    assert!(field.contains(goal));
}
