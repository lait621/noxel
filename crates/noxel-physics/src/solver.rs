//! Sequential-impulse contact solver.
//!
//! The solver is warm-start free: every step rebuilds the contact set, the
//! effective masses and the restitution targets, then runs
//! `solver_iterations` Gauss-Seidel sweeps over the contacts, clamping the
//! *accumulated* normal impulse to be non-negative and the accumulated
//! tangential impulse to the Coulomb cone `|jt| <= mu * jn`.
//!
//! # Angular response is yaw-only
//!
//! Contacts exchange linear momentum plus angular momentum about **Y**. That is
//! the only rotational degree of freedom a top-down RPG needs, and it means a
//! crate can never be tipped over by a friction impulse applied at one corner.
//! `Body::angular_velocity`'s X and Z components still exist and are
//! integrated, so gameplay code can tumble a body deliberately (an explosion,
//! a ragdoll); contacts simply never create them.
//!
//! # Restitution threshold
//!
//! Restitution is only applied when the approach speed exceeds
//! [`RESTITUTION_THRESHOLD`]. Without that gate a resting body bounces on its
//! own numerical noise forever.

use noxel_core::math::Vec3;
use noxel_core::pool::SlotMap;

use crate::body::Body;
use crate::narrow::Contact;
use crate::shape::ColliderShape;

/// Approach speed below which restitution is ignored, in m/s.
///
/// A body landing at 1 m/s would otherwise bounce to a height of `e^2 / (2g)`,
/// which for the default gravity and `e = 0.5` is 6 mm — visibly jittery
/// resting contact for no gameplay benefit.
pub(crate) const RESTITUTION_THRESHOLD: f32 = 1.0;

/// Largest positional correction a single contact may apply in one step, in
/// metres. Stops a deeply-overlapping pair from teleporting apart.
const MAX_CORRECTION: f32 = 0.2;

/// Tangential speed below which friction is not worth solving, in m/s.
const FRICTION_EPSILON: f32 = 1e-5;

/// The per-contact terms that do not change during a velocity sweep.
#[derive(Clone, Copy, Debug)]
struct Kernel {
    inv_ma: f32,
    inv_mb: f32,
    inv_ia: f32,
    inv_ib: f32,
    ra: Vec3,
    rb: Vec3,
}

impl Kernel {
    /// Builds the kernel for a contact, or `None` when neither body can move.
    fn new(body_a: &Body, body_b: &Body, contact: &Contact) -> Option<Self> {
        let inv_ma = movable_inv_mass(body_a);
        let inv_mb = movable_inv_mass(body_b);
        if inv_ma + inv_mb <= 0.0 {
            return None;
        }
        Some(Self {
            inv_ma,
            inv_mb,
            inv_ia: movable_inv_inertia(body_a),
            inv_ib: movable_inv_inertia(body_b),
            ra: contact.point - body_a.position,
            rb: contact.point - body_b.position,
        })
    }

    /// Inverse of the effective mass along `dir`.
    #[inline]
    fn effective_mass(&self, dir: Vec3) -> f32 {
        let ra_cross = self.ra.cross(dir);
        let rb_cross = self.rb.cross(dir);
        self.inv_ma
            + self.inv_mb
            + self.inv_ia * ra_cross.length_squared()
            + self.inv_ib * rb_cross.length_squared()
    }

    /// Relative velocity of `b` with respect to `a` at the contact point.
    #[inline]
    fn relative_velocity(&self, body_a: &Body, body_b: &Body) -> Vec3 {
        let va = body_a.linear_velocity + body_a.angular_velocity.cross(self.ra);
        let vb = body_b.linear_velocity + body_b.angular_velocity.cross(self.rb);
        vb - va
    }

    /// Applies `magnitude * dir` to `b` and its negation to `a`.
    #[inline]
    fn apply(&self, body_a: &mut Body, body_b: &mut Body, dir: Vec3, magnitude: f32) {
        let impulse = dir * magnitude;
        body_a.linear_velocity -= impulse * self.inv_ma;
        body_b.linear_velocity += impulse * self.inv_mb;
        // Yaw only: (r x impulse).y / I_y.
        body_a.angular_velocity.y -= self.ra.cross(dir).y * magnitude * self.inv_ia;
        body_b.angular_velocity.y += self.rb.cross(dir).y * magnitude * self.inv_ib;
    }
}

/// Inverse mass a body contributes to the solver; zero for anything the solver
/// may not move.
#[inline]
fn movable_inv_mass(body: &Body) -> f32 {
    if body.is_dynamic() && !body.sleeping {
        body.inv_mass
    } else {
        0.0
    }
}

/// Inverse yaw moment of inertia a body contributes to the solver.
#[inline]
fn movable_inv_inertia(body: &Body) -> f32 {
    if body.is_dynamic() && !body.sleeping {
        inverse_yaw_inertia(body)
    } else {
        0.0
    }
}

/// The yaw moment of inertia of a body's collider.
///
/// Exact for boxes (`m/3 (hx^2 + hz^2)`), spheres (`2/5 m r^2`) and cylinders
/// (`1/2 m r^2`); a capsule uses its cylindrical part, which under-estimates
/// the hemispherical caps by a few percent.
#[must_use]
pub fn yaw_inertia(shape: &ColliderShape, mass: f32) -> f32 {
    let m = if mass.is_finite() { mass.max(0.0) } else { 0.0 };
    match *shape {
        ColliderShape::Box { half_extents } => {
            let h = half_extents.abs();
            m / 3.0 * (h.x * h.x + h.z * h.z)
        }
        ColliderShape::Sphere { radius } => 0.4 * m * radius * radius,
        ColliderShape::Capsule { radius, .. } => 0.5 * m * radius * radius,
        ColliderShape::Cylinder { radius, .. } => 0.5 * m * radius * radius,
    }
}

/// `1 / yaw_inertia`, or zero when the body cannot rotate.
#[must_use]
pub(crate) fn inverse_yaw_inertia(body: &Body) -> f32 {
    let i = yaw_inertia(&body.shape, body.mass);
    if i > 1e-9 { 1.0 / i } else { 0.0 }
}

/// Reusable per-step solver storage.
///
/// Owning the scratch buffers on the world keeps a step allocation-free after
/// the first few frames.
#[derive(Clone, Debug, Default)]
pub(crate) struct SolverScratch {
    /// Accumulated normal impulse per contact; also the value reported by
    /// collision events.
    normal_impulses: Vec<f32>,
    /// Accumulated tangential impulse per contact.
    friction_impulses: Vec<Vec3>,
    /// Restitution target for the relative normal velocity.
    targets: Vec<f32>,
    /// Cached per-contact terms.
    kernels: Vec<Option<Kernel>>,
}

impl SolverScratch {
    /// The accumulated normal impulse of contact `index`.
    #[must_use]
    pub(crate) fn normal_impulse(&self, index: usize) -> f32 {
        self.normal_impulses.get(index).copied().unwrap_or(0.0)
    }

    /// Rebuilds the kernels and restitution targets for this step's contacts.
    pub(crate) fn prepare(&mut self, bodies: &SlotMap<Body>, contacts: &[Contact]) {
        let n = contacts.len();
        self.normal_impulses.clear();
        self.normal_impulses.resize(n, 0.0);
        self.friction_impulses.clear();
        self.friction_impulses.resize(n, Vec3::ZERO);
        self.targets.clear();
        self.targets.resize(n, 0.0);
        self.kernels.clear();
        self.kernels.resize(n, None);

        for (i, contact) in contacts.iter().enumerate() {
            if contact.is_sensor {
                continue;
            }
            let (Some(body_a), Some(body_b)) = (bodies.get(contact.a), bodies.get(contact.b))
            else {
                continue;
            };
            let Some(kernel) = Kernel::new(body_a, body_b, contact) else {
                continue;
            };
            let vn = kernel.relative_velocity(body_a, body_b).dot(contact.normal);
            let restitution = body_a.restitution.max(body_b.restitution);
            self.targets[i] = if vn < -RESTITUTION_THRESHOLD {
                -restitution * vn
            } else {
                0.0
            };
            self.kernels[i] = Some(kernel);
        }
    }

    /// Runs `iterations` Gauss-Seidel sweeps over the contacts.
    pub(crate) fn solve_velocities(
        &mut self,
        bodies: &mut SlotMap<Body>,
        contacts: &[Contact],
        iterations: u32,
    ) {
        for _ in 0..iterations {
            for (i, contact) in contacts.iter().enumerate() {
                let Some(kernel) = self.kernels.get(i).copied().flatten() else {
                    continue;
                };
                let Some((body_a, body_b)) = bodies.get_two_mut(contact.a, contact.b) else {
                    continue;
                };

                // --- normal impulse -------------------------------------
                let n = contact.normal;
                let mut denom = kernel.effective_mass(n);
                if denom > 1e-12 {
                    let vn = kernel.relative_velocity(body_a, body_b).dot(n);
                    let lambda = (self.targets[i] - vn) / denom;
                    if lambda.is_finite() {
                        let old = self.normal_impulses[i];
                        let new = (old + lambda).max(0.0);
                        if new != old {
                            self.normal_impulses[i] = new;
                            kernel.apply(body_a, body_b, n, new - old);
                        }
                    }
                }

                // --- friction impulse -----------------------------------
                let friction = body_a.friction.min(body_b.friction).max(0.0);
                let bound = friction * self.normal_impulses[i];
                if bound <= 0.0 {
                    continue;
                }
                let rv = kernel.relative_velocity(body_a, body_b);
                let tangential = rv - n * rv.dot(n);
                let speed = tangential.length();
                if speed <= FRICTION_EPSILON {
                    continue;
                }
                let dir = tangential / speed;
                denom = kernel.effective_mass(dir);
                if denom <= 1e-12 {
                    continue;
                }
                let lambda = -speed / denom;
                if !lambda.is_finite() {
                    continue;
                }
                let accumulated = self.friction_impulses[i] + dir * lambda;
                let length = accumulated.length();
                let clamped = if length > bound {
                    accumulated * (bound / length)
                } else {
                    accumulated
                };
                let delta = clamped - self.friction_impulses[i];
                if delta != Vec3::ZERO {
                    self.friction_impulses[i] = clamped;
                    kernel.apply(body_a, body_b, delta.normalize_or_zero(), delta.length());
                }
            }
        }
    }

    /// Pushes overlapping bodies apart (Baumgarte-style positional projection).
    ///
    /// Position correction runs *after* the velocity solve so the impulse it
    /// removes never becomes energy: the bias term of a classic Baumgarte
    /// solver would otherwise make resting bodies creep.
    pub(crate) fn correct_positions(
        &mut self,
        bodies: &mut SlotMap<Body>,
        contacts: &[Contact],
        factor: f32,
    ) {
        for contact in contacts {
            if contact.is_sensor || contact.penetration <= 0.0 {
                continue;
            }
            let Some((body_a, body_b)) = bodies.get_two_mut(contact.a, contact.b) else {
                continue;
            };
            let inv_ma = movable_inv_mass(body_a);
            let inv_mb = movable_inv_mass(body_b);
            let total = inv_ma + inv_mb;
            if total <= 0.0 {
                continue;
            }
            let correction = (contact.penetration * factor).min(MAX_CORRECTION);
            if !correction.is_finite() {
                continue;
            }
            let n = contact.normal;
            body_a.position -= n * (correction * (inv_ma / total));
            body_b.position += n * (correction * (inv_mb / total));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{BodyDesc, BodyHandle};

    fn body(kind: crate::body::BodyKind, shape: ColliderShape, pos: Vec3) -> Body {
        match kind {
            crate::body::BodyKind::Dynamic => BodyDesc::dynamic(shape).at(pos).build(),
            crate::body::BodyKind::Static => BodyDesc::static_body(shape).at(pos).build(),
            crate::body::BodyKind::Kinematic => BodyDesc::kinematic(shape).at(pos).build(),
        }
    }

    fn contact(a: BodyHandle, b: BodyHandle, normal: Vec3, point: Vec3) -> Contact {
        Contact {
            a,
            b,
            normal,
            point,
            penetration: 0.0,
            is_sensor: false,
        }
    }

    #[test]
    fn yaw_inertia_matches_closed_forms() {
        // 1x1x1 box: I_y = m/12 (1^2 + 1^2) = m/6.
        let i = yaw_inertia(
            &ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            },
            1.0,
        );
        assert!((i - 1.0 / 6.0).abs() < 1e-5, "{i}");
        // Solid sphere: 2/5 m r^2.
        let i = yaw_inertia(&ColliderShape::Sphere { radius: 2.0 }, 3.0);
        assert!((i - 0.4 * 3.0 * 4.0).abs() < 1e-5);
        // Cylinder about its axis: 1/2 m r^2.
        let i = yaw_inertia(
            &ColliderShape::Cylinder {
                radius: 0.5,
                half_height: 2.0,
            },
            4.0,
        );
        assert!((i - 0.5 * 4.0 * 0.25).abs() < 1e-5);
    }

    #[test]
    fn approaching_body_is_stopped_by_the_normal_impulse() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::ZERO,
        ));
        let b = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::new(1.0, 0.0, 0.0),
        ));
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::X;
        bodies.get_mut(b).unwrap().linear_velocity = -Vec3::X;
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        let va = bodies.get(a).unwrap().linear_velocity.x;
        let vb = bodies.get(b).unwrap().linear_velocity.x;
        assert!(va.abs() < 1e-4, "a should stop, got {va}");
        assert!(vb.abs() < 1e-4, "b should stop, got {vb}");
        assert!(scratch.normal_impulse(0) > 0.0);
    }

    #[test]
    fn separating_bodies_get_no_impulse() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::ZERO,
        ));
        let b = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::new(1.0, 0.0, 0.0),
        ));
        bodies.get_mut(a).unwrap().linear_velocity = -Vec3::X;
        bodies.get_mut(b).unwrap().linear_velocity = Vec3::X;
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        assert_eq!(scratch.normal_impulse(0), 0.0);
        assert!((bodies.get(a).unwrap().linear_velocity.x + 1.0).abs() < 1e-6);
    }

    #[test]
    fn restitution_reflects_a_fast_impact() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::ZERO,
        ));
        let b = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(1.0, 0.0, 0.0))
                .with_restitution(1.0)
                .build(),
        );
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::new(5.0, 0.0, 0.0);
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        // Equal masses, perfectly elastic: velocities are exchanged.
        assert!((bodies.get(a).unwrap().linear_velocity.x).abs() < 1e-3);
        assert!((bodies.get(b).unwrap().linear_velocity.x - 5.0).abs() < 1e-3);
    }

    #[test]
    fn restitution_is_ignored_below_the_threshold() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::ZERO,
        ));
        let b = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(1.0, 0.0, 0.0))
                .with_restitution(1.0)
                .build(),
        );
        // 0.5 m/s is below RESTITUTION_THRESHOLD.
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::new(0.5, 0.0, 0.0);
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        // Perfectly inelastic: equal masses end at the common velocity 0.25.
        let va = bodies.get(a).unwrap().linear_velocity.x;
        let vb = bodies.get(b).unwrap().linear_velocity.x;
        assert!((va - 0.25).abs() < 1e-3, "{va}");
        assert!((va - vb).abs() < 1e-4, "they move together: {va} vs {vb}");
        assert!(va < 0.5, "the impact cost energy rather than adding it");
    }

    #[test]
    fn static_bodies_are_never_pushed() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::ZERO,
        ));
        let b = bodies.insert(body(
            crate::body::BodyKind::Static,
            ColliderShape::Sphere { radius: 0.5 },
            Vec3::new(1.0, 0.0, 0.0),
        ));
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::new(3.0, 0.0, 0.0);
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 8);
        assert_eq!(bodies.get(b).unwrap().position, Vec3::new(1.0, 0.0, 0.0));
        assert!(bodies.get(a).unwrap().linear_velocity.x.abs() < 1e-3);
    }

    #[test]
    fn kinematic_bodies_push_but_are_not_pushed() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(body(
            crate::body::BodyKind::Kinematic,
            ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            },
            Vec3::ZERO,
        ));
        let b = bodies.insert(body(
            crate::body::BodyKind::Dynamic,
            ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            },
            Vec3::new(0.9, 0.0, 0.0),
        ));
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::new(1.0, 0.0, 0.0);
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 8);
        assert!(
            (bodies.get(a).unwrap().linear_velocity.x - 1.0).abs() < 1e-6,
            "the platform keeps its velocity"
        );
        assert!(
            bodies.get(b).unwrap().linear_velocity.x > 0.5,
            "the crate is pushed"
        );
    }

    #[test]
    fn friction_is_clamped_by_the_friction_cone() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            })
            .with_friction(0.5)
            .build(),
        );
        let b = bodies.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            })
            .with_friction(0.5)
            .build(),
        );
        // Sliding fast in +X while resting on a floor whose normal is +Y.
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::new(10.0, -1.0, 0.0);
        let contacts = [contact(a, b, Vec3::Y, Vec3::new(0.0, -0.5, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 1);
        let jn = scratch.normal_impulse(0);
        let vx = bodies.get(a).unwrap().linear_velocity.x;
        let dv = 10.0 - vx;
        // Equal masses: the friction impulse is exactly the velocity change.
        assert!(
            dv <= 0.5 * jn + 1e-4,
            "dv {dv} must respect mu * jn = {}",
            0.5 * jn
        );
        assert!(vx > 0.0, "one iteration must not reverse the slide");
    }

    #[test]
    fn sleeping_bodies_are_immovable() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).build());
        let b = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(1.0, 0.0, 0.0))
                .sleeping(true)
                .build(),
        );
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::X;
        let contacts = [contact(a, b, Vec3::X, Vec3::new(0.5, 0.0, 0.0))];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        assert!(bodies.get(a).unwrap().linear_velocity.x.abs() < 1e-4);
        assert!(bodies.get(b).unwrap().linear_velocity.x.abs() < 1e-6);
    }

    #[test]
    fn position_correction_splits_by_inverse_mass() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .with_mass(1.0)
                .build(),
        );
        let b = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(0.5, 0.0, 0.0))
                .with_mass(3.0)
                .build(),
        );
        let mut c = contact(a, b, Vec3::X, Vec3::new(0.25, 0.0, 0.0));
        c.penetration = 0.1;
        let mut scratch = SolverScratch::default();
        scratch.correct_positions(&mut bodies, &[c], 1.0);
        let pa = bodies.get(a).unwrap().position.x;
        let pb = bodies.get(b).unwrap().position.x;
        // inv masses 1 and 1/3 -> a moves 3/4 of the correction, b moves 1/4.
        assert!((pa + 0.075).abs() < 1e-5, "{pa}");
        assert!((pb - 0.525).abs() < 1e-5, "{pb}");
    }

    #[test]
    fn sensor_contacts_are_ignored_by_the_solver() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).build());
        let b = bodies.insert(BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }).build());
        bodies.get_mut(a).unwrap().linear_velocity = Vec3::X;
        let mut c = contact(a, b, Vec3::X, Vec3::ZERO);
        c.is_sensor = true;
        c.penetration = 0.5;
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &[c]);
        scratch.solve_velocities(&mut bodies, &[c], 4);
        scratch.correct_positions(&mut bodies, &[c], 1.0);
        assert!((bodies.get(a).unwrap().linear_velocity.x - 1.0).abs() < 1e-6);
        assert_eq!(bodies.get(a).unwrap().position, Vec3::ZERO);
    }

    #[test]
    fn non_finite_impulses_cannot_poison_a_solve() {
        let mut bodies = SlotMap::new();
        let a = bodies.insert(BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).build());
        let b = bodies.insert(
            BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(1.0, 0.0, 0.0))
                .build(),
        );
        // The descriptor sanitises its inputs, so the poison has to be written
        // straight into the body the way a gameplay bug would.
        bodies.get_mut(b).unwrap().position = Vec3::new(f32::NAN, 0.0, 0.0);
        let contacts = [contact(a, b, Vec3::X, Vec3::ZERO)];
        let mut scratch = SolverScratch::default();
        scratch.prepare(&bodies, &contacts);
        scratch.solve_velocities(&mut bodies, &contacts, 4);
        assert!(bodies.get(a).unwrap().linear_velocity.is_finite());
    }
}
