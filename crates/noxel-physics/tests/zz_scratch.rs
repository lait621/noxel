use noxel_core::math::{Aabb, Vec3};
use noxel_physics::{
    BodyDesc, ColliderShape, LAYER_ALL, LAYER_PLAYER, PhysicsConfig, PhysicsEvent, PhysicsWorld,
};

#[test]
fn debug_character_step_up() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.insert_static_aabb(Aabb::new(Vec3::new(-20.0, -1.0, -20.0), Vec3::new(20.0, 0.0, 20.0)), 0);
    world.insert_static_aabb(Aabb::new(Vec3::new(-20.0, 0.0, 1.0), Vec3::new(20.0, 0.3, 20.0)), 0);
    let character = world.insert(
        BodyDesc::kinematic(ColliderShape::Capsule { radius: 0.3, half_height: 0.5 })
            .at(Vec3::new(0.0, 0.8, 0.0))
            .with_layer(LAYER_PLAYER, LAYER_ALL),
    );
    world.step(1.0 / 60.0);
    for i in 0..70 {
        let m = world.move_character(character, Vec3::new(0.0, 0.0, 2.0 / 60.0), Vec3::Y);
        let p = world.body(character).unwrap().position;
        println!(
            "{i:2} pos=({:.4},{:.4},{:.4}) t=({:.4},{:.4},{:.4}) grounded={} wall={} ceiling={} hits={}",
            p.x, p.y, p.z, m.translation.x, m.translation.y, m.translation.z, m.grounded, m.hit_wall,
            m.hit_ceiling, m.hit_count
        );
    }
}

#[test]
fn debug_sleep_wake() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    world.insert_static_aabb(Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)), 1);
    let sleeper = world.insert(
        BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) }).at(Vec3::new(0.0, 0.5, 0.0)),
    );
    for _ in 0..90 {
        world.step(1.0 / 60.0);
    }
    println!("sleeper sleeping after settle: {}", world.body(sleeper).unwrap().sleeping);
    let projectile = world.insert(
        BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) }).at(Vec3::new(0.0, 3.0, 0.0)),
    );
    world.clear_events();
    for i in 0..60 {
        world.step(1.0 / 60.0);
        let s = world.body(sleeper).unwrap();
        let p = world.body(projectile).unwrap();
        let evts: Vec<String> = world
            .events()
            .iter()
            .map(|e| match e {
                PhysicsEvent::BodySlept { .. } => "slept".to_string(),
                PhysicsEvent::BodyWoke { .. } => "woke".to_string(),
                PhysicsEvent::CollisionEnter { .. } => "enter".to_string(),
                PhysicsEvent::CollisionExit { .. } => "exit".to_string(),
                _ => "trigger".to_string(),
            })
            .collect();
        println!(
            "{i:2} sleeper y={:.4} sleep={} v={:.3} | proj y={:.4} sleep={} v={:.3} | {:?}",
            s.position.y, s.sleeping, s.linear_velocity.y, p.position.y, p.sleeping, p.linear_velocity.y, evts
        );
        world.clear_events();
    }
}
