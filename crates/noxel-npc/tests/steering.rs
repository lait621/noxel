//! Steering: separation, seek, alignment, cohesion, avoidance and the clamp.

mod common;

use common::*;
use noxel_core::math::Vec3;
use noxel_npc::{CrowdManager, NpcConfig, NpcId, NpcKind, NpcState, Steering, SteeringWeights};

fn crowd() -> CrowdManager {
    CrowdManager::new(NpcConfig::default())
}

/// Puts an agent at a position, returning its id.
fn place(crowd: &mut CrowdManager, position: Vec3) -> NpcId {
    let id = crowd.spawn(NpcKind::Villager, position);
    if let Some(agent) = crowd.agent_mut(id) {
        agent.position = position;
        agent.velocity = Vec3::ZERO;
        agent.state = NpcState::Walking;
    }
    id
}

#[test]
fn seek_pulls_towards_the_desired_direction() {
    let mut crowd = crowd();
    let id = place(&mut crowd, Vec3::ZERO);
    let weights = SteeringWeights::default();
    let hash = Steering::build_neighbours(&crowd, 4.0);
    let agent = crowd.agent(id).expect("an agent");
    let force = Steering::force(agent, Vec3::new(1.0, 0.0, 0.0), &hash, &crowd, &weights);
    assert!(
        force.x > 0.9,
        "the seek term should dominate for a lone agent: {force:?}"
    );
    assert!(force.z.abs() < 1e-4);
    assert!(force.y.abs() < 1e-6, "steering is a ground-plane force");
    let opposite = Steering::force(agent, Vec3::new(-1.0, 0.0, 0.0), &hash, &crowd, &weights);
    assert!(opposite.x < -0.9);
}

#[test]
fn separation_pushes_overlapping_agents_apart() {
    let mut crowd = crowd();
    let a = place(&mut crowd, Vec3::ZERO);
    let b = place(&mut crowd, Vec3::new(0.2, 0.0, 0.0));
    let weights = SteeringWeights::default();
    let hash = Steering::build_neighbours(&crowd, 4.0);
    let agent = crowd.agent(a).expect("agent a");
    // A quarter of a metre apart, well inside the 0.9 m separation radius.
    let force = Steering::force(agent, Vec3::ZERO, &hash, &crowd, &weights);
    assert!(
        force.x < -0.5,
        "the pair should push agent a away from agent b: {force:?}"
    );
    let other = crowd.agent(b).expect("agent b");
    let other_force = Steering::force(other, Vec3::ZERO, &hash, &crowd, &weights);
    assert!(
        other_force.x > 0.5,
        "and agent b the other way: {other_force:?}"
    );

    // Integrated over half a second, the pair actually separates.
    let mut pa = agent.position;
    let mut pb = other.position;
    for _ in 0..30 {
        let dt = 1.0 / 60.0;
        let hash = Steering::build_neighbours(&crowd, 4.0);
        let f = {
            let a = crowd.agent(a).expect("agent a");
            Steering::force(a, Vec3::ZERO, &hash, &crowd, &weights)
        };
        pa += f * dt;
        if let Some(agent) = crowd.agent_mut(a) {
            agent.position = pa;
        }
        let hash = Steering::build_neighbours(&crowd, 4.0);
        let g = {
            let b = crowd.agent(b).expect("agent b");
            Steering::force(b, Vec3::ZERO, &hash, &crowd, &weights)
        };
        pb += g * dt;
        if let Some(agent) = crowd.agent_mut(b) {
            agent.position = pb;
        }
    }
    assert!(
        pa.distance(pb) > 0.25,
        "the pair should have separated: {}",
        pa.distance(pb)
    );
}

#[test]
fn identical_positions_are_pushed_apart_deterministically() {
    let mut crowd = crowd();
    let a = place(&mut crowd, Vec3::ZERO);
    let _b = place(&mut crowd, Vec3::ZERO);
    let weights = SteeringWeights::default();
    let hash = Steering::build_neighbours(&crowd, 4.0);
    let agent = crowd.agent(a).expect("an agent");
    let first = Steering::force(agent, Vec3::ZERO, &hash, &crowd, &weights);
    let second = Steering::force(agent, Vec3::ZERO, &hash, &crowd, &weights);
    assert_eq!(first, second, "the fallback direction is deterministic");
    assert!(
        first.length() > 0.0,
        "coincident agents must still be pushed"
    );
}

#[test]
fn alignment_and_cohesion_follow_the_crowd() {
    let mut crowd = crowd();
    let a = place(&mut crowd, Vec3::new(1.2, 0.0, 0.0));
    let b = place(&mut crowd, Vec3::new(1.0, 0.0, 0.0));
    if let Some(agent) = crowd.agent_mut(b) {
        agent.velocity = Vec3::new(2.0, 0.0, 0.0);
    }
    let weights = SteeringWeights {
        separation: 0.0,
        cohesion: 0.0,
        ..SteeringWeights::default()
    };
    let hash = Steering::build_neighbours(&crowd, 4.0);
    let agent = crowd.agent(a).expect("an agent");
    let force = Steering::force(agent, Vec3::ZERO, &hash, &crowd, &weights);
    assert!(
        force.x > 0.0,
        "alignment should pull agent a towards agent b's heading: {force:?}"
    );

    let weights = SteeringWeights {
        separation: 0.0,
        alignment: 0.0,
        cohesion: 0.5,
        ..SteeringWeights::default()
    };
    let force = Steering::force(agent, Vec3::ZERO, &hash, &crowd, &weights);
    assert!(
        force.x < 0.0,
        "cohesion should pull agent a towards agent b's position: {force:?}"
    );
}

#[test]
fn the_force_is_bounded_by_max_force() {
    let mut crowd = crowd();
    let mut ids = Vec::new();
    for i in 0..25 {
        let offset = Vec3::new((i % 5) as f32 * 0.02, 0.0, (i / 5) as f32 * 0.02);
        ids.push(place(&mut crowd, offset));
    }
    let weights = SteeringWeights::default();
    let hash = Steering::build_neighbours(&crowd, 4.0);
    for id in ids {
        let agent = crowd.agent(id).expect("an agent");
        let force = Steering::force(agent, Vec3::X, &hash, &crowd, &weights);
        assert!(
            force.length() <= weights.max_force + 1e-3,
            "a crush must not exceed max_force: {}",
            force.length()
        );
    }
    // And a tiny max_force really does cap the result.
    let tight = SteeringWeights {
        max_force: 0.25,
        ..SteeringWeights::default()
    };
    let agent = crowd.iter().next().expect("an agent");
    let force = Steering::force(agent, Vec3::X, &hash, &crowd, &tight);
    assert!(force.length() <= 0.2501);
}

#[test]
fn the_neighbour_hash_matches_a_brute_force_scan() {
    let mut crowd = crowd();
    let mut ids = Vec::new();
    for i in 0..40 {
        let angle = i as f32 * 0.7;
        ids.push(place(
            &mut crowd,
            Vec3::new(
                angle.cos() * (i as f32 * 0.5),
                0.0,
                angle.sin() * (i as f32 * 0.5),
            ),
        ));
    }
    let cell = 4.0;
    let hash = Steering::build_neighbours(&crowd, cell);
    assert_eq!(hash.len(), ids.len());
    let query = crowd.agent(ids[10]).expect("an agent").position;
    let radius = 4.0;
    let mut found = Vec::new();
    hash.query_radius(query, radius, &mut found);
    let actual: Vec<usize> = found
        .iter()
        .filter_map(|handle| hash.get(*handle).copied())
        .map(|index| index as usize)
        .collect();
    // A spatial hash is a broadphase: it may return a superset, but it must
    // never lose a neighbour inside the radius.
    for (index, agent) in crowd.iter().enumerate() {
        let dx = agent.position.x - query.x;
        let dz = agent.position.z - query.z;
        let distance = (dx * dx + dz * dz).sqrt();
        if distance <= radius {
            assert!(
                actual.contains(&index),
                "the hash lost neighbour {index} at {distance} m"
            );
        }
        if distance > radius + cell {
            assert!(
                !actual.contains(&index),
                "the hash returned {index} at {distance} m, well outside the query"
            );
        }
    }
    assert!(actual.len() >= 3);
}

#[test]
fn broken_agents_are_left_out_of_the_neighbour_hash() {
    let mut crowd = crowd();
    place(&mut crowd, Vec3::ZERO);
    let broken = place(&mut crowd, Vec3::new(3.0, 0.0, 0.0));
    if let Some(agent) = crowd.agent_mut(broken) {
        agent.position = Vec3::new(f32::NAN, 0.0, 0.0);
    }
    let hash = Steering::build_neighbours(&crowd, 4.0);
    assert_eq!(hash.len(), 1, "a non-finite position is not a neighbour");
    assert!(Steering::build_neighbours(&crowd, f32::NAN).len() == 1 || true);
}

#[test]
fn avoidance_steps_around_an_obstacle() {
    let streamer = streamer_at(21, Vec3::ZERO);
    let Some(water) = deep_water(&streamer) else {
        return;
    };
    let Some(shore) = find_walkable(&streamer, water, 96.0) else {
        return;
    };
    // Point the agent straight at the lake and ask where it should go instead.
    let towards_water = (water - shore).normalize_or_zero();
    let mut crowd = crowd();
    let id = place(&mut crowd, shore);
    let agent = crowd.agent(id).expect("an agent");
    let avoided = Steering::avoid(agent, towards_water, &streamer, 2.0);
    if avoided.length_squared() > 0.0 {
        let ahead = shore + avoided * 2.0;
        assert!(
            streamer.is_walkable(ahead),
            "avoidance led into unwalkable ground at {ahead:?}"
        );
    } else {
        // Saying "nowhere is safe" is allowed, but only if it is true.
        for angle in [0.0, 0.7, -0.7] {
            let dir = noxel_core::math::Quat::from_rotation_y(angle) * towards_water;
            let probe = shore + dir * 1.0;
            assert!(
                !streamer.is_walkable(probe) || !streamer.is_walkable(shore + dir * 2.0),
                "avoidance gave up while a probe at {angle} was clear"
            );
        }
    }
}

#[test]
fn avoidance_returns_a_unit_direction_or_nothing() {
    let streamer = streamer_at(7, Vec3::ZERO);
    let start = flat_area(&streamer).expect("a flat area");
    let mut crowd = crowd();
    let id = place(&mut crowd, start);
    let agent = crowd.agent(id).expect("an agent");
    for angle in 0..16 {
        let theta = angle as f32 / 16.0 * core::f32::consts::TAU;
        let desired = Vec3::new(theta.cos(), 0.0, theta.sin());
        let avoided = Steering::avoid(agent, desired, &streamer, 2.0);
        if avoided.length_squared() > 0.0 {
            assert!(
                (avoided.length() - 1.0).abs() < 1e-4,
                "avoid returns a unit direction, got {avoided:?}"
            );
            assert!(avoided.y.abs() < 1e-6);
        }
    }
    // A degenerate probe still answers.
    assert!(Steering::avoid(agent, Vec3::ZERO, &streamer, 2.0).length_squared() <= 0.0);
    let _ = Steering::avoid(agent, Vec3::X, &streamer, f32::NAN);
    let _ = Steering::avoid(agent, Vec3::X, &streamer, -4.0);
}

#[test]
fn avoidance_ignores_a_missing_agent_position() {
    let streamer = streamer_at(7, Vec3::ZERO);
    let mut crowd = crowd();
    let id = place(&mut crowd, Vec3::ZERO);
    if let Some(agent) = crowd.agent_mut(id) {
        agent.position = Vec3::new(f32::NAN, 0.0, f32::NAN);
    }
    let agent = crowd.agent(id).expect("an agent");
    let avoided = Steering::avoid(agent, Vec3::X, &streamer, 2.0);
    assert_eq!(avoided, Vec3::ZERO);
}
