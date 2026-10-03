//! A compile-time lock on the public API contract.
//!
//! Three sibling crates are written against these exact paths, names and
//! signatures, so every one of them is named here. A rename, a type change or a
//! dropped method fails to compile instead of failing at integration time.

use std::collections::HashSet;

use noxel_core::math::{Aabb, Quat, Ray, Vec3};
use noxel_physics::{
    Body, BodyDesc, BodyHandle, BodyKind, CharacterMove, ColliderShape, Contact, LAYER_ALL,
    LAYER_CAMERA, LAYER_NPC, LAYER_PLAYER, LAYER_PROP, LAYER_TRIGGER, LAYER_WORLD, PhysicsConfig,
    PhysicsEvent, PhysicsStats, PhysicsWorld, QueryFilter, RaycastHit,
};

#[test]
fn layer_constants_are_distinct_single_bits() {
    let bits = [
        LAYER_WORLD,
        LAYER_PLAYER,
        LAYER_NPC,
        LAYER_PROP,
        LAYER_TRIGGER,
        LAYER_CAMERA,
    ];
    let unique: HashSet<u32> = bits.iter().copied().collect();
    assert_eq!(unique.len(), bits.len());
    for bit in bits {
        assert_eq!(bit.count_ones(), 1);
    }
    assert_eq!(LAYER_ALL, u32::MAX);
}

#[test]
fn body_kind_is_hashable_and_defaults_to_dynamic() {
    let mut set = HashSet::new();
    for kind in [BodyKind::Dynamic, BodyKind::Static, BodyKind::Kinematic] {
        set.insert(kind);
        let _copy: BodyKind = kind;
        let _ = format!("{kind:?}");
    }
    assert_eq!(set.len(), 3);
    assert_eq!(BodyKind::default(), BodyKind::Dynamic);
}

#[test]
fn collider_shape_surface() {
    let shapes = [
        ColliderShape::Box {
            half_extents: Vec3::ONE,
        },
        ColliderShape::Sphere { radius: 1.0 },
        ColliderShape::Capsule {
            radius: 0.5,
            half_height: 1.0,
        },
        ColliderShape::Cylinder {
            radius: 0.5,
            half_height: 1.0,
        },
    ];
    for shape in shapes {
        let copy: ColliderShape = shape;
        assert_eq!(copy, shape, "ColliderShape: PartialEq + Copy");
        let bounds: Aabb = shape.aabb(Vec3::ZERO, Quat::IDENTITY);
        let radius: f32 = shape.bounding_radius();
        let volume: f32 = shape.volume();
        let inside: bool = shape.contains_point(Vec3::ZERO);
        let support: Vec3 = shape.support(Vec3::Y);
        assert!(bounds.is_finite() && radius > 0.0 && volume > 0.0 && inside);
        assert!(support.is_finite());
        let _ = format!("{shape:?}");
    }
}

#[test]
fn body_fields_and_helpers() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    let handle: BodyHandle = world.insert(
        BodyDesc::dynamic(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        })
        .at(Vec3::new(1.0, 2.0, 3.0)),
    );
    let body: &Body = world.body(handle).unwrap();
    let _: BodyKind = body.kind;
    let _: Vec3 = body.position;
    let _: Quat = body.rotation;
    let _: Vec3 = body.linear_velocity;
    let _: Vec3 = body.angular_velocity;
    let _: f32 = body.mass;
    let _: f32 = body.inv_mass;
    let _: f32 = body.restitution;
    let _: f32 = body.friction;
    let _: f32 = body.linear_damping;
    let _: f32 = body.angular_damping;
    let _: f32 = body.gravity_scale;
    let _: bool = body.is_sensor;
    let _: bool = body.sleeping;
    let _: f32 = body.sleep_timer;
    let _: u32 = body.layer;
    let _: u32 = body.mask;
    let _: u64 = body.user_data;
    let _: ColliderShape = body.shape;
    let _: Aabb = body.aabb();
    assert!(body.is_dynamic() && !body.is_static() && !body.is_kinematic());
    assert!(body.can_collide_with(body));

    let body: &mut Body = world.body_mut(handle).unwrap();
    body.wake();
    body.apply_impulse(Vec3::new(0.0, 1.0, 0.0));
    assert!(!body.sleeping);
}

#[test]
fn body_desc_builder_surface() {
    let desc: BodyDesc = BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
        .at(Vec3::new(0.0, 1.0, 0.0))
        .with_rotation(Quat::IDENTITY)
        .with_velocity(Vec3::X)
        .with_angular_velocity(Vec3::Y)
        .with_mass(2.0)
        .with_friction(0.4)
        .with_restitution(0.2)
        .with_damping(0.1, 0.2)
        .with_gravity_scale(0.5)
        .with_layer(LAYER_PROP, LAYER_ALL)
        .with_user_data(9)
        .as_sensor()
        .sleeping(false);
    let _ = format!("{desc:?}");
    let _: BodyDesc = BodyDesc::static_body(ColliderShape::Sphere { radius: 1.0 });
    let _: BodyDesc = BodyDesc::kinematic(ColliderShape::Capsule {
        radius: 0.3,
        half_height: 0.5,
    });
    let _cloned: BodyDesc = desc.clone();
}

#[test]
fn physics_config_surface() {
    let config = PhysicsConfig {
        gravity: Vec3::new(0.0, -20.0, 0.0),
        broadphase_cell_size: 4.0,
        contact_tolerance: 0.005,
        solver_iterations: 4,
        position_correction: 0.2,
        max_step_delta: 0.05,
        sleep_linear_threshold: 0.05,
        sleep_angular_threshold: 0.05,
        sleep_time_required: 0.5,
        max_bodies: 128,
    };
    let copy: PhysicsConfig = config;
    let _ = format!("{copy:?}");
    let _ = PhysicsConfig::default();
    let mut world = PhysicsWorld::new(config);
    let _: &PhysicsConfig = world.config();
    world.set_config(PhysicsConfig::default());
    world.set_gravity(Vec3::new(0.0, -9.81, 0.0));
    let _: Vec3 = world.gravity();
}

#[test]
fn query_types_surface() {
    let filter: QueryFilter = QueryFilter::default();
    let _: u32 = filter.mask;
    let _: Option<BodyHandle> = filter.ignore;
    let _: bool = filter.include_sensors;
    let _: bool = filter.include_static;
    let filter: QueryFilter = QueryFilter::with_mask(LAYER_ALL)
        .ignoring(BodyHandle::INVALID)
        .with_sensors(true)
        .with_static(false);
    let _all: QueryFilter = QueryFilter::ALL;
    let _copy: QueryFilter = filter;
    let _ = format!("{filter:?}");

    let hit = RaycastHit {
        body: BodyHandle::INVALID,
        distance: 1.0,
        point: Vec3::ZERO,
        normal: Vec3::Y,
        user_data: 3,
    };
    assert_eq!(hit, hit);
    let _ = format!("{hit:?}");
}

#[test]
fn world_surface_exists() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    let desc = BodyDesc::dynamic(ColliderShape::Box {
        half_extents: Vec3::splat(0.5),
    });
    let handle: BodyHandle = world.insert(desc);
    let _: BodyHandle =
        world.insert_static_aabb(Aabb::new(Vec3::new(-5.0, -1.0, -5.0), Vec3::ZERO), 1);
    assert!(world.contains(handle));
    let _: Option<&Body> = world.body(handle);
    let _: Option<&mut Body> = world.body_mut(handle);
    let count = world.bodies().count();
    assert_eq!(count, world.len());
    assert!(!world.is_empty());

    assert!(world.set_position(handle, Vec3::new(0.0, 2.0, 0.0)));
    assert!(world.set_rotation(handle, Quat::from_rotation_y(0.5)));
    assert!(world.set_velocity(handle, Vec3::X));
    assert!(world.translate_kinematic(handle, Vec3::Y));
    world.step(1.0 / 60.0);

    let _: &[Contact] = world.contacts();
    let _: &[PhysicsEvent] = world.events();
    world.clear_events();
    let stats: PhysicsStats = world.stats();
    let _: usize = stats.body_count;
    let _: usize = stats.active_bodies;
    let _: usize = stats.sleeping_bodies;
    let _: usize = stats.broadphase_pairs;
    let _: usize = stats.contact_count;
    let _: u32 = stats.solver_iterations;
    assert_eq!(stats.body_count, world.len());

    let mut hits: Vec<RaycastHit> = Vec::new();
    let _: Option<RaycastHit> =
        world.raycast(&Ray::new(Vec3::Y, Vec3::DOWN), QueryFilter::default());
    world.raycast_all(
        &Ray::new(Vec3::Y, Vec3::DOWN),
        QueryFilter::default(),
        &mut hits,
    );
    let mut bodies: Vec<BodyHandle> = Vec::new();
    world.overlap_sphere(Vec3::ZERO, 1.0, QueryFilter::default(), &mut bodies);
    world.overlap_aabb(
        Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)),
        QueryFilter::default(),
        &mut bodies,
    );

    let moved: CharacterMove = world.move_character(handle, Vec3::new(0.0, 0.0, 0.1), Vec3::Y);
    let _: Vec3 = moved.translation;
    let _: Vec3 = moved.velocity;
    let _: bool = moved.grounded;
    let _: Vec3 = moved.ground_normal;
    let _: Option<BodyHandle> = moved.ground_body;
    let _: bool = moved.hit_ceiling;
    let _: bool = moved.hit_wall;
    let _: u32 = moved.hit_count;
    let _: CharacterMove = CharacterMove::default();
    let _ = format!("{moved:?}");

    assert!(world.remove(handle));
    world.clear();
    assert!(world.is_empty());
}

#[test]
fn contact_fields_exist() {
    let mut world = PhysicsWorld::new(PhysicsConfig::default());
    let a = world.insert(
        BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, 0.0, 0.0)),
    );
    let b = world.insert(
        BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, 1.0, 0.0)),
    );
    world.step(1.0 / 60.0);
    let contacts: &[Contact] = world.contacts();
    assert!(!contacts.is_empty());
    let contact = contacts[0];
    let _: BodyHandle = contact.a;
    let _: BodyHandle = contact.b;
    let _: Vec3 = contact.normal;
    let _: Vec3 = contact.point;
    let _: f32 = contact.penetration;
    let _: bool = contact.is_sensor;
    assert_eq!(contact.a, a.min(b));
    let _copy = contact;
    assert_eq!(contact, contact);
    let _ = format!("{contact:?}");
}

#[test]
fn event_variants_exist() {
    let a: BodyHandle = BodyHandle::INVALID;
    let events = [
        PhysicsEvent::CollisionEnter {
            a,
            b: a,
            point: Vec3::ZERO,
            normal: Vec3::Y,
            impulse: 0.5,
        },
        PhysicsEvent::CollisionExit { a, b: a },
        PhysicsEvent::TriggerEnter {
            trigger: a,
            other: a,
        },
        PhysicsEvent::TriggerExit {
            trigger: a,
            other: a,
        },
        PhysicsEvent::BodySlept { body: a },
        PhysicsEvent::BodyWoke { body: a },
    ];
    for event in events {
        let _copy: PhysicsEvent = event;
        let _ = format!("{event:?}");
        assert_eq!(event, event);
    }
}
