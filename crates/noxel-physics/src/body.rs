//! Rigid bodies and their descriptors.

use noxel_core::math::{Aabb, Quat, Vec3};
use noxel_core::pool::Handle;

use crate::shape::ColliderShape;

/// A handle to a body stored in a [`crate::PhysicsWorld`].
///
/// Handles are generational: a handle to a removed body never resolves to the
/// body that reuses its slot.
pub type BodyHandle = Handle<Body>;

/// Layer bit used by level geometry, floors and walls.
pub const LAYER_WORLD: u32 = 1 << 0;
/// Layer bit used by player-controlled bodies.
pub const LAYER_PLAYER: u32 = 1 << 1;
/// Layer bit used by non-player characters.
pub const LAYER_NPC: u32 = 1 << 2;
/// Layer bit used by props, crates and other movable scenery.
pub const LAYER_PROP: u32 = 1 << 3;
/// Layer bit used by trigger volumes.
pub const LAYER_TRIGGER: u32 = 1 << 4;
/// Layer bit used by camera collision proxies.
pub const LAYER_CAMERA: u32 = 1 << 5;
/// Every layer bit set; the default collision mask.
pub const LAYER_ALL: u32 = u32::MAX;

/// Mass assigned to a body with no explicit mass, per cubic metre.
///
/// One kilogram per cubic metre keeps a 1×1×1 m crate at mass 1, which makes
/// impulse and force numbers in gameplay code read naturally.
pub const DEFAULT_DENSITY: f32 = 1.0;

/// How a body is moved and how it responds to contacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BodyKind {
    /// Moved by gravity, impulses and the solver.
    #[default]
    Dynamic,
    /// Never moves. Infinite mass, participates as an immovable obstacle.
    Static,
    /// Moved only by game code (`set_position`, `translate_kinematic` or a
    /// velocity that is integrated directly). It pushes dynamic bodies but is
    /// never pushed by them.
    Kinematic,
}

/// A rigid body: a collider plus the state the simulation integrates.
///
/// Fields are public on purpose — gameplay code reads and tweaks them
/// constantly — but anything that changes the *position* should go through
/// [`crate::PhysicsWorld::set_position`] so the broadphase stays in sync.
#[derive(Clone, Debug)]
pub struct Body {
    /// How the body is moved and how it responds to contacts.
    pub kind: BodyKind,
    /// Centre of the collider, in world space.
    pub position: Vec3,
    /// Orientation of the collider. Boxes use it in full; capsules and
    /// cylinders use it to orient their local Y axis.
    pub rotation: Quat,
    /// Linear velocity in metres per second.
    pub linear_velocity: Vec3,
    /// Angular velocity in radians per second, around the body's local axes.
    ///
    /// Yaw (Y) is what a top-down game uses; X and Z let a body tumble after
    /// an explosion.
    pub angular_velocity: Vec3,
    /// Mass in kilograms. Ignored for [`BodyKind::Static`] and
    /// [`BodyKind::Kinematic`] bodies, which behave as infinitely heavy.
    pub mass: f32,
    /// `1 / mass`, or `0` for bodies with infinite mass.
    pub inv_mass: f32,
    /// Bounciness in `[0, 1]`. `0` is a dead stop.
    pub restitution: f32,
    /// Coulomb friction coefficient in `[0, 1]`.
    pub friction: f32,
    /// Fraction of linear velocity removed per second.
    pub linear_damping: f32,
    /// Fraction of angular velocity removed per second.
    pub angular_damping: f32,
    /// Multiplier applied to world gravity for this body.
    pub gravity_scale: f32,
    /// When true the body reports overlaps but never pushes back.
    pub is_sensor: bool,
    /// True while the body is excluded from integration and the solver.
    pub sleeping: bool,
    /// Seconds the body has been below the sleep thresholds.
    pub sleep_timer: f32,
    /// Collision layer bits (usually one bit).
    pub layer: u32,
    /// Collision mask: the layer bits this body collides with.
    pub mask: u32,
    /// Free value for gameplay code; echoed back by query hits and events.
    pub user_data: u64,
    /// The collider, in the body's local space.
    pub shape: ColliderShape,
}

impl Body {
    /// World-space bounds of the collider, used by the broadphase.
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        self.shape.aabb(self.position, self.rotation)
    }

    /// True for [`BodyKind::Dynamic`].
    #[must_use]
    pub fn is_dynamic(&self) -> bool {
        self.kind == BodyKind::Dynamic
    }

    /// True for [`BodyKind::Static`].
    #[must_use]
    pub fn is_static(&self) -> bool {
        self.kind == BodyKind::Static
    }

    /// True for [`BodyKind::Kinematic`].
    #[must_use]
    pub fn is_kinematic(&self) -> bool {
        self.kind == BodyKind::Kinematic
    }

    /// True when both bodies agree to collide: each one's layer must be in the
    /// other's mask.
    #[must_use]
    pub fn can_collide_with(&self, other: &Body) -> bool {
        (self.layer & other.mask) != 0 && (other.layer & self.mask) != 0
    }

    /// Clears the sleep state and restarts the sleep timer.
    pub fn wake(&mut self) {
        self.sleeping = false;
        self.sleep_timer = 0.0;
    }

    /// Applies an instantaneous impulse (kg·m/s) at the centre of mass.
    ///
    /// Ignored for static and kinematic bodies, and for non-finite input.
    pub fn apply_impulse(&mut self, impulse: Vec3) {
        if !self.is_dynamic() || !impulse.is_finite() {
            return;
        }
        self.linear_velocity += impulse * self.inv_mass;
        self.wake();
    }

    /// True when every component of the body state is finite.
    #[must_use]
    pub(crate) fn is_finite(&self) -> bool {
        self.position.is_finite()
            && self.rotation.is_finite()
            && self.linear_velocity.is_finite()
            && self.angular_velocity.is_finite()
    }

    /// True when the body is moving faster than the given thresholds.
    #[must_use]
    pub(crate) fn is_moving_faster_than(&self, linear: f32, angular: f32) -> bool {
        self.linear_velocity.length_squared() > linear * linear
            || self.angular_velocity.length_squared() > angular * angular
    }
}

/// Builder for [`Body`], created through the constructors below.
///
/// ```
/// use noxel_core::math::{Vec3, Quat};
/// use noxel_physics::{BodyDesc, ColliderShape, LAYER_PROP};
///
/// let desc = BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
///     .at(Vec3::new(1.0, 2.0, 3.0))
///     .with_mass(2.0)
///     .with_layer(LAYER_PROP, u32::MAX);
/// ```
#[derive(Clone, Debug)]
pub struct BodyDesc {
    kind: BodyKind,
    shape: ColliderShape,
    position: Vec3,
    rotation: Quat,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    mass: Option<f32>,
    restitution: f32,
    friction: f32,
    linear_damping: f32,
    angular_damping: f32,
    gravity_scale: f32,
    layer: u32,
    mask: u32,
    user_data: u64,
    is_sensor: bool,
    sleeping: bool,
}

impl BodyDesc {
    fn new(kind: BodyKind, shape: ColliderShape) -> Self {
        Self {
            kind,
            shape,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            mass: None,
            restitution: 0.0,
            friction: 0.5,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
            layer: LAYER_WORLD,
            mask: LAYER_ALL,
            user_data: 0,
            is_sensor: false,
            sleeping: false,
        }
    }

    /// A body moved by gravity and contacts.
    #[must_use]
    pub fn dynamic(shape: ColliderShape) -> Self {
        Self::new(BodyKind::Dynamic, shape)
    }

    /// An immovable body. Named `static_body` because `static` is a keyword.
    #[must_use]
    pub fn static_body(shape: ColliderShape) -> Self {
        Self::new(BodyKind::Static, shape)
    }

    /// A body moved only by game code, which pushes dynamic bodies.
    #[must_use]
    pub fn kinematic(shape: ColliderShape) -> Self {
        Self::new(BodyKind::Kinematic, shape)
    }

    /// Sets the world position.
    #[must_use]
    pub fn at(mut self, position: Vec3) -> Self {
        self.position = position;
        self
    }

    /// Sets the orientation.
    #[must_use]
    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Sets the initial linear velocity.
    #[must_use]
    pub fn with_velocity(mut self, linear: Vec3) -> Self {
        self.linear_velocity = linear;
        self
    }

    /// Sets the initial angular velocity.
    #[must_use]
    pub fn with_angular_velocity(mut self, angular: Vec3) -> Self {
        self.angular_velocity = angular;
        self
    }

    /// Sets the mass. Ignored for static and kinematic bodies.
    #[must_use]
    pub fn with_mass(mut self, mass: f32) -> Self {
        self.mass = Some(mass);
        self
    }

    /// Sets the Coulomb friction coefficient.
    #[must_use]
    pub fn with_friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    /// Sets the restitution (bounciness).
    #[must_use]
    pub fn with_restitution(mut self, restitution: f32) -> Self {
        self.restitution = restitution;
        self
    }

    /// Sets the linear and angular damping.
    #[must_use]
    pub fn with_damping(mut self, linear: f32, angular: f32) -> Self {
        self.linear_damping = linear;
        self.angular_damping = angular;
        self
    }

    /// Scales world gravity for this body; `0` makes it float.
    #[must_use]
    pub fn with_gravity_scale(mut self, scale: f32) -> Self {
        self.gravity_scale = scale;
        self
    }

    /// Sets the collision layer membership and mask.
    #[must_use]
    pub fn with_layer(mut self, layer: u32, mask: u32) -> Self {
        self.layer = layer;
        self.mask = mask;
        self
    }

    /// Attaches a free `u64` for gameplay code.
    #[must_use]
    pub fn with_user_data(mut self, user_data: u64) -> Self {
        self.user_data = user_data;
        self
    }

    /// Makes the body a sensor: it reports overlaps but never pushes back.
    #[must_use]
    pub fn as_sensor(mut self) -> Self {
        self.is_sensor = true;
        self
    }

    /// Sets the initial sleep state.
    #[must_use]
    pub fn sleeping(mut self, sleeping: bool) -> Self {
        self.sleeping = sleeping;
        self
    }

    /// Builds the [`Body`], sanitising the shape, the mass and the vectors.
    #[must_use]
    pub(crate) fn build(self) -> Body {
        let shape = sanitize_shape(self.shape);
        let mass = match self.mass {
            Some(m) if m.is_finite() && m > 0.0 => m,
            Some(_) | None => (shape.volume() * DEFAULT_DENSITY).max(1e-3),
        };
        let infinite = self.kind != BodyKind::Dynamic;
        let position = finite_or(self.position, Vec3::ZERO);
        let rotation = finite_or_quat(self.rotation);
        Body {
            kind: self.kind,
            position,
            rotation,
            linear_velocity: finite_or(self.linear_velocity, Vec3::ZERO),
            angular_velocity: finite_or(self.angular_velocity, Vec3::ZERO),
            mass,
            inv_mass: if infinite { 0.0 } else { 1.0 / mass },
            restitution: sanitize_unit(self.restitution),
            friction: sanitize_unit(self.friction),
            linear_damping: sanitize_damping(self.linear_damping),
            angular_damping: sanitize_damping(self.angular_damping),
            gravity_scale: if self.gravity_scale.is_finite() { self.gravity_scale } else { 1.0 },
            is_sensor: self.is_sensor,
            // Only dynamic bodies sleep: static and kinematic bodies are moved
            // by their kind, not by the sleep state, and reporting them as
            // asleep would confuse `stats()` and gameplay code alike.
            sleeping: self.sleeping && self.kind == BodyKind::Dynamic,
            sleep_timer: 0.0,
            layer: self.layer,
            mask: self.mask,
            user_data: self.user_data,
            shape,
        }
    }
}

/// Clamps shape dimensions into a range the collision routines can trust.
#[must_use]
pub(crate) fn sanitize_shape(shape: ColliderShape) -> ColliderShape {
    const MIN: f32 = 1e-4;
    match shape {
        ColliderShape::Box { half_extents } => ColliderShape::Box {
            half_extents: Vec3::new(
                half_extents.x.abs().max(MIN),
                half_extents.y.abs().max(MIN),
                half_extents.z.abs().max(MIN),
            ),
        },
        ColliderShape::Sphere { radius } => {
            ColliderShape::Sphere { radius: radius.abs().max(MIN) }
        }
        ColliderShape::Capsule { radius, half_height } => ColliderShape::Capsule {
            radius: radius.abs().max(MIN),
            half_height: half_height.abs().max(0.0),
        },
        ColliderShape::Cylinder { radius, half_height } => ColliderShape::Cylinder {
            radius: radius.abs().max(MIN),
            half_height: half_height.abs().max(0.0),
        },
    }
}

#[inline]
fn finite_or(v: Vec3, fallback: Vec3) -> Vec3 {
    if v.is_finite() { v } else { fallback }
}

#[inline]
fn finite_or_quat(q: Quat) -> Quat {
    if q.is_finite() { q.normalize() } else { Quat::IDENTITY }
}

#[inline]
fn sanitize_unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

#[inline]
fn sanitize_damping(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_body_has_inverse_mass() {
        let b = BodyDesc::dynamic(ColliderShape::Sphere { radius: 1.0 }).with_mass(4.0).build();
        assert!((b.inv_mass - 0.25).abs() < 1e-6);
        assert!(b.is_dynamic());
    }

    #[test]
    fn static_and_kinematic_bodies_have_infinite_mass() {
        let s = BodyDesc::static_body(ColliderShape::Sphere { radius: 1.0 })
            .with_mass(3.0)
            .build();
        assert_eq!(s.inv_mass, 0.0);
        assert!(s.is_static());
        let k = BodyDesc::kinematic(ColliderShape::Sphere { radius: 1.0 }).build();
        assert_eq!(k.inv_mass, 0.0);
        assert!(k.is_kinematic());
        // Neither kind ever reports itself as asleep: kinematic bodies must
        // keep integrating their velocity.
        assert!(!s.sleeping);
        assert!(!k.sleeping);
        assert!(!s.is_dynamic() && !k.is_dynamic());
    }

    #[test]
    fn default_mass_follows_volume() {
        // A 1x1x1 m box has volume 1, so its default mass is DEFAULT_DENSITY.
        let b = BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) }).build();
        assert!((b.mass - DEFAULT_DENSITY).abs() < 1e-4);
    }

    #[test]
    fn layer_mask_is_symmetric() {
        let a = BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
            .with_layer(LAYER_PLAYER, LAYER_WORLD)
            .build();
        let b = BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 })
            .with_layer(LAYER_WORLD, LAYER_PLAYER)
            .build();
        let c = BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 })
            .with_layer(LAYER_NPC, LAYER_ALL)
            .build();
        assert!(a.can_collide_with(&b));
        assert!(b.can_collide_with(&a));
        assert!(!a.can_collide_with(&c), "player must not collide with an NPC layer");
        assert!(!c.can_collide_with(&a));
    }

    #[test]
    fn impulse_changes_velocity_and_wakes() {
        let mut b = BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
            .with_mass(2.0)
            .sleeping(true)
            .build();
        b.apply_impulse(Vec3::new(4.0, 0.0, 0.0));
        assert!((b.linear_velocity.x - 2.0).abs() < 1e-6, "J/m = 4/2");
        assert!(!b.sleeping);
    }

    #[test]
    fn impulse_is_ignored_for_static_bodies() {
        let mut b = BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }).build();
        b.apply_impulse(Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(b.linear_velocity, Vec3::ZERO);
    }

    #[test]
    fn non_finite_inputs_are_sanitised_at_build_time() {
        let b = BodyDesc::dynamic(ColliderShape::Sphere { radius: 1.0 })
            .at(Vec3::new(f32::NAN, 0.0, 0.0))
            .with_velocity(Vec3::new(f32::INFINITY, 0.0, 0.0))
            .build();
        assert!(b.is_finite());
        assert_eq!(b.position, Vec3::ZERO);
        assert_eq!(b.linear_velocity, Vec3::ZERO);
    }

    #[test]
    fn shape_dimensions_are_clamped() {
        let b = BodyDesc::dynamic(ColliderShape::Sphere { radius: -0.0 }).build();
        assert!(b.shape.bounding_radius() > 0.0);
    }

    #[test]
    fn aabb_tracks_position_and_rotation() {
        let b = BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) })
            .at(Vec3::new(0.0, 1.0, 0.0))
            .build();
        let aabb = b.aabb();
        assert!((aabb.center() - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
        assert!((aabb.half_extents() - Vec3::splat(0.5)).length() < 1e-5);
    }
}
