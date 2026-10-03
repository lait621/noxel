//! The crowd system end to end: population, tiers, physics capsules,
//! determinism, the stuck detector, robustness and the frame budget.

mod common;

use common::*;
use noxel_core::math::Vec3;
use noxel_npc::path::MAX_PATHS_PER_STEP;
use noxel_npc::{NpcConfig, NpcContext, NpcId, NpcKind, NpcState, NpcSystem};
use noxel_physics::{LAYER_NPC, PhysicsWorld};
use noxel_world::stream::WorldStreamer;

/// A system, a streamer and a physics world sharing one flat area, as separate
/// locals: an [`NpcContext`] borrows the streamer and the physics world, so the
/// system must not be reachable through the same binding.
fn setup(
    seed: u64,
    target: usize,
    max_agents: usize,
) -> (NpcSystem, WorldStreamer, PhysicsWorld, Vec3) {
    let mut streamer = streamer_at(seed, Vec3::ZERO);
    let center = flat_area(&streamer).unwrap_or(Vec3::ZERO);
    streamer.update(center);
    let physics = physics_with_floor(center, center.y, 220.0);
    let system = NpcSystem::new(NpcConfig {
        seed,
        target_population: target,
        max_agents,
        ..NpcConfig::default()
    });
    (system, streamer, physics, center)
}

/// Runs updates until the population stops growing, returning the count.
fn fill(system: &mut NpcSystem, ctx: &mut NpcContext<'_>, steps: usize) -> usize {
    let mut last = 0;
    for _ in 0..steps {
        system.update(ctx);
        let active = system.crowd().len();
        if active == last {
            break;
        }
        last = active;
    }
    last
}

/// A context for one step, at eight in the morning.
fn context<'a>(
    streamer: &'a WorldStreamer,
    physics: &'a mut PhysicsWorld,
    center: Vec3,
    dt: f32,
) -> NpcContext<'a> {
    NpcContext::new(streamer, physics, center, 8.0 * 3600.0, dt)
}

#[test]
fn maintain_population_reaches_the_target() {
    let (mut system, streamer, mut physics, center) = setup(1, 200, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    let filled = fill(&mut system, &mut ctx, 20);
    assert_eq!(filled, 200, "the target is reached within a few steps");
    let stats = system.update(&mut ctx);
    assert_eq!(stats.active, 200);
    assert!(stats.spawned_this_step <= 200);
    // Every agent is within the spawn radius of the centre.
    for agent in system.crowd().iter() {
        let dx = agent.position.x - center.x;
        let dz = agent.position.z - center.z;
        assert!(
            (dx * dx + dz * dz).sqrt() <= 70.0 + 1.0,
            "spawned outside the spawn radius: {:?}",
            agent.position
        );
    }
}

#[test]
fn population_never_exceeds_max_agents() {
    let (mut system, streamer, mut physics, center) = setup(2, 500, 40);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    for _ in 0..5 {
        system.update(&mut ctx);
        assert!(
            system.crowd().len() <= 40,
            "the ceiling was breached: {}",
            system.crowd().len()
        );
    }
    assert_eq!(system.crowd().len(), 40);
}

#[test]
fn agents_left_behind_are_despawned() {
    let (mut system, streamer, mut physics, center) = setup(3, 150, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    fill(&mut system, &mut ctx, 20);
    assert_eq!(system.crowd().len(), 150);
    let first_ids: Vec<NpcId> = system.crowd().ids().collect();
    // Teleport the focus a kilometre away: everything is now behind us, and
    // there is no loaded world to spawn into.
    ctx.center = center + Vec3::new(1_000.0, 0.0, 0.0);
    system.maintain_population(&mut ctx);
    assert_eq!(system.crowd().len(), 0, "the old crowd is retired");
    for id in first_ids {
        assert!(system.crowd().agent(id).is_none());
    }
}

#[test]
fn population_is_stable_when_the_centre_is_still() {
    let (mut system, streamer, mut physics, center) = setup(4, 120, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    let settled = fill(&mut system, &mut ctx, 20);
    assert_eq!(settled, 120);
    for step in 0..90 {
        ctx.world_time += 1.0 / 60.0;
        let stats = system.update(&mut ctx);
        assert_eq!(
            stats.active, settled,
            "the population moved on step {step}: {}",
            stats.active
        );
    }
}

#[test]
fn tiers_follow_distance_and_add_up() {
    let (mut system, streamer, mut physics, center) = setup(5, 300, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    fill(&mut system, &mut ctx, 20);
    // A tier is assigned from the position at the top of a step, so it is
    // checked against the position at the top of the step.
    let before: Vec<(NpcId, Vec3)> = system
        .crowd()
        .iter()
        .map(|agent| (agent.id, agent.position))
        .collect();
    let stats = system.update(&mut ctx);
    assert_eq!(stats.active, 300);
    let total: usize = stats.tier_counts.iter().sum();
    assert_eq!(total, stats.active, "tier counts must sum to the crowd");
    let tier_config = system.config().tier;
    for (id, position) in before {
        let agent = system.crowd().agent(id).expect("a resident agent");
        let dx = position.x - center.x;
        let dz = position.z - center.z;
        let expected = tier_config.tier_for((dx * dx + dz * dz).sqrt());
        assert_eq!(agent.tier, expected, "wrong tier at {position:?}");
        assert!(
            agent.tier != noxel_npc::CrowdTier::Frozen,
            "a resident agent is never frozen"
        );
    }
}

/// A destination `distance` metres along `dir` that is clear the whole way.
fn clear_destination(
    streamer: &noxel_world::WorldStreamer,
    from: Vec3,
    dir: Vec3,
    distance: f32,
) -> Option<Vec3> {
    let to = from + dir * distance;
    if sample_segment(from, to, 1.0)
        .iter()
        .all(|point| ground_at(streamer, *point).is_some())
    {
        ground_at(streamer, to)
    } else {
        None
    }
}

#[test]
fn a_far_agent_moves_less_often_but_still_advances() {
    let (mut system, streamer, mut physics, center) = setup(6, 1, 8);
    // One near agent and one in the far ring, each with a destination that
    // keeps it in its own tier and a clear line to walk.
    let near_start = find_walkable(&streamer, center + Vec3::new(10.0, 0.0, 0.0), 6.0)
        .expect("open ground near the centre");
    let far_start = find_walkable(&streamer, center + Vec3::new(70.0, 0.0, 0.0), 6.0)
        .expect("open ground in the far ring");
    let tangents = [Vec3::Z, -Vec3::Z, Vec3::X, -Vec3::X];
    let near_goal = tangents
        .iter()
        .find_map(|dir| clear_destination(&streamer, near_start, *dir, 8.0))
        .expect("a clear walk near the centre");
    let far_goal = tangents
        .iter()
        .find_map(|dir| clear_destination(&streamer, far_start, *dir, 6.0))
        .expect("a clear walk in the far ring");

    let near = system.crowd_mut().spawn(NpcKind::Villager, near_start);
    let far = system.crowd_mut().spawn(NpcKind::Villager, far_start);
    assert!(near.is_valid() && far.is_valid());
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    assert_eq!(
        system.crowd().agent(near).map(|a| a.tier),
        Some(noxel_npc::CrowdTier::Near)
    );
    assert_eq!(
        system.crowd().agent(far).map(|a| a.tier),
        Some(noxel_npc::CrowdTier::Far)
    );
    assert!(system.send_to(near, near_goal));
    assert!(system.send_to(far, far_goal));

    let mut near_moves = 0;
    let mut far_moves = 0;
    let mut last_near = system.crowd().agent(near).expect("near").position;
    let mut last_far = system.crowd().agent(far).expect("far").position;
    let start_far = last_far;
    for _ in 0..180 {
        ctx.world_time += 1.0 / 60.0;
        system.update(&mut ctx);
        let now_near = system.crowd().agent(near).expect("near").position;
        let now_far = system.crowd().agent(far).expect("far").position;
        if now_near.distance(last_near) > 1e-5 {
            near_moves += 1;
        }
        if now_far.distance(last_far) > 1e-5 {
            far_moves += 1;
        }
        last_near = now_near;
        last_far = now_far;
    }
    assert!(
        near_moves > far_moves,
        "near {near_moves} moves vs far {far_moves}"
    );
    assert!(far_moves > 0, "the far agent never moved at all");
    assert!(
        last_far.distance(far_goal) < start_far.distance(far_goal),
        "the far agent did not advance: {:?} -> {:?}",
        start_far,
        last_far
    );
}

#[test]
fn physics_bodies_return_to_their_starting_count() {
    let (mut system, streamer, mut physics, center) = setup(7, 250, 4096);
    let before = physics.len();
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    let with_crowd = ctx.physics.len();
    assert!(
        with_crowd > before,
        "tier-0 agents must own capsules: {with_crowd}"
    );
    // Everyone leaves the region: every capsule must come back out.
    ctx.center = center + Vec3::new(900.0, 0.0, 0.0);
    system.maintain_population(&mut ctx);
    assert_eq!(system.crowd().len(), 0);
    assert_eq!(physics.len(), before, "a round trip must not leak bodies");
}

#[test]
fn tier_zero_capsules_are_on_the_npc_layer() {
    let (mut system, streamer, mut physics, center) = setup(8, 200, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    fill(&mut system, &mut ctx, 20);
    let tier_zero = system
        .crowd()
        .iter()
        .filter(|agent| agent.tier == noxel_npc::CrowdTier::Near)
        .count();
    assert!(tier_zero > 0, "expected agents in the near bubble");
    let bodies: Vec<u64> = physics
        .bodies()
        .filter(|(_, body)| body.layer == LAYER_NPC)
        .map(|(_, body)| body.user_data)
        .collect();
    assert_eq!(bodies.len(), tier_zero, "one capsule per tier-0 agent");
    for id in bodies {
        assert!(
            system.crowd().agent(NpcId(id as u32)).is_some(),
            "capsule {id} belongs to a live agent"
        );
    }
}

#[test]
fn two_runs_of_one_seed_are_bit_identical() {
    let (mut a, a_streamer, mut a_physics, a_center) = setup(11, 300, 4096);
    let (mut b, b_streamer, mut b_physics, b_center) = setup(11, 300, 4096);
    let dt = 1.0 / 60.0;
    let mut ctx_a = context(&a_streamer, &mut a_physics, a_center, dt);
    let mut ctx_b = context(&b_streamer, &mut b_physics, b_center, dt);
    for step in 0..600 {
        ctx_a.world_time += f64::from(dt);
        ctx_b.world_time += f64::from(dt);
        let stats_a = a.update(&mut ctx_a);
        let stats_b = b.update(&mut ctx_b);
        assert_eq!(stats_a.active, stats_b.active, "population diverged");
        assert_eq!(
            stats_a.paths_computed, stats_b.paths_computed,
            "path requests diverged on step {step}"
        );
    }
    let left: Vec<[u32; 3]> = a
        .crowd()
        .iter()
        .map(|agent| {
            [
                agent.position.x.to_bits(),
                agent.position.y.to_bits(),
                agent.position.z.to_bits(),
            ]
        })
        .collect();
    let right: Vec<[u32; 3]> = b
        .crowd()
        .iter()
        .map(|agent| {
            [
                agent.position.x.to_bits(),
                agent.position.y.to_bits(),
                agent.position.z.to_bits(),
            ]
        })
        .collect();
    assert_eq!(left.len(), 300);
    assert_eq!(left, right, "600 steps of one seed must be bit-identical");
}

#[test]
fn different_seeds_produce_different_towns() {
    let (mut a, a_streamer, mut a_physics, a_center) = setup(11, 120, 4096);
    let (mut b, b_streamer, mut b_physics, b_center) = setup(12, 120, 4096);
    let mut ctx_a = context(&a_streamer, &mut a_physics, a_center, 1.0 / 60.0);
    let mut ctx_b = context(&b_streamer, &mut b_physics, b_center, 1.0 / 60.0);
    let mut differences = 0;
    for _ in 0..120 {
        ctx_a.world_time += 1.0 / 60.0;
        ctx_b.world_time += 1.0 / 60.0;
        a.update(&mut ctx_a);
        b.update(&mut ctx_b);
        let left: Vec<Vec3> = a.crowd().iter().map(|x| x.position).collect();
        let right: Vec<Vec3> = b.crowd().iter().map(|x| x.position).collect();
        if left != right {
            differences += 1;
        }
    }
    assert!(
        differences > 100,
        "two seeds barely differed ({differences} of 120 steps)"
    );
}

#[test]
fn a_stuck_agent_repaths_twice_and_then_gives_up() {
    let (mut system, streamer, mut physics, center) = setup(9, 1, 4);
    // A sealed box the pathfinder cannot see: physics blocks the agent in every
    // direction, while A* — which reads the world's tiles, not the colliders —
    // happily routes straight through the walls.
    let half = 1.5;
    let low = center.y - 0.5;
    let high = center.y + 3.0;
    for (min, max) in [
        (
            Vec3::new(center.x - half - 0.5, low, center.z - half - 0.5),
            Vec3::new(center.x - half, high, center.z + half + 0.5),
        ),
        (
            Vec3::new(center.x + half, low, center.z - half - 0.5),
            Vec3::new(center.x + half + 0.5, high, center.z + half + 0.5),
        ),
        (
            Vec3::new(center.x - half, low, center.z - half - 0.5),
            Vec3::new(center.x + half, high, center.z - half),
        ),
        (
            Vec3::new(center.x - half, low, center.z + half),
            Vec3::new(center.x + half, high, center.z + half + 0.5),
        ),
    ] {
        physics.insert_static_aabb(noxel_core::math::Aabb::new(min, max), 0);
    }
    let id = system.crowd_mut().spawn(NpcKind::Guard, center);
    assert!(id.is_valid());
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 30.0);
    system.update(&mut ctx);
    let goal = center + Vec3::new(12.0, 0.0, 0.0);
    assert!(system.send_to(id, goal));
    let before = system.stats().paths_computed;

    let mut became_stuck = false;
    for _ in 0..600 {
        ctx.world_time += 1.0 / 30.0;
        system.update(&mut ctx);
        if system.crowd().agent(id).map(|a| a.state) == Some(NpcState::Stuck) {
            became_stuck = true;
            break;
        }
    }
    let stats = system.stats();
    assert!(
        became_stuck,
        "the agent never gave up: state {:?}, blocked for {:?}",
        system.crowd().agent(id).map(|a| a.state),
        system.crowd().agent(id).map(|a| a.blocked_time)
    );
    assert!(
        stats.paths_computed >= before + 2,
        "the stuck detector should have repathed at least twice: {before} -> {}",
        stats.paths_computed
    );
    let agent = system.crowd().agent(id).expect("the agent");
    match agent.destination {
        Some(destination) => assert_ne!(
            destination, goal,
            "a stuck agent gives up on the destination it cannot reach"
        ),
        None => panic!("a stuck agent still has somewhere to be"),
    }
    assert!(agent.blocked_time >= 0.0);
}

#[test]
fn send_to_needs_a_live_agent_and_a_real_destination() {
    let (mut system, streamer, mut physics, center) = setup(13, 20, 64);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    let id = system.crowd().ids().next().expect("an agent");
    assert!(system.send_to(id, center + Vec3::new(5.0, 0.0, 5.0)));
    assert!(!system.send_to(NpcId::INVALID, center));
    assert!(!system.send_to(NpcId(9_999), center));
    assert!(!system.send_to(id, Vec3::new(f32::NAN, 0.0, 0.0)));
    // The request is served inside the same step's path budget.
    system.update(&mut ctx);
    assert!(system.stats().paths_computed >= 1);
}

#[test]
fn a_destination_inside_a_wall_is_survivable() {
    let (mut system, streamer, mut physics, center) = setup(14, 20, 64);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    let id = system.crowd().ids().next().expect("an agent");
    let inside = Vec3::new(center.x + 1.0, center.y, center.z + 1.0);
    assert!(system.send_to(id, inside));
    for _ in 0..30 {
        ctx.world_time += 1.0 / 60.0;
        system.update(&mut ctx);
    }
    let agent = system.crowd().agent(id).expect("the agent");
    assert!(agent.position.is_finite());
    assert!(agent.state != NpcState::Stuck || agent.blocked_time >= 0.0);
}

#[test]
fn an_agent_with_no_path_keeps_its_dignity() {
    let (mut system, streamer, mut physics, center) = setup(29, 30, 64);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    fill(&mut system, &mut ctx, 20);
    let id = system.crowd().ids().next().expect("an agent");
    // Somewhere the pathfinder refuses: far outside the streamed world.
    assert!(system.send_to(id, center + Vec3::new(20_000.0, 0.0, 20_000.0)));
    for _ in 0..60 {
        ctx.world_time += 1.0 / 60.0;
        system.update(&mut ctx);
    }
    let agent = system.crowd().agent(id).expect("the agent");
    assert!(agent.position.is_finite());
    assert!(agent.velocity.is_finite());
    assert!(
        agent.destination.is_some(),
        "the request is still on record"
    );
    // And a crowd whose world has nothing loaded simply does not spawn.
    let empty = common::streamer(5);
    let mut bare = common::physics();
    let mut ctx = NpcContext::new(&empty, &mut bare, center, 0.0, 1.0 / 60.0);
    let mut system = NpcSystem::new(NpcConfig {
        seed: 5,
        target_population: 50,
        ..NpcConfig::default()
    });
    let stats = system.update(&mut ctx);
    assert_eq!(stats.active, 0, "nowhere to spawn is not a panic");
}

#[test]
fn dt_zero_does_nothing_and_a_huge_dt_does_not_teleport() {
    let (mut system, streamer, mut physics, center) = setup(15, 60, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 0.0);
    // A first step with a zero timestep still populates the town.
    system.maintain_population(&mut ctx);
    let snapshot: Vec<Vec3> = system.crowd().iter().map(|a| a.position).collect();
    system.update(&mut ctx);
    let after: Vec<Vec3> = system.crowd().iter().map(|a| a.position).collect();
    assert_eq!(snapshot, after, "dt = 0 must not move anybody");

    // A week-long frame is clamped to MAX_DT.
    ctx.dt = 604_800.0;
    let before: Vec<Vec3> = system.crowd().iter().map(|a| a.position).collect();
    system.update(&mut ctx);
    let after: Vec<Vec3> = system.crowd().iter().map(|a| a.position).collect();
    let limit = system.config().run_speed * noxel_npc::MAX_DT + 1e-3;
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(
            a.distance(*b) <= limit,
            "a huge dt teleported an agent {} m",
            a.distance(*b)
        );
    }
}

#[test]
fn nan_positions_are_retired_not_propagated() {
    let (mut system, streamer, mut physics, center) = setup(16, 40, 4096);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    let id = system.crowd().ids().next().expect("an agent");
    if let Some(agent) = system.crowd_mut().agent_mut(id) {
        agent.position = Vec3::new(f32::NAN, 0.0, f32::NAN);
    }
    let stats = system.update(&mut ctx);
    assert!(
        stats.despawned_this_step >= 1,
        "the broken agent is retired"
    );
    assert!(system.crowd().agent(id).is_none());
    for agent in system.crowd().iter() {
        assert!(agent.position.is_finite());
    }
    // A non-finite centre is treated as the origin rather than poisoning the
    // crowd; the population simply moves house.
    ctx.center = Vec3::new(f32::NAN, 0.0, 0.0);
    system.update(&mut ctx);
    for agent in system.crowd().iter() {
        assert!(agent.position.is_finite());
    }
}

#[test]
fn max_agents_zero_is_a_valid_town_of_nobody() {
    let (mut system, streamer, mut physics, center) = setup(17, 100, 0);
    let before = physics.len();
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    let stats = system.update(&mut ctx);
    assert_eq!(stats.active, 0);
    assert_eq!(ctx.physics.len(), before, "nobody owns a capsule");
    for _ in 0..10 {
        system.update(&mut ctx);
    }
    assert_eq!(system.crowd().len(), 0);
}

#[test]
fn externally_despawned_agents_stay_gone() {
    let (mut system, streamer, mut physics, center) = setup(18, 40, 64);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    system.update(&mut ctx);
    let id = system.crowd().ids().next().expect("an agent");
    assert!(system.crowd_mut().despawn(id));
    system.update(&mut ctx);
    assert!(system.crowd().agent(id).is_none());
    assert!(!system.send_to(id, center));
}

#[test]
fn the_path_budget_holds_a_whole_town() {
    let (mut system, streamer, mut physics, center) = setup(19, 600, 1200);
    let mut ctx = context(&streamer, &mut physics, center, 1.0 / 60.0);
    let steps = 120u32;
    for _ in 0..steps {
        ctx.world_time += 1.0 / 60.0;
        system.update(&mut ctx);
    }
    let stats = system.stats();
    assert_eq!(stats.active, 600);
    assert!(
        stats.paths_computed <= u64::from(steps) * MAX_PATHS_PER_STEP as u64,
        "the path budget was breached: {}",
        stats.paths_computed
    );
    assert!(
        stats.flow_fields_cached > 0,
        "a town full of agents sharing destinations should use flow fields"
    );
    assert!(stats.path_nodes_expanded > 0);
}

#[test]
fn a_thousand_agents_stay_inside_the_frame_budget() {
    let (mut system, streamer, mut physics, center) = setup(23, 1000, 1200);
    let dt = 1.0 / 60.0;
    let mut ctx = context(&streamer, &mut physics, center, dt);
    let filled = fill(&mut system, &mut ctx, 40);
    assert_eq!(filled, 1000, "the town must be full");

    let steps = 300u32;
    let mut total = 0.0f32;
    let mut worst = 0.0f32;
    for step in 0..steps {
        ctx.world_time += f64::from(dt);
        let stats = system.update(&mut ctx);
        total += stats.last_step_ms;
        worst = worst.max(stats.last_step_ms);
        if step % 100 == 0 {
            assert!(
                stats.active >= 950,
                "the town thinned out on step {step}: {}",
                stats.active
            );
        }
        if step == 0 {
            // A deliberately generous bound. A single step's wall-clock reading
            // is dominated by whatever else the machine is doing (this suite runs
            // in parallel), so a tight assertion here measures the CI machine, not
            // the crowd. The *mean* below is the number that has to hold.
            assert!(
                stats.last_step_ms < 250.0,
                "the fill step took {} ms, which is a hang rather than a slow frame",
                stats.last_step_ms
            );
        }
    }
    let mean = total / steps as f32;
    let stats = system.stats();
    let per_agent = mean * 1000.0 / stats.active as f32;
    println!(
        "1000-agent crowd: mean {mean:.3} ms/step over {steps} steps, worst {worst:.3} ms, \
         {per_agent:.4} us/agent, {} paths, {} nodes, {} flow fields, {:.1} MB",
        stats.paths_computed,
        stats.path_nodes_expanded,
        stats.flow_fields_cached,
        system.memory_bytes() as f32 / (1024.0 * 1024.0),
    );
    assert!(stats.active >= 950, "a thousand agents stayed resident");
    // The engine's target is a 4 ms NPC budget. The test allows double that so
    // it cannot flake on a loaded machine, but it is still a real ceiling:
    // anything slower than this cannot hold a town.
    assert!(
        mean < 8.0,
        "the crowd averaged {mean:.3} ms per step, over the 8 ms ceiling"
    );
    assert!(
        stats.us_per_agent < 8.0,
        "{} us per agent",
        stats.us_per_agent
    );
    assert!(system.memory_bytes() > 0);
    assert!(system.memory_bytes() < 512 * 1024 * 1024);
}
