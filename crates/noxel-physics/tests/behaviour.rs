//! End-to-end behaviour tests.
//!
//! Everything here goes through the public API only, and every asserted number
//! is derived in a comment from the configuration rather than guessed.

use noxel_core::math::{Aabb, Quat, Ray, Vec3};
use noxel_physics::{
    BodyDesc, BodyHandle, BodyKind, CharacterMove, ColliderShape, LAYER_ALL, LAYER_PLAYER,
    LAYER_PROP, LAYER_WORLD, PhysicsConfig, PhysicsEvent, PhysicsWorld, QueryFilter,
};

/// A world with a 20x20 floor whose top surface is y = 0.
fn floor_world() -> PhysicsWorld {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.insert_static_aabb(
        Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)),
        1,
    );
    world
}

fn dynamic_box(world: &mut PhysicsWorld, half: f32, at: Vec3) -> BodyHandle {
    world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(half),
        })
        .at(at),
    )
}

fn run(world: &mut PhysicsWorld, steps: u32) {
    for _ in 0..steps {
        world.step(1.0 / 60.0);
    }
}

#[test]
fn falling_box_rests_on_the_floor() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 5.0, 0.0));
    run(&mut world, 240); // 4 s: a 4.5 m fall takes sqrt(2*4.5/19.62) = 0.68 s.
    let y = world.body(b).unwrap().position.y;
    assert!(
        (y - 0.5).abs() < 0.01,
        "resting centre y = {y}, expected 0.5"
    );
    assert!(world.body(b).unwrap().linear_velocity.length() < 0.05);
}

#[test]
fn resting_box_does_not_sink_over_six_hundred_steps() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.6, 0.0));
    run(&mut world, 120);
    let settled = world.body(b).unwrap().position.y;
    run(&mut world, 600);
    let after = world.body(b).unwrap().position.y;
    assert!(
        (after - settled).abs() < 0.005,
        "drifted from {settled} to {after} over 10 s of rest"
    );
    assert!(after > 0.49, "must not sink below the floor, y = {after}");
    assert!(
        world.body(b).unwrap().sleeping,
        "a resting body should be asleep"
    );
}

#[test]
fn every_collider_rests_at_its_analytic_height() {
    let cases: [(ColliderShape, f32); 4] = [
        // A box of half extent h rests with its centre h above the floor.
        (
            ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            },
            0.5,
        ),
        (ColliderShape::Sphere { radius: 0.5 }, 0.5),
        // Capsule: the bottom cap centre is half_height up, plus the radius.
        (
            ColliderShape::Capsule {
                radius: 0.3,
                half_height: 0.5,
            },
            0.8,
        ),
        // Cylinder: half the height (it collides as its bounding box).
        (
            ColliderShape::Cylinder {
                radius: 0.4,
                half_height: 0.6,
            },
            0.6,
        ),
    ];
    for (shape, expected) in cases {
        let mut world = floor_world();
        let body = world.insert(BodyDesc::dynamic(shape).at(Vec3::new(0.0, 3.0, 0.0)));
        run(&mut world, 300);
        let y = world.body(body).unwrap().position.y;
        assert!(
            (y - expected).abs() < 0.01,
            "{shape:?} rests at {y}, expected {expected}"
        );
    }
}

#[test]
fn restitution_controls_bounce_height() {
    // Energy argument: rebounding at e*v leaves a rise of (e*v)^2 / (2g),
    // i.e. e^2 of the drop. With e = 0.8 that is 64% of 2.5 m ~ 1.6 m.
    let drop_from = 3.0f32;
    let rest = 0.5f32;

    let mut dull = floor_world();
    let dull_ball = dull.insert(
        BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, drop_from, 0.0)),
    );
    let mut bouncy = floor_world();
    let bouncy_ball = bouncy.insert(
        BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
            .at(Vec3::new(0.0, drop_from, 0.0))
            .with_restitution(0.8),
    );

    let mut dull_peak = 0.0f32;
    let mut bouncy_peak = 0.0f32;
    let mut dull_landed = false;
    let mut bouncy_landed = false;
    for _ in 0..240 {
        dull.step(1.0 / 60.0);
        bouncy.step(1.0 / 60.0);
        let dy = dull.body(dull_ball).unwrap().position.y;
        let by = bouncy.body(bouncy_ball).unwrap().position.y;
        // Only measure the rebound: the peak after the ball first touches.
        if dy <= rest + 1e-3 {
            dull_landed = true;
        }
        if by <= rest + 1e-3 {
            bouncy_landed = true;
        }
        if dull_landed {
            dull_peak = dull_peak.max(dy);
        }
        if bouncy_landed {
            bouncy_peak = bouncy_peak.max(by);
        }
    }
    assert!(bouncy_landed, "the bouncy ball must reach the floor");
    assert!(
        dull_peak < rest + 0.05,
        "a dull ball must not bounce, peaked at {dull_peak}"
    );
    assert!(
        bouncy_peak > rest + 1.0,
        "an e = 0.8 ball should rebound about 1.6 m, peaked at {bouncy_peak}"
    );
    assert!((dull.body(dull_ball).unwrap().position.y - rest).abs() < 0.01);
}

#[test]
fn friction_decelerates_a_sliding_box_by_mu_g() {
    // Coulomb friction removes mu*g of speed per second once the contact is
    // sliding: mu = 0.25, g = 10 -> 2.5 m/s^2.
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.set_gravity(Vec3::new(0.0, -10.0, 0.0));
    world.insert(
        BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::splat(10.0),
        })
        .at(Vec3::new(0.0, -10.0, 0.0))
        .with_friction(0.25),
    );
    let slider = world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 0.5, 0.0))
        .with_friction(0.25)
        .with_velocity(Vec3::new(5.0, 0.0, 0.0)),
    );
    run(&mut world, 60); // one second
    let vx = world.body(slider).unwrap().linear_velocity.x;
    assert!(
        (vx - 2.5).abs() < 0.15,
        "after 1 s at mu*g = 2.5 m/s^2, v = {vx}"
    );
    let x = world.body(slider).unwrap().position.x;
    // s = v0 t - a t^2 / 2 = 5 - 1.25 = 3.75 m.
    assert!(
        (x - 3.75).abs() < 0.2,
        "travelled {x} m, expected about 3.75"
    );
}

#[test]
fn a_frictionless_box_keeps_its_speed() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.set_gravity(Vec3::new(0.0, -10.0, 0.0));
    world.insert(
        BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::splat(10.0),
        })
        .at(Vec3::new(0.0, -10.0, 0.0))
        .with_friction(0.0),
    );
    let slider = world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 0.5, 0.0))
        .with_friction(0.0)
        .with_velocity(Vec3::new(5.0, 0.0, 0.0)),
    );
    run(&mut world, 60);
    let vx = world.body(slider).unwrap().linear_velocity.x;
    assert!(
        (vx - 5.0).abs() < 0.05,
        "frictionless slide keeps 5 m/s, got {vx}"
    );
}

#[test]
fn a_body_sleeps_then_wakes_on_impulse() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.6, 0.0));
    // sleep_time_required = 0.5 s, so 1.5 s of rest is plenty.
    run(&mut world, 90);
    assert!(
        world.body(b).unwrap().sleeping,
        "the box should have settled"
    );
    assert!(world.stats().sleeping_bodies >= 1);
    assert_eq!(world.body(b).unwrap().linear_velocity, Vec3::ZERO);

    // `Body::apply_impulse` wakes the body in place; the world reports the wake
    // when the change goes through a world setter.
    world.clear_events();
    assert!(world.set_velocity(b, Vec3::new(0.0, 0.0, 3.0)));
    assert!(!world.body(b).unwrap().sleeping);
    assert!(
        world
            .events()
            .iter()
            .any(|e| matches!(e, PhysicsEvent::BodyWoke { body } if *body == b)),
        "a world setter reports the wake"
    );
    // And an impulse on the body itself wakes it too.
    run(&mut world, 90);
    assert!(world.body(b).unwrap().sleeping);
    world
        .body_mut(b)
        .unwrap()
        .apply_impulse(Vec3::new(0.0, 0.0, 3.0));
    assert!(!world.body(b).unwrap().sleeping);

    let before = world.body(b).unwrap().position.z;
    run(&mut world, 10);
    assert!(
        world.body(b).unwrap().position.z > before + 0.05,
        "the impulse moved it"
    );
}

#[test]
fn a_sleeping_body_wakes_when_a_moving_body_hits_it() {
    let mut world = floor_world();
    let sleeper = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.5, 0.0));
    run(&mut world, 90);
    assert!(world.body(sleeper).unwrap().sleeping);

    // Drop a second box squarely onto the sleeper. The sleeper may settle back
    // to sleep once the pile is at rest, so watch the whole window for the
    // wake event rather than only the final frame.
    let projectile = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 3.0, 0.0));
    let mut woke = false;
    let mut awake_at_impact = false;
    for _ in 0..30 {
        world.step(1.0 / 60.0);
        woke |= world
            .events()
            .iter()
            .any(|e| matches!(e, PhysicsEvent::BodyWoke { body } if *body == sleeper));
        world.clear_events();
        let projectile_y = world.body(projectile).unwrap().position.y;
        if projectile_y < 1.6 {
            awake_at_impact |= !world.body(sleeper).unwrap().sleeping;
        }
    }
    assert!(woke, "the impact must report a wake for the sleeping box");
    assert!(
        awake_at_impact,
        "the sleeping box is awake while the impact resolves"
    );
    // And the pile comes back to rest.
    run(&mut world, 120);
    assert!(world.body(sleeper).unwrap().sleeping);
    assert!(world.body(projectile).unwrap().sleeping);
    let top = world.body(projectile).unwrap().position.y;
    assert!(
        (top - 1.5).abs() < 0.03,
        "the second box rests on the first, y = {top}"
    );
}

#[test]
fn collision_enter_and_exit_fire_once_each() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 2.0, 0.0));
    world.clear_events();
    let mut enters = 0;
    let mut impulse = 0.0f32;
    for _ in 0..90 {
        world.step(1.0 / 60.0);
        for event in world.events() {
            if let PhysicsEvent::CollisionEnter {
                a,
                b: other,
                impulse: j,
                ..
            } = *event
            {
                assert!(a < other, "the lower handle is always reported first");
                enters += 1;
                impulse = impulse.max(j);
            }
        }
        world.clear_events();
    }
    assert_eq!(enters, 1, "landing must fire exactly one CollisionEnter");
    assert!(impulse > 0.0, "a landing impact has a positive impulse");

    // Teleporting away must produce exactly one exit.
    world.set_position(b, Vec3::new(0.0, 5.0, 0.0));
    world.step(1.0 / 60.0);
    let exits = world
        .events()
        .iter()
        .filter(|e| matches!(e, PhysicsEvent::CollisionExit { .. }))
        .count();
    assert_eq!(exits, 1);
}

#[test]
fn triggers_fire_enter_and_exit_without_pushing() {
    let mut world = floor_world();
    let trigger = world.insert(
        BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::new(1.0, 0.5, 1.0),
        })
        .at(Vec3::new(0.0, 1.5, 0.0))
        .as_sensor()
        .with_layer(LAYER_WORLD, LAYER_ALL),
    );
    let faller = world.insert(
        BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.25 }).at(Vec3::new(0.0, 4.0, 0.0)),
    );

    let mut enters = 0;
    let mut exits = 0;
    let mut saw_sensor_contact = false;
    for _ in 0..180 {
        world.step(1.0 / 60.0);
        saw_sensor_contact |= world.contacts().iter().any(|c| c.is_sensor);
        for event in world.events() {
            match *event {
                PhysicsEvent::TriggerEnter { trigger: t, other } => {
                    assert_eq!(t, trigger);
                    assert_eq!(other, faller);
                    enters += 1;
                }
                PhysicsEvent::TriggerExit { trigger: t, other } => {
                    assert_eq!(t, trigger);
                    assert_eq!(other, faller);
                    exits += 1;
                }
                _ => {}
            }
        }
        world.clear_events();
    }
    assert_eq!(enters, 1, "one TriggerEnter as the sphere passes through");
    assert_eq!(exits, 1, "one TriggerExit after it lands below the trigger");
    assert!(saw_sensor_contact, "sensor contacts appear in contacts()");
    // The trigger must not have slowed the fall: the sphere rests on the floor.
    let y = world.body(faller).unwrap().position.y;
    assert!((y - 0.25).abs() < 0.01, "the sensor never pushes, y = {y}");
}

#[test]
fn layer_masks_decide_who_collides() {
    // A prop-layer floor that only collides with LAYER_PROP.
    let make = |floor_mask: u32| {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(10.0),
            })
            .at(Vec3::new(0.0, -10.0, 0.0))
            .with_layer(LAYER_WORLD, floor_mask),
        );
        world.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(0.0, 1.0, 0.0))
                .with_layer(LAYER_PROP, LAYER_WORLD),
        );
        world
    };

    let mut blocked = make(LAYER_PROP);
    let ball = blocked.bodies().nth(1).map(|(h, _)| h).unwrap();
    run(&mut blocked, 180);
    let resting = blocked.body(ball).unwrap().position.y;
    assert!(
        (resting - 0.5).abs() < 0.01,
        "matching layers collide, y = {resting}"
    );

    let mut ignored = make(LAYER_PLAYER);
    let ball = ignored.bodies().nth(1).map(|(h, _)| h).unwrap();
    run(&mut ignored, 180);
    let fallen = ignored.body(ball).unwrap().position.y;
    assert!(
        fallen < -20.0,
        "a mismatched mask falls straight through, y = {fallen}"
    );
}

#[test]
fn queries_respect_change_after_set_position() {
    let mut world = floor_world();
    let b = world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 5.0, 0.0)),
    );
    let ray = Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::DOWN);
    let first = world.raycast(&ray, QueryFilter::default()).unwrap();
    assert_eq!(first.body, b);
    assert!((first.distance - 4.5).abs() < 1e-3);

    assert!(world.set_position(b, Vec3::new(5.0, 5.0, 0.0)));
    let after = world.raycast(&ray, QueryFilter::default()).unwrap();
    assert_ne!(after.body, b, "the box moved out of the ray");
    assert!(
        (after.distance - 10.0).abs() < 1e-3,
        "the ray now hits the floor"
    );
}

#[test]
fn overlap_queries_filter_by_layer_and_sensor() {
    let mut world = floor_world();
    let prop = world.insert(
        BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
            .at(Vec3::new(0.0, 5.0, 0.0))
            .with_layer(LAYER_PROP, LAYER_ALL),
    );
    let trigger = world.insert(
        BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 })
            .at(Vec3::new(0.0, 7.0, 0.0))
            .as_sensor()
            .with_layer(LAYER_WORLD, LAYER_ALL),
    );

    let mut out = Vec::new();
    world.overlap_sphere(
        Vec3::new(0.0, 6.0, 0.0),
        2.0,
        QueryFilter::default(),
        &mut out,
    );
    assert_eq!(out, vec![prop], "sensors are excluded by default");

    world.overlap_sphere(
        Vec3::new(0.0, 6.0, 0.0),
        2.0,
        QueryFilter::default().with_sensors(true),
        &mut out,
    );
    assert_eq!(out, vec![prop, trigger], "sorted by handle");

    world.overlap_sphere(
        Vec3::new(0.0, 6.0, 0.0),
        2.0,
        QueryFilter::with_mask(LAYER_PLAYER).with_sensors(true),
        &mut out,
    );
    assert!(out.is_empty(), "no body is on the player layer");

    world.overlap_aabb(
        Aabb::new(Vec3::new(-1.0, 4.5, -1.0), Vec3::new(1.0, 5.5, 1.0)),
        QueryFilter::default(),
        &mut out,
    );
    assert_eq!(out, vec![prop]);
}

#[test]
fn raycast_reports_the_nearest_of_several_bodies() {
    let mut world = floor_world();
    let near = world.insert(
        BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, 2.0, 0.0)),
    );
    let far = world.insert(
        BodyDesc::static_body(ColliderShape::Capsule {
            radius: 0.5,
            half_height: 1.0,
        })
        .at(Vec3::new(0.0, 6.0, 0.0)),
    );
    let ray = Ray::new(Vec3::new(0.0, 20.0, 0.0), Vec3::DOWN);
    let mut hits = Vec::new();
    world.raycast_all(&ray, QueryFilter::default(), &mut hits);
    assert_eq!(hits.len(), 3, "two bodies plus the floor");
    assert_eq!(hits[0].body, far, "the capsule cap reaches y = 6 + 1 + 0.5");
    assert!((hits[0].distance - 12.5).abs() < 1e-3);
    assert_eq!(hits[1].body, near);
    assert!((hits[1].distance - 17.5).abs() < 1e-3);
    let nearest = world.raycast(&ray, QueryFilter::default()).unwrap();
    assert_eq!(nearest.body, hits[0].body);
}

#[test]
fn determinism_is_bit_exact() {
    let scenario = || {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)),
            0,
        );
        // A pile of mixed shapes plus a kinematic platform and a sensor.
        for i in 0..12 {
            let f = i as f32;
            let shape = match i % 4 {
                0 => ColliderShape::Box {
                    half_extents: Vec3::splat(0.4),
                },
                1 => ColliderShape::Sphere { radius: 0.4 },
                2 => ColliderShape::Capsule {
                    radius: 0.3,
                    half_height: 0.4,
                },
                _ => ColliderShape::Cylinder {
                    radius: 0.35,
                    half_height: 0.45,
                },
            };
            world.insert(
                BodyDesc::dynamic(shape)
                    .at(Vec3::new(f * 0.35 - 2.0, 1.0 + f * 0.5, f * 0.1))
                    .with_velocity(Vec3::new(f * 0.1, 0.0, -f * 0.05))
                    .with_restitution(0.2)
                    .with_friction(0.4)
                    .with_user_data(i as u64),
            );
        }
        world.insert(
            BodyDesc::kinematic(ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            })
            .at(Vec3::new(-4.0, 1.0, 0.0))
            .with_velocity(Vec3::new(0.5, 0.0, 0.0)),
        );
        world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(1.0),
            })
            .at(Vec3::new(3.0, 1.0, 0.0))
            .as_sensor(),
        );
        for _ in 0..180 {
            world.step(1.0 / 60.0);
        }
        // Churn the slot map so free-list reuse is exercised too.
        let doomed: Vec<BodyHandle> = world.bodies().map(|(h, _)| h).take(3).collect();
        for h in doomed {
            world.remove(h);
        }
        for i in 0..3 {
            world.insert(
                BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.3 })
                    .at(Vec3::new(i as f32, 4.0, 0.0)),
            );
        }
        for _ in 0..60 {
            world.step(1.0 / 60.0);
        }
        world
            .bodies()
            .map(|(h, b)| {
                (
                    h.to_bits(),
                    b.position.to_array().map(f32::to_bits),
                    b.rotation.to_array().map(f32::to_bits),
                    b.linear_velocity.to_array().map(f32::to_bits),
                    b.sleeping,
                )
            })
            .collect::<Vec<_>>()
    };

    let a = scenario();
    let b = scenario();
    assert_eq!(a.len(), b.len());
    assert_eq!(a, b, "two runs of the same scenario must be bit-identical");
    assert!(a.len() > 10);
}

#[test]
fn non_finite_state_is_recovered_not_propagated() {
    let mut world = floor_world();
    let victim = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.5, 0.0));
    let neighbour = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 1.5, 0.0));
    run(&mut world, 120);
    let safe = world.body(victim).unwrap().position;
    assert!(safe.is_finite());

    // A gameplay bug writes poison straight into the body.
    world.body_mut(victim).unwrap().linear_velocity = Vec3::new(f32::NAN, f32::INFINITY, 0.0);
    world.body_mut(victim).unwrap().position = Vec3::new(f32::NAN, f32::NAN, f32::NAN);
    world.clear_events();
    run(&mut world, 5);

    let body = world.body(victim).unwrap();
    assert!(
        body.position.is_finite(),
        "position recovered: {:?}",
        body.position
    );
    assert!(body.linear_velocity.is_finite());
    assert!(body.rotation.is_finite());
    assert!(
        body.sleeping,
        "recovery is deliberately quiet: the body sleeps"
    );
    assert!(
        (body.position.y - safe.y).abs() < 0.05,
        "recovered to the last finite spot: {} vs {}",
        body.position.y,
        safe.y
    );
    assert!(
        world.body(neighbour).unwrap().position.is_finite(),
        "no neighbour poisoning"
    );
    assert!(
        world
            .events()
            .iter()
            .all(|e| !matches!(e, PhysicsEvent::CollisionEnter { .. }))
    );
}

#[test]
fn non_finite_setters_are_rejected() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 1.0, 0.0));
    assert!(!world.set_position(b, Vec3::new(f32::NAN, 0.0, 0.0)));
    assert!(!world.set_velocity(b, Vec3::new(f32::INFINITY, 0.0, 0.0)));
    assert!(!world.set_rotation(b, Quat::new(f32::NAN, 0.0, 0.0, 1.0)));
    assert_eq!(world.body(b).unwrap().position, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(world.body(b).unwrap().linear_velocity, Vec3::ZERO);

    // A non-finite dt is simply ignored rather than exploding the world.
    world.step(f32::NAN);
    world.step(-1.0);
    assert!(world.body(b).unwrap().position.is_finite());
}

#[test]
fn the_step_delta_is_clamped() {
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 5.0, 0.0));
    world.step(10.0); // clamped to max_step_delta = 0.05
    let v = world.body(b).unwrap().linear_velocity.y;
    // v = -g * dt = -19.62 * 0.05 = -0.981, nowhere near -196.
    assert!(
        (v + 19.62 * 0.05).abs() < 1e-3,
        "clamped dt produced v = {v}"
    );
}

#[test]
fn kinematic_platform_pushes_a_crate() {
    let mut world = floor_world();
    let platform = world.insert(
        BodyDesc::kinematic(ColliderShape::Box {
            half_extents: Vec3::new(0.5, 0.5, 3.0),
        })
        .at(Vec3::new(-2.0, 0.5, 0.0))
        .with_velocity(Vec3::new(3.0, 0.0, 0.0)),
    );
    let boxed = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.5, 0.0));
    run(&mut world, 60);
    let moved = world.body(boxed).unwrap().position.x;
    assert!(moved > 0.2, "the platform pushed the crate to x = {moved}");
    // The platform itself must be unaffected by the collision.
    let platform_x = world.body(platform).unwrap().position.x;
    assert!(
        (platform_x - (-2.0 + 3.0)).abs() < 0.05,
        "platform at {platform_x}"
    );
}

#[test]
fn kinematic_and_static_bodies_never_fall() {
    let mut world = floor_world();
    let k = world.insert(
        BodyDesc::kinematic(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, 5.0, 0.0)),
    );
    let s = world.insert(
        BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(2.0, 5.0, 0.0)),
    );
    run(&mut world, 120);
    assert_eq!(world.body(k).unwrap().position.y, 5.0);
    assert_eq!(world.body(s).unwrap().position.y, 5.0);
    assert_eq!(world.body(k).unwrap().kind, BodyKind::Kinematic);
    assert_eq!(world.body(s).unwrap().kind, BodyKind::Static);
}

#[test]
fn translate_kinematic_moves_only_movable_bodies() {
    let mut world = floor_world();
    let k = world.insert(
        BodyDesc::kinematic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 1.0, 0.0)),
    );
    assert!(world.translate_kinematic(k, Vec3::new(2.0, 0.0, 0.0)));
    assert_eq!(world.body(k).unwrap().position, Vec3::new(2.0, 1.0, 0.0));

    let s = world.insert(
        BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 1.0, 0.0)),
    );
    assert!(
        !world.translate_kinematic(s, Vec3::X),
        "level geometry is built once"
    );
    assert!(!world.translate_kinematic(k, Vec3::splat(f32::NAN)));
}

#[test]
fn body_lifecycle_and_stats() {
    let mut world = floor_world();
    assert_eq!(world.len(), 1);
    assert!(!world.is_empty());

    let a = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 1.0, 0.0));
    let b = dynamic_box(&mut world, 0.5, Vec3::new(3.0, 1.0, 0.0));
    assert!(world.contains(a) && world.contains(b));
    assert_eq!(world.len(), 3);

    run(&mut world, 60);
    let stats = world.stats();
    assert_eq!(stats.body_count, 3);
    assert_eq!(stats.solver_iterations, world.config().solver_iterations);
    assert!(
        stats.broadphase_pairs >= 1,
        "the boxes overlap the floor's cells"
    );

    assert!(world.remove(b));
    assert!(!world.remove(b), "removing twice fails");
    assert!(!world.contains(b));
    assert_eq!(world.len(), 2);
    assert!(world.body(b).is_none());

    world.clear();
    assert!(world.is_empty());
    assert!(world.contacts().is_empty());
    assert_eq!(world.stats().body_count, 0);
}

#[test]
fn max_bodies_limits_inserts() {
    let mut world = PhysicsWorld::new(PhysicsConfig {
        max_bodies: 2,
        ..Default::default()
    });
    let a = dynamic_box(&mut world, 0.5, Vec3::ZERO);
    let b = dynamic_box(&mut world, 0.5, Vec3::X);
    let c = dynamic_box(&mut world, 0.5, Vec3::Z);
    assert!(world.contains(a) && world.contains(b));
    assert_eq!(c, BodyHandle::INVALID);
    assert!(!world.contains(c));
    assert_eq!(world.len(), 2);
}

#[test]
fn sleeping_bodies_do_not_react_to_a_resting_neighbour() {
    // Two boxes stacked: once settled, both must sleep and stay asleep, which
    // is the regression guard for a wake loop between touching bodies.
    let mut world = floor_world();
    let lower = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 0.5, 0.0));
    let upper = dynamic_box(&mut world, 0.5, Vec3::new(0.0, 1.5, 0.0));
    run(&mut world, 240);
    let asleep_lower = world.body(lower).unwrap().sleeping;
    let asleep_upper = world.body(upper).unwrap().sleeping;
    assert!(asleep_upper, "the top box should settle and sleep");
    assert!(asleep_lower, "the bottom box should settle and sleep");
    let y = world.body(upper).unwrap().position.y;
    assert!(
        (y - 1.5).abs() < 0.02,
        "the stack holds its shape, upper y = {y}"
    );
}

#[test]
fn a_body_never_tunnels_a_thin_floor_when_it_starts_inside_it() {
    // Degenerate but legal: a body spawned overlapping static geometry must be
    // pushed out, not launched.
    let mut world = floor_world();
    let b = dynamic_box(&mut world, 0.5, Vec3::new(0.0, -0.25, 0.0));
    run(&mut world, 240);
    let body = world.body(b).unwrap();
    assert!(body.position.is_finite());
    assert!(
        body.position.y > 0.0,
        "pushed out of the floor rather than through it: y = {}",
        body.position.y
    );
}

#[test]
fn rotation_is_integrated_and_contacts_only_exchange_yaw() {
    let mut world = floor_world();
    let spun = world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(0.0, 0.5, 0.0))
        .with_angular_velocity(Vec3::new(0.0, 1.0, 0.0)),
    );
    world.step(1.0 / 60.0);
    let yaw = world.body(spun).unwrap().rotation.to_yaw();
    // One step of 1 rad/s about Y advances the yaw by dt.
    assert!((yaw - 1.0 / 60.0).abs() < 1e-3, "yaw = {yaw}");

    // Once it stops spinning the box must settle without any tumbling.
    run(&mut world, 120);
    let body = world.body(spun).unwrap();
    assert!(
        body.angular_velocity.x.abs() < 1e-3,
        "no pitch from contacts"
    );
    assert!(
        body.angular_velocity.z.abs() < 1e-3,
        "no roll from contacts"
    );
}

#[test]
fn character_walks_a_course_of_steps_and_walls() {
    // The "player feel" smoke test: a corridor with a step, a wall and a drop.
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.insert_static_aabb(
        Aabb::new(Vec3::new(-20.0, -1.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
        0,
    );
    // A kerb at z = 1 (within reach of a 2 m walk) and a wall at z = 4.
    world.insert_static_aabb(
        Aabb::new(Vec3::new(-20.0, 0.0, 1.0), Vec3::new(20.0, 0.3, 20.0)),
        0,
    );
    world.insert_static_aabb(
        Aabb::new(Vec3::new(-20.0, 0.0, 4.0), Vec3::new(20.0, 3.0, 20.0)),
        0,
    );
    let character = world.insert(
        BodyDesc::kinematic(ColliderShape::Capsule {
            radius: 0.3,
            half_height: 0.5,
        })
        .at(Vec3::new(0.0, 0.8, 0.0))
        .with_layer(LAYER_PLAYER, LAYER_ALL),
    );
    world.step(1.0 / 60.0);

    // Walk forward for two seconds at 2 m/s: 4 m of motion, more than the
    // 3.7 m from the spawn to the wall.
    let mut last = CharacterMove::default();
    let mut mid_course_y = 0.0f32;
    for step in 0..120 {
        last = world.move_character(character, Vec3::new(0.0, 0.0, 2.0 / 60.0), Vec3::Y);
        let position = world.body(character).unwrap().position;
        assert!(position.is_finite());
        if step == 60 {
            // Half way: the kerb has been climbed and the walk continues.
            assert!(
                position.z > 1.5,
                "climbed the kerb and kept going: z = {}",
                position.z
            );
            mid_course_y = position.y;
        }
    }
    let position = world.body(character).unwrap().position;
    assert!(
        (mid_course_y - (0.3 + 0.8)).abs() < 0.05,
        "standing on the kerb mid-course: y = {mid_course_y}"
    );
    assert!(
        (position.y - (0.3 + 0.8)).abs() < 0.05,
        "still on the kerb: y = {}",
        position.y
    );
    assert!(
        (position.z - (4.0 - 0.3)).abs() < 0.02,
        "stopped at the wall: z = {}",
        position.z
    );
    assert!(last.hit_wall, "the wall reports a hit");
    assert!(last.grounded);
}

#[test]
fn queries_inside_a_sensor_and_through_it() {
    let mut world = floor_world();
    let wall = world.insert_static_aabb(
        Aabb::new(Vec3::new(-5.0, 0.0, 4.0), Vec3::new(5.0, 2.0, 4.2)),
        42,
    );
    let trigger = world.insert(
        BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::splat(1.0),
        })
        .at(Vec3::new(0.0, 1.0, 4.0))
        .as_sensor(),
    );
    let eye = Vec3::new(0.0, 1.0, 0.0);

    // A line-of-sight ray ignores the trigger and stops at the wall.
    let hit = world
        .raycast(&Ray::new(eye, Vec3::Z), QueryFilter::default())
        .unwrap();
    assert_eq!(hit.body, wall);
    assert!((hit.distance - 4.0).abs() < 1e-3);
    assert_eq!(hit.user_data, 42);

    // With sensors enabled the trigger is nearer and wins.
    let hit = world
        .raycast(
            &Ray::new(eye, Vec3::Z),
            QueryFilter::default().with_sensors(true),
        )
        .unwrap();
    assert_eq!(hit.body, trigger);

    // A filtered query can still see exactly one of them.
    let hit = world
        .raycast(&Ray::new(eye, Vec3::Z), QueryFilter::ALL.ignoring(wall))
        .unwrap();
    assert_eq!(hit.body, trigger);
}
