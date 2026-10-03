//! A\* against a real streamed world: real tiles, real water, real cliffs.

mod common;

use common::*;
use noxel_core::math::Vec3;
use noxel_npc::path::{PathConfig, is_traversable};
use noxel_npc::{Path, Pathfinder};

fn finder() -> Pathfinder {
    Pathfinder::new(PathConfig {
        max_nodes: 20_000,
        max_iterations: 40_000,
        ..PathConfig::default()
    })
}

/// Every point along a path is ground an agent may stand on.
fn assert_path_traversable(path: &Path, streamer: &noxel_world::WorldStreamer) {
    let config = PathConfig::default();
    let samples = sample_path(&path.waypoints, 1.0, path.waypoints[0]);
    for point in samples {
        assert!(
            is_traversable(streamer, &config, point),
            "path leaves traversable ground at {point:?}"
        );
    }
}

#[test]
fn a_path_across_open_ground_exists() {
    let streamer = streamer_at(7, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let mut pathfinder = finder();
    // Walk 30 m towards the middle of the region.
    let from = open;
    let to = open + Vec3::new(30.0, 0.0, 0.0);
    let Some(goal) = find_ground(&streamer, to) else {
        // The 30 m sample landed in water; try the other axis.
        let to = open + Vec3::new(0.0, 0.0, 30.0);
        let goal = find_ground(&streamer, to).expect("some open ground nearby");
        let path = pathfinder
            .find_path(&streamer, from, goal)
            .expect("a path across open ground");
        assert!(!path.is_empty());
        return;
    };
    let path = pathfinder
        .find_path(&streamer, from, goal)
        .expect("a path across open ground");
    assert!(!path.is_empty(), "a 30 m walk needs waypoints");
    assert!(path.complete, "the goal is reachable");
    assert!(path.cost > 0.0);
    assert_path_traversable(&path, &streamer);
}

/// The nearest standable ground to `p`, wherever the caller asked for.
fn find_ground(streamer: &noxel_world::WorldStreamer, p: Vec3) -> Option<Vec3> {
    find_walkable(streamer, p, 24.0)
}

#[test]
fn the_path_is_walkable_end_to_end() {
    let streamer = streamer_at(11, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let mut pathfinder = finder();
    let mut found = 0;
    for (dx, dz) in [(24.0, 0.0), (0.0, 24.0), (-20.0, -12.0), (16.0, -18.0)] {
        let Some(goal) = find_ground(&streamer, open + Vec3::new(dx, 0.0, dz)) else {
            continue;
        };
        if goal.distance(open) < 8.0 {
            continue;
        }
        let Some(path) = pathfinder.find_path(&streamer, open, goal) else {
            continue;
        };
        found += 1;
        assert_path_traversable(&path, &streamer);
        for waypoint in &path.waypoints {
            assert!(
                is_loaded(&streamer, *waypoint),
                "waypoint {waypoint:?} is outside the streamed region"
            );
        }
    }
    assert!(found >= 2, "expected several usable routes, found {found}");
}

#[test]
fn the_same_request_twice_is_identical() {
    let streamer = streamer_at(3, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let goal = find_ground(&streamer, open + Vec3::new(26.0, 0.0, 8.0)).expect("a goal");
    let mut pathfinder = finder();
    let first = pathfinder
        .find_path(&streamer, open, goal)
        .expect("first path");
    pathfinder.clear_cache();
    let second = pathfinder
        .find_path(&streamer, open, goal)
        .expect("second path");
    assert_eq!(first.waypoints, second.waypoints);
    assert_eq!(first.cost, second.cost);
    assert_eq!(first.complete, second.complete);
}

#[test]
fn two_pathfinders_agree_bit_for_bit() {
    let streamer = streamer_at(5, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let goal = find_ground(&streamer, open + Vec3::new(-22.0, 0.0, 17.0)).expect("a goal");
    let mut a = finder();
    let mut b = finder();
    let first = a.find_path(&streamer, open, goal);
    let second = b.find_path(&streamer, open, goal);
    match (first, second) {
        (Some(first), Some(second)) => {
            assert_eq!(first.waypoints, second.waypoints);
            assert_eq!(first.cost.to_bits(), second.cost.to_bits());
        }
        (None, None) => {}
        other => panic!("pathfinders disagreed: {other:?}"),
    }
}

#[test]
fn a_goal_outside_the_streamed_region_is_refused_or_clamped() {
    let streamer = streamer_at(9, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let mut pathfinder = finder();
    let far = open + Vec3::new(5_000.0, 0.0, 5_000.0);
    match pathfinder.find_path(&streamer, open, far) {
        None => {}
        Some(path) => {
            assert!(!path.complete, "a clamped goal cannot be a complete path");
            for waypoint in &path.waypoints {
                assert!(
                    is_loaded(&streamer, *waypoint),
                    "clamped path left the streamed region at {waypoint:?}"
                );
            }
        }
    }
}

#[test]
fn a_start_outside_the_streamed_region_is_refused() {
    let streamer = streamer_at(9, Vec3::ZERO);
    let mut pathfinder = finder();
    let outside = Vec3::new(9_000.0, 0.0, 9_000.0);
    assert!(
        pathfinder
            .find_path(&streamer, outside, Vec3::ZERO)
            .is_none(),
        "a search may not start in an unloaded chunk"
    );
    assert!(
        pathfinder
            .find_path(&streamer, Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO)
            .is_none()
    );
    assert!(
        pathfinder
            .find_path(&streamer, Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0))
            .is_none()
    );
}

#[test]
fn a_path_never_crosses_deep_water() {
    let streamer = streamer_at(21, Vec3::ZERO);
    let Some((west, east)) = shores_across_water(&streamer) else {
        // No lake in this world: the property is vacuous, but say so.
        assert!(deep_water(&streamer).is_none(), "water helper disagrees");
        return;
    };
    assert!(deep_water(&streamer).is_some(), "a lake was found");
    let mut pathfinder = finder();
    match pathfinder.find_path(&streamer, west, east) {
        None => {}
        Some(path) => {
            assert_path_traversable(&path, &streamer);
            for waypoint in &path.waypoints {
                assert!(
                    is_traversable(&streamer, &PathConfig::default(), *waypoint),
                    "the path swims through {waypoint:?}"
                );
            }
        }
    }
}

#[test]
fn a_goal_in_deep_water_is_refused() {
    let streamer = streamer_at(21, Vec3::ZERO);
    let Some(water) = deep_water(&streamer) else {
        return;
    };
    let Some(shore) = find_walkable(&streamer, water, 64.0) else {
        return;
    };
    if shore.distance(water) < 12.0 {
        // The "deep" sample turned out to be a puddle next to the shore.
        return;
    }
    let mut pathfinder = finder();
    match pathfinder.find_path(&streamer, shore, water) {
        None => {}
        Some(path) => {
            assert!(
                !path.complete,
                "an agent cannot stand in the middle of a lake"
            );
            for waypoint in &path.waypoints {
                assert!(
                    is_traversable(&streamer, &PathConfig::default(), *waypoint),
                    "partial path entered the water at {waypoint:?}"
                );
            }
        }
    }
}

#[test]
fn simplify_shortens_a_staircase_and_stays_on_walkable_ground() {
    let streamer = streamer_at(13, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    // A staircase of 2 m steps, exactly what A* produces on a grid.
    let mut waypoints = Vec::new();
    for step in 1..=10 {
        waypoints.push(open + Vec3::new(step as f32 * 2.0, 0.0, step as f32 * 2.0));
    }
    let path = Path {
        waypoints,
        cost: 40.0,
        complete: true,
    };
    let simplified = path.simplify(1.5);
    assert!(
        simplified.len() < path.len(),
        "simplification must remove waypoints: {} -> {}",
        path.len(),
        simplified.len()
    );
    assert!(simplified.len() >= 2);
    let samples = sample_path(&simplified.waypoints, 1.0, open);
    let config = PathConfig::default();
    for point in samples {
        assert!(
            is_traversable(&streamer, &config, point),
            "the simplified route leaves walkable ground at {point:?}"
        );
    }
}

#[test]
fn the_cache_serves_a_repeat_request() {
    let streamer = streamer_at(4, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let goal = find_ground(&streamer, open + Vec3::new(18.0, 0.0, 12.0)).expect("a goal");
    let mut pathfinder = finder();
    let first = pathfinder.find_path(&streamer, open, goal).expect("a path");
    let after_first = pathfinder.stats().paths_computed;
    let second = pathfinder.find_path(&streamer, open, goal).expect("a path");
    assert_eq!(first.waypoints, second.waypoints);
    assert_eq!(
        pathfinder.stats().paths_computed,
        after_first,
        "a cache hit must not run a search"
    );
    assert!(pathfinder.cache_len() >= 1);
    pathfinder.clear_cache();
    assert_eq!(pathfinder.cache_len(), 0);
    let third = pathfinder.find_path(&streamer, open, goal).expect("a path");
    assert_eq!(first.waypoints, third.waypoints);
    assert!(pathfinder.stats().paths_computed > after_first);
}

#[test]
fn changing_the_config_drops_the_cache() {
    let streamer = streamer_at(4, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let goal = find_ground(&streamer, open + Vec3::new(16.0, 0.0, 10.0)).expect("a goal");
    let mut pathfinder = finder();
    assert!(pathfinder.find_path(&streamer, open, goal).is_some());
    assert!(pathfinder.cache_len() > 0);
    pathfinder.set_config(PathConfig {
        node_size: 3.0,
        ..PathConfig::default()
    });
    assert_eq!(
        pathfinder.cache_len(),
        0,
        "a new grid invalidates old routes"
    );
    assert_eq!(pathfinder.config().node_size, 3.0);
}

#[test]
fn a_blocked_goal_yields_no_path_or_a_partial_one() {
    let streamer = streamer_at(31, Vec3::ZERO);
    let Some(water) = deep_water(&streamer) else {
        return;
    };
    let Some(shore) = find_walkable(&streamer, water, 64.0) else {
        return;
    };
    let mut pathfinder = finder();
    let path = pathfinder.find_path(&streamer, shore, water);
    if let Some(path) = path {
        assert!(!path.complete);
        assert!(path.len() < 200);
    }
}

#[test]
fn a_short_hop_is_a_direct_step() {
    let streamer = streamer_at(17, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let mut pathfinder = finder();
    let goal = open + Vec3::new(1.0, 0.0, 0.5);
    let path = pathfinder
        .find_path(&streamer, open, goal)
        .expect("a one-metre walk");
    assert!(path.cost < 6.0);
    assert!(path.len() <= 3, "a short hop needs almost no waypoints");
    let last = path.next_waypoint(path.len() - 1).expect("a waypoint");
    assert_eq!(
        path.advance(path.len() - 1, last, 0.6),
        path.len(),
        "arriving at the last waypoint finishes the path"
    );
}

#[test]
fn stats_count_searches_and_nodes() {
    let streamer = streamer_at(23, Vec3::ZERO);
    let open = flat_area(&streamer).expect("a flat area");
    let mut pathfinder = finder();
    assert_eq!(pathfinder.stats().paths_computed, 0);
    assert_eq!(pathfinder.stats().path_nodes_expanded, 0);
    for (dx, dz) in [(20.0, 6.0), (-16.0, 14.0), (9.0, -21.0)] {
        if let Some(goal) = find_ground(&streamer, open + Vec3::new(dx, 0.0, dz)) {
            let _ = pathfinder.find_path(&streamer, open, goal);
        }
    }
    let stats = pathfinder.stats();
    assert!(stats.paths_computed >= 1);
    assert!(stats.path_nodes_expanded >= stats.paths_computed);
    assert!(pathfinder.memory_bytes() > 0);
}
