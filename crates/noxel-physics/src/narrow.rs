//! Narrowphase: closest features, separations and contact generation.
//!
//! Every pair of shapes is reduced to a single *feature pair*: the closest
//! point on `a`, the closest point on `b`, a unit normal pointing from `a`
//! towards `b` and the signed separation between them. A contact exists when
//! that separation is not larger than the world's `contact_tolerance`, and the
//! penetration depth is `-separation`.
//!
//! The same routine drives [contact generation](contact_between) and the
//! character controller's conservative-advancement sweep, so a character can
//! never come to rest somewhere the solver disagrees with.
//!
//! # Shape pairs
//!
//! Colliders are first reduced to a [`Prim`] (box, sphere or capsule), which is
//! what makes the dispatcher small and total:
//!
//! | Pair | Method |
//! |---|---|
//! | sphere/sphere | analytic |
//! | sphere/box | closest point on the box (analytic, including the "centre inside" case) |
//! | sphere/capsule | closest point on the axis segment |
//! | capsule/capsule | closest points between two segments |
//! | capsule/box | exact closest point between a segment and an AABB |
//! | box/box | 15-axis SAT with minimum translation vector |
//! | any/cylinder | the cylinder is replaced by its local bounding box |
//!
//! Boxes are treated as fully oriented (the SAT is general), but the solver
//! only ever exchanges angular momentum about Y — see `solver.rs`.
//!
//! # Manifold size
//!
//! One contact point per pair, placed at the centroid of the incident box's
//! penetrating vertices for box/box. A single well-placed point is enough for a
//! top-down game (nothing stacks more than two or three deep) and it keeps the
//! solver branch-free and deterministic.

use noxel_core::math::{Quat, Vec3};

use crate::body::{Body, BodyHandle};
use crate::shape::{CONTACT_EPSILON, ColliderShape};

/// The closest pair of features between two shapes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Features {
    /// The closest point on shape `a`, in world space.
    pub point_a: Vec3,
    /// The closest point on shape `b`, in world space.
    pub point_b: Vec3,
    /// Unit normal pointing from `a` towards `b`.
    pub normal: Vec3,
    /// Signed distance between the features; negative when they overlap.
    pub separation: f32,
}

impl Features {
    /// The same features seen from `b`: normals and points swap.
    #[must_use]
    fn flipped(self) -> Self {
        Self {
            point_a: self.point_b,
            point_b: self.point_a,
            normal: -self.normal,
            separation: self.separation,
        }
    }
}

/// One contact point between two bodies.
///
/// Contacts are rebuilt from scratch every step, so `contacts()` always
/// describes the state the solver just acted on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// The body with the lower handle.
    pub a: BodyHandle,
    /// The body with the higher handle.
    pub b: BodyHandle,
    /// Unit normal pointing from `a` towards `b`.
    pub normal: Vec3,
    /// World-space contact point, at the middle of the overlap region.
    pub point: Vec3,
    /// Penetration depth in metres; zero for a body pair that is merely
    /// touching inside the contact tolerance.
    pub penetration: f32,
    /// True when either body is a sensor. Sensor contacts never push.
    pub is_sensor: bool,
}

/// Builds the contact between two bodies, or `None` when they are further
/// apart than `tolerance`.
#[must_use]
pub(crate) fn contact_between(
    a: BodyHandle,
    b: BodyHandle,
    body_a: &Body,
    body_b: &Body,
    tolerance: f32,
) -> Option<Contact> {
    let f = closest_features(
        &body_a.shape,
        body_a.position,
        body_a.rotation,
        &body_b.shape,
        body_b.position,
        body_b.rotation,
    );
    if !f.separation.is_finite() || f.separation > tolerance {
        // A non-finite separation means a shape is poisoned: report no contact
        // rather than a contact that would poison the solver.
        return None;
    }
    let midpoint = (f.point_a + f.point_b) * 0.5;
    Some(Contact {
        a,
        b,
        normal: f.normal,
        point: if midpoint.is_finite() {
            midpoint
        } else {
            f.point_a
        },
        penetration: (-f.separation).max(0.0),
        is_sensor: body_a.is_sensor || body_b.is_sensor,
    })
}

/// A collider reduced to the primitive the narrowphase works with.
///
/// Cylinders arrive here already replaced by their bounding box
/// ([`ColliderShape::collision_shape`]).
#[derive(Clone, Copy, Debug)]
enum Prim {
    /// An oriented box: centre, three orthonormal world axes, half extents.
    Box {
        /// Centre of the box.
        center: Vec3,
        /// World-space unit axes of the box.
        axes: [Vec3; 3],
        /// Half extents along the local axes.
        half: Vec3,
    },
    /// A sphere.
    Sphere {
        /// Centre of the sphere.
        center: Vec3,
        /// Radius of the sphere.
        radius: f32,
    },
    /// A capsule: the ends of its axis segment, swept by `radius`.
    Capsule {
        /// Start of the axis segment.
        a0: Vec3,
        /// End of the axis segment.
        a1: Vec3,
        /// Radius of the swept sphere.
        radius: f32,
    },
}

/// Places a collider in the world as a [`Prim`].
#[must_use]
fn primitive(shape: &ColliderShape, position: Vec3, rotation: Quat) -> Prim {
    match *shape {
        ColliderShape::Box { half_extents } => Prim::Box {
            center: position,
            axes: box_axes(rotation),
            half: half_extents.abs(),
        },
        ColliderShape::Sphere { radius } => Prim::Sphere {
            center: position,
            radius: radius.abs(),
        },
        ColliderShape::Capsule {
            radius,
            half_height,
        } => {
            let (a0, a1) = capsule_axis(position, rotation, half_height.abs());
            Prim::Capsule {
                a0,
                a1,
                radius: radius.abs(),
            }
        }
        // Unreachable through `collision_shape`, but recursing keeps the match
        // total instead of panicking.
        ColliderShape::Cylinder { .. } => primitive(&shape.collision_shape(), position, rotation),
    }
}

/// The closest features of two shapes placed in the world.
#[must_use]
pub(crate) fn closest_features(
    shape_a: &ColliderShape,
    pos_a: Vec3,
    rot_a: Quat,
    shape_b: &ColliderShape,
    pos_b: Vec3,
    rot_b: Quat,
) -> Features {
    // Cylinders collide as their bounding box; every branch below therefore
    // only has to handle boxes, spheres and capsules.
    let a = primitive(&shape_a.collision_shape(), pos_a, rot_a);
    let b = primitive(&shape_b.collision_shape(), pos_b, rot_b);

    match (&a, &b) {
        (Prim::Sphere { center, radius }, other) => sphere_primitive(*center, *radius, other),
        (other, Prim::Sphere { center, radius }) => {
            sphere_primitive(*center, *radius, other).flipped()
        }
        (
            Prim::Capsule { a0, a1, radius },
            Prim::Capsule {
                a0: b0,
                a1: b1,
                radius: rb,
            },
        ) => capsule_capsule(*a0, *a1, *radius, *b0, *b1, *rb),
        (Prim::Capsule { a0, a1, radius }, Prim::Box { center, axes, half }) => {
            capsule_box(*a0, *a1, *radius, *center, axes, *half)
        }
        (Prim::Box { center, axes, half }, Prim::Capsule { a0, a1, radius }) => {
            capsule_box(*a0, *a1, *radius, *center, axes, *half).flipped()
        }
        (
            Prim::Box {
                center: ca,
                axes: ax,
                half: ha,
            },
            Prim::Box {
                center: cb,
                axes: bx,
                half: hb,
            },
        ) => box_box(*ca, ax, *ha, *cb, bx, *hb),
    }
}

/// Sphere against any other primitive. The normal points from the sphere to the
/// other shape.
fn sphere_primitive(center: Vec3, radius: f32, other: &Prim) -> Features {
    match *other {
        Prim::Sphere {
            center: other_center,
            radius: other_radius,
        } => sphere_sphere(center, radius, other_center, other_radius),
        Prim::Box {
            center: box_center,
            axes,
            half,
        } => sphere_box(center, radius, box_center, &axes, half),
        Prim::Capsule {
            a0,
            a1,
            radius: cap_radius,
        } => {
            let closest = closest_point_on_segment(center, a0, a1);
            sphere_sphere(center, radius, closest, cap_radius)
        }
    }
}

/// The two endpoints of a Y-aligned capsule axis.
#[inline]
fn capsule_axis(center: Vec3, rotation: Quat, half_height: f32) -> (Vec3, Vec3) {
    let axis = rotation.rotate_vec3(Vec3::Y) * half_height;
    (center - axis, center + axis)
}

/// Sphere against sphere.
fn sphere_sphere(center_a: Vec3, radius_a: f32, center_b: Vec3, radius_b: f32) -> Features {
    let delta = center_b - center_a;
    let distance = delta.length();
    let normal = if distance > CONTACT_EPSILON {
        delta / distance
    } else {
        // Coincident centres: any axis is valid, Y keeps it deterministic.
        Vec3::Y
    };
    Features {
        point_a: center_a + normal * radius_a,
        point_b: center_b - normal * radius_b,
        normal,
        separation: distance - radius_a - radius_b,
    }
}

/// Sphere against box. The normal points from the sphere to the box.
fn sphere_box(
    center: Vec3,
    radius: f32,
    box_center: Vec3,
    axes: &[Vec3; 3],
    half: Vec3,
) -> Features {
    let half = half.abs();
    let local = to_local(center - box_center, axes);
    let clamped = local.clamp(-half, half);
    let delta = local - clamped;
    let distance = delta.length();

    if distance > CONTACT_EPSILON {
        // Sphere centre outside the box: `delta` points from the box surface
        // towards the sphere, so the sphere->box normal is `-delta`.
        let to_sphere = delta / distance;
        let point_a_local = local - to_sphere * radius;
        Features {
            point_a: box_center + to_world(point_a_local, axes),
            point_b: box_center + to_world(clamped, axes),
            normal: to_world(-to_sphere, axes),
            separation: distance - radius,
        }
    } else {
        // Sphere centre inside the box: escape through the nearest face.
        let mut axis = 0usize;
        let mut depth = half.x - local.x.abs();
        for i in 1..3 {
            let d = half[i] - local[i].abs();
            if d < depth {
                depth = d;
                axis = i;
            }
        }
        let mut escape = Vec3::ZERO;
        escape[axis] = if local[axis] >= 0.0 { 1.0 } else { -1.0 };
        let point_a_local = local + escape * radius;
        let point_b_local = local + escape * depth;
        Features {
            point_a: box_center + to_world(point_a_local, axes),
            point_b: box_center + to_world(point_b_local, axes),
            normal: to_world(-escape, axes),
            separation: -(depth + radius),
        }
    }
}

/// Capsule against capsule, given each capsule's axis segment.
fn capsule_capsule(
    a0: Vec3,
    a1: Vec3,
    radius_a: f32,
    b0: Vec3,
    b1: Vec3,
    radius_b: f32,
) -> Features {
    let (qa, qb) = closest_points_segments(a0, a1, b0, b1);
    let delta = qb - qa;
    let distance = delta.length();
    let normal = if distance > CONTACT_EPSILON {
        delta / distance
    } else {
        let fallback = (b0 + b1) - (a0 + a1);
        if fallback.length_squared() > CONTACT_EPSILON {
            fallback.normalize_or_zero()
        } else {
            Vec3::Y
        }
    };
    Features {
        point_a: qa + normal * radius_a,
        point_b: qb - normal * radius_b,
        normal,
        separation: distance - radius_a - radius_b,
    }
}

/// Capsule against box. The normal points from the capsule to the box.
fn capsule_box(
    a0: Vec3,
    a1: Vec3,
    radius: f32,
    box_center: Vec3,
    axes: &[Vec3; 3],
    half: Vec3,
) -> Features {
    let half = half.abs();
    let l0 = to_local(a0 - box_center, axes);
    let l1 = to_local(a1 - box_center, axes);
    let (seg_local, box_local) = closest_point_segment_aabb(l0, l1, half);
    let delta = box_local - seg_local;
    let distance = delta.length();

    if distance > CONTACT_EPSILON {
        let to_box = delta / distance;
        Features {
            point_a: box_center + to_world(seg_local + to_box * radius, axes),
            point_b: box_center + to_world(box_local, axes),
            normal: to_world(to_box, axes),
            separation: distance - radius,
        }
    } else {
        // The capsule axis lies inside the box: escape through the nearest
        // face of the segment midpoint.
        let mid = (l0 + l1) * 0.5;
        let mut axis = 0usize;
        let mut depth = half.x - mid.x.abs();
        for i in 1..3 {
            let d = half[i] - mid[i].abs();
            if d < depth {
                depth = d;
                axis = i;
            }
        }
        let mut escape = Vec3::ZERO;
        escape[axis] = if mid[axis] >= 0.0 { 1.0 } else { -1.0 };
        Features {
            point_a: box_center + to_world(mid + escape * radius, axes),
            point_b: box_center + to_world(mid + escape * depth, axes),
            normal: to_world(-escape, axes),
            separation: -(depth + radius),
        }
    }
}

/// Box against box: 15-axis separating-axis test with the minimum translation
/// vector.
///
/// Any pitch or roll on the bodies is honoured — the SAT is general — but the
/// *solver* only exchanges yaw, so boxes cannot topple (see `solver.rs`).
fn box_box(
    pos_a: Vec3,
    axes_a: &[Vec3; 3],
    half_a: Vec3,
    pos_b: Vec3,
    axes_b: &[Vec3; 3],
    half_b: Vec3,
) -> Features {
    let ax = *axes_a;
    let bx = *axes_b;
    let half_a = half_a.abs();
    let half_b = half_b.abs();
    let t = pos_b - pos_a;

    // The minimum-overlap axis and, separately, the largest gap found (a lower
    // bound of the true distance when the boxes are apart).
    let mut min_overlap = f32::INFINITY;
    let mut mtv = Vec3::Y;
    let mut mtv_ref_is_a = true;
    let mut separated = false;
    let mut max_separation = f32::NEG_INFINITY;
    let mut sep_normal = Vec3::Y;

    {
        let mut consider = |axis: Vec3, ref_is_a: bool| {
            let len_sq = axis.length_squared();
            if len_sq < 1e-10 {
                // Parallel edge pair: the cross product carries no information.
                return;
            }
            let axis = axis * (1.0 / len_sq.sqrt());
            let ra = project_extent(&ax, half_a, axis);
            let rb = project_extent(&bx, half_b, axis);
            let signed = t.dot(axis);
            let overlap = ra + rb - signed.abs();
            let sign = if signed >= 0.0 { 1.0 } else { -1.0 };
            if overlap < min_overlap {
                min_overlap = overlap;
                mtv = axis * sign;
                mtv_ref_is_a = ref_is_a;
            }
            if overlap < 0.0 {
                separated = true;
                let separation = -overlap;
                if separation > max_separation {
                    max_separation = separation;
                    sep_normal = axis * sign;
                }
            }
        };
        for axis in ax {
            consider(axis, true);
        }
        for axis in bx {
            consider(axis, false);
        }
        for axis_a in ax {
            for axis_b in bx {
                consider(axis_a.cross(axis_b), true);
            }
        }
    }

    if separated {
        // Conservative advancement only needs a lower bound of the distance,
        // and a bound that never over-estimates can never let a character
        // tunnel through the gap.
        let point_a = pos_a + support_in(&ax, half_a, sep_normal);
        let point_b = pos_b + support_in(&bx, half_b, -sep_normal);
        return Features {
            point_a,
            point_b,
            normal: sep_normal,
            separation: max_separation,
        };
    }

    // Touching or overlapping: place the contact at the centroid of the
    // incident box's penetrating vertices, which keeps a box resting flat on
    // the floor from being spun by friction at one corner.
    let (point_a, point_b) = if mtv_ref_is_a {
        // `a` carries the reference face; `b` is incident and lies on the +mtv
        // side of `a`.
        let (reference, incident) = contact_features(pos_a, &ax, half_a, pos_b, &bx, half_b, mtv);
        (reference, incident)
    } else {
        let (reference, incident) = contact_features(pos_b, &bx, half_b, pos_a, &ax, half_a, -mtv);
        (incident, reference)
    };
    Features {
        point_a,
        point_b,
        normal: mtv,
        separation: -min_overlap,
    }
}

/// The three orthonormal axes of a box, as world-space unit vectors.
#[inline]
fn box_axes(rotation: Quat) -> [Vec3; 3] {
    [
        rotation.rotate_vec3(Vec3::X).normalize_or_zero(),
        rotation.rotate_vec3(Vec3::Y).normalize_or_zero(),
        rotation.rotate_vec3(Vec3::Z).normalize_or_zero(),
    ]
}

/// World vector -> box-local vector.
#[inline]
fn to_local(v: Vec3, axes: &[Vec3; 3]) -> Vec3 {
    Vec3::new(v.dot(axes[0]), v.dot(axes[1]), v.dot(axes[2]))
}

/// Box-local vector -> world vector.
#[inline]
fn to_world(local: Vec3, axes: &[Vec3; 3]) -> Vec3 {
    axes[0] * local.x + axes[1] * local.y + axes[2] * local.z
}

/// The support extent of a box along `dir`: `sum |half_i * axis_i . dir|`.
#[inline]
fn project_extent(axes: &[Vec3; 3], half: Vec3, dir: Vec3) -> f32 {
    axes[0].dot(dir).abs() * half.x
        + axes[1].dot(dir).abs() * half.y
        + axes[2].dot(dir).abs() * half.z
}

/// The support point of a box along `dir`, relative to its centre.
#[inline]
fn support_in(axes: &[Vec3; 3], half: Vec3, dir: Vec3) -> Vec3 {
    let mut out = Vec3::ZERO;
    for i in 0..3 {
        let sign = if axes[i].dot(dir) >= 0.0 { 1.0 } else { -1.0 };
        out += axes[i] * (half[i] * sign);
    }
    out
}

/// The eight world-space corners of a box.
#[inline]
fn box_vertices(center: Vec3, axes: &[Vec3; 3], half: Vec3) -> [Vec3; 8] {
    let mut out = [Vec3::ZERO; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        let sx = if i & 1 == 0 { half.x } else { -half.x };
        let sy = if i & 2 == 0 { half.y } else { -half.y };
        let sz = if i & 4 == 0 { half.z } else { -half.z };
        *slot = center + axes[0] * sx + axes[1] * sy + axes[2] * sz;
    }
    out
}

/// Contact geometry for an overlapping box pair.
///
/// Returns the point on the reference face plane and the point on the incident
/// box, given the direction from the reference box towards the incident one.
fn contact_features(
    ref_center: Vec3,
    ref_axes: &[Vec3; 3],
    ref_half: Vec3,
    inc_center: Vec3,
    inc_axes: &[Vec3; 3],
    inc_half: Vec3,
    dir_to_incident: Vec3,
) -> (Vec3, Vec3) {
    let extent_ref = project_extent(ref_axes, ref_half, dir_to_incident);
    let vertices = box_vertices(inc_center, inc_axes, inc_half);
    let mut sum = Vec3::ZERO;
    let mut count = 0usize;
    let mut deepest = vertices[0];
    let mut deepest_projection = f32::INFINITY;
    for vertex in vertices {
        let local = vertex - ref_center;
        let inside = (0..3).all(|i| local.dot(ref_axes[i]).abs() <= ref_half[i] + CONTACT_EPSILON);
        if inside {
            sum += vertex;
            count += 1;
        }
        let projection = local.dot(dir_to_incident);
        if projection < deepest_projection {
            deepest_projection = projection;
            deepest = vertex;
        }
    }
    let incident = if count > 0 {
        sum * (1.0 / count as f32)
    } else {
        deepest
    };
    let signed = (incident - ref_center).dot(dir_to_incident) - extent_ref;
    let reference = incident - dir_to_incident * signed;
    (reference, incident)
}

/// The closest point on the segment `p0..p1` to `p`.
#[inline]
#[must_use]
pub(crate) fn closest_point_on_segment(p: Vec3, p0: Vec3, p1: Vec3) -> Vec3 {
    let d = p1 - p0;
    let len_sq = d.length_squared();
    if len_sq <= CONTACT_EPSILON {
        return p0;
    }
    p0 + d * ((p - p0).dot(d) / len_sq).clamp(0.0, 1.0)
}

/// The closest pair of points on two segments (Ericson, *Real-Time Collision
/// Detection* section 5.1.9).
#[must_use]
pub(crate) fn closest_points_segments(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.length_squared();
    let e = d2.length_squared();
    let f = d2.dot(r);
    const EPS: f32 = 1e-10;

    let (s, t);
    if a <= EPS && e <= EPS {
        return (p1, p2);
    } else if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s_raw = if denom > EPS {
                (b * f - c * e) / denom
            } else {
                0.0
            };
            s_raw = s_raw.clamp(0.0, 1.0);
            let mut t_raw = (b * s_raw + f) / e;
            if t_raw < 0.0 {
                t_raw = 0.0;
                s_raw = (-c / a).clamp(0.0, 1.0);
            } else if t_raw > 1.0 {
                t_raw = 1.0;
                s_raw = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s_raw;
            t = t_raw;
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

/// Exact closest point between the segment `p0..p1` and the axis-aligned box
/// centred on the origin with the given half extents.
///
/// Returns `(point_on_segment, point_on_box)`. The distance function along the
/// segment is convex and piecewise quadratic, with breakpoints exactly where a
/// coordinate crosses a face plane, so minimising each piece in closed form
/// gives the exact answer with no iteration.
#[must_use]
pub(crate) fn closest_point_segment_aabb(p0: Vec3, p1: Vec3, half: Vec3) -> (Vec3, Vec3) {
    let half = half.abs();
    let d = p1 - p0;
    let len_sq = d.length_squared();
    if len_sq <= CONTACT_EPSILON {
        let q = p0.clamp(-half, half);
        return (p0, q);
    }

    // Breakpoints: parameter values where the segment crosses a face plane.
    let mut breaks = [0.0f32; 8];
    let mut count = 2usize;
    breaks[0] = 0.0;
    breaks[1] = 1.0;
    for i in 0..3 {
        if d[i].abs() > 1e-12 {
            for bound in [-half[i], half[i]] {
                let t = (bound - p0[i]) / d[i];
                // Three axes with two bounds each plus the two endpoints fit in
                // the array exactly; the guard keeps that invariant explicit.
                if t > 0.0 && t < 1.0 && count < breaks.len() {
                    breaks[count] = t;
                    count += 1;
                }
            }
        }
    }
    // Insertion sort: at most eight values, so this beats any cleverness.
    for i in 1..count {
        let value = breaks[i];
        let mut j = i;
        while j > 0 && breaks[j - 1] > value {
            breaks[j] = breaks[j - 1];
            j -= 1;
        }
        breaks[j] = value;
    }

    let mut best_seg = p0;
    let mut best_box = p0.clamp(-half, half);
    let mut best_dist = (best_box - p0).length_squared();

    for window in 0..count.saturating_sub(1) {
        let (t0, t1) = (breaks[window], breaks[window + 1]);
        if t1 - t0 <= 0.0 {
            continue;
        }
        // The closest point on the box is constant across this piece.
        let mid = p0 + d * ((t0 + t1) * 0.5);
        let box_point = mid.clamp(-half, half);
        // Stationary point of |p0 + d*t - box_point|^2 is linear in t.
        let t_star = (t0 + d.dot(box_point - p0) / len_sq).clamp(t0, t1);
        let seg_point = p0 + d * t_star;
        let q = seg_point.clamp(-half, half);
        let dist = (q - seg_point).length_squared();
        if dist < best_dist {
            best_dist = dist;
            best_seg = seg_point;
            best_box = q;
        }
    }
    (best_seg, best_box)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn sphere(r: f32) -> ColliderShape {
        ColliderShape::Sphere { radius: r }
    }
    fn boxx(h: f32) -> ColliderShape {
        ColliderShape::Box {
            half_extents: Vec3::splat(h),
        }
    }
    fn capsule(r: f32, hh: f32) -> ColliderShape {
        ColliderShape::Capsule {
            radius: r,
            half_height: hh,
        }
    }

    fn features(a: &ColliderShape, pa: Vec3, b: &ColliderShape, pb: Vec3) -> Features {
        closest_features(a, pa, Quat::IDENTITY, b, pb, Quat::IDENTITY)
    }

    #[test]
    fn sphere_sphere_normal_points_from_a_to_b() {
        let f = features(
            &sphere(1.0),
            Vec3::ZERO,
            &sphere(1.0),
            Vec3::new(3.0, 0.0, 0.0),
        );
        assert!((f.normal - Vec3::X).length() < EPS);
        assert!((f.separation - 1.0).abs() < EPS, "3 - 1 - 1");
        assert!((f.point_a - Vec3::X).length() < EPS);
        assert!((f.point_b - Vec3::new(2.0, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn sphere_sphere_penetration_is_negative_separation() {
        let f = features(
            &sphere(1.0),
            Vec3::ZERO,
            &sphere(1.0),
            Vec3::new(1.5, 0.0, 0.0),
        );
        assert!((f.separation + 0.5).abs() < EPS, "1.5 - 2.0");
    }

    #[test]
    fn sphere_sphere_symmetry() {
        let a = features(
            &sphere(0.5),
            Vec3::new(1.0, 2.0, 3.0),
            &sphere(2.0),
            Vec3::new(-1.0, 0.0, 0.0),
        );
        let b = features(
            &sphere(2.0),
            Vec3::new(-1.0, 0.0, 0.0),
            &sphere(0.5),
            Vec3::new(1.0, 2.0, 3.0),
        );
        assert!((a.normal + b.normal).length() < EPS);
        assert!((a.separation - b.separation).abs() < EPS);
        assert!((a.point_a - b.point_b).length() < EPS);
    }

    #[test]
    fn sphere_box_face_contact() {
        // Sphere of radius 0.5 at y = 1.5 above a unit box whose top is y = 0.5.
        let f = features(
            &sphere(0.5),
            Vec3::new(0.0, 1.5, 0.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        assert!((f.separation - 0.5).abs() < EPS, "gap = 1.5 - 0.5 - 0.5");
        assert!((f.normal - Vec3::DOWN).length() < EPS, "{:?}", f.normal);
    }

    #[test]
    fn sphere_box_corner_uses_corner_normal() {
        // Sphere beyond the box corner (0.5, 0.5, 0.5); centre to corner is
        // (0.5, 0.5, 0.5), length sqrt(0.75) = 0.8660.
        let f = features(
            &sphere(0.25),
            Vec3::new(1.0, 1.0, 1.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        let expected = Vec3::splat(0.5).length() - 0.25;
        assert!(
            (f.separation - expected).abs() < EPS,
            "{} vs {}",
            f.separation,
            expected
        );
        assert!((f.normal.length() - 1.0).abs() < EPS);
        // The normal points from the sphere (a) towards the box (b).
        assert!(
            (f.normal + Vec3::splat(1.0 / 3.0f32.sqrt())).length() < EPS,
            "{:?}",
            f.normal
        );
    }

    #[test]
    fn sphere_inside_box_escapes_through_nearest_face() {
        // Centre 0.1 below the top face: depth to the top = 0.5 - 0.4 = 0.1,
        // so penetration = 0.1 + radius.
        let f = features(
            &sphere(0.25),
            Vec3::new(0.0, 0.4, 0.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        assert!((f.separation + 0.35).abs() < EPS, "{}", f.separation);
        // Normal points from the sphere towards the box, i.e. downwards
        // because the sphere escapes upwards.
        assert!((f.normal + Vec3::Y).length() < EPS, "{:?}", f.normal);
    }

    #[test]
    fn box_box_axis_aligned_uses_minimum_translation_axis() {
        // Two unit boxes offset by (0.8, 0, 0): overlap on X is 0.2, on Y and
        // Z it is 1.0, so the MTV is +X with depth 0.2.
        let f = features(&boxx(0.5), Vec3::ZERO, &boxx(0.5), Vec3::new(0.8, 0.0, 0.0));
        assert!((f.normal - Vec3::X).length() < EPS, "{:?}", f.normal);
        assert!((f.separation + 0.2).abs() < EPS, "{}", f.separation);
    }

    #[test]
    fn box_box_stacked_is_stable_and_vertical() {
        let f = features(&boxx(0.5), Vec3::ZERO, &boxx(0.5), Vec3::new(0.0, 0.9, 0.0));
        assert!((f.normal - Vec3::Y).length() < EPS);
        assert!((f.separation + 0.1).abs() < EPS);
        // Contact point is the centroid of the incident box's four penetrating
        // bottom vertices: the centre of that face.
        let midpoint = (f.point_a + f.point_b) * 0.5;
        assert!(
            midpoint.x.abs() < EPS && midpoint.z.abs() < EPS,
            "{midpoint:?}"
        );
        assert!((midpoint.y - 0.45).abs() < 0.05, "{midpoint:?}");
    }

    #[test]
    fn box_box_separated_reports_positive_gap() {
        let f = features(&boxx(0.5), Vec3::ZERO, &boxx(0.5), Vec3::new(3.0, 0.0, 0.0));
        assert!(f.separation > 0.0);
        assert!((f.separation - 2.0).abs() < EPS, "3 - 0.5 - 0.5");
    }

    #[test]
    fn box_box_yawed_edge_axis_is_found() {
        // A yawed box overlapping a flat one: SAT must find the cross axis,
        // and the result must still be a unit normal with a sensible depth.
        let a = ColliderShape::Box {
            half_extents: Vec3::new(2.0, 0.5, 0.2),
        };
        let b = ColliderShape::Box {
            half_extents: Vec3::new(2.0, 0.5, 0.2),
        };
        let f = closest_features(
            &a,
            Vec3::ZERO,
            Quat::IDENTITY,
            &b,
            Vec3::new(0.0, 0.0, 0.3),
            Quat::IDENTITY,
        );
        assert!((f.normal.length() - 1.0).abs() < EPS);
        assert!(f.separation < 0.0);
    }

    #[test]
    fn capsule_box_resting_on_top() {
        // Capsule (r = 0.4, half height 0.5) lying vertically with its bottom
        // cap centre at y = 0.9, above a box whose top is y = 0.5.
        let f = features(
            &capsule(0.4, 0.5),
            Vec3::new(0.0, 1.4, 0.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        assert!((f.separation - 0.0).abs() < EPS, "0.9 - 0.5 - 0.4 = 0");
        assert!((f.normal - Vec3::DOWN).length() < EPS, "{:?}", f.normal);
    }

    #[test]
    fn capsule_box_penetration_matches_overlap() {
        let f = features(
            &capsule(0.4, 0.5),
            Vec3::new(0.0, 1.3, 0.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        assert!((f.separation + 0.1).abs() < EPS, "{}", f.separation);
    }

    #[test]
    fn capsule_box_side_contact_is_horizontal() {
        // Capsule axis on the +X side of the box, 0.9 from centre.
        let f = features(
            &capsule(0.25, 0.5),
            Vec3::new(0.9, 0.0, 0.0),
            &boxx(0.5),
            Vec3::ZERO,
        );
        assert!((f.normal + Vec3::X).length() < EPS, "{:?}", f.normal);
        assert!((f.separation - 0.15).abs() < EPS, "0.9 - 0.5 - 0.25");
    }

    #[test]
    fn capsule_capsule_parallel_gap() {
        let a = capsule(0.5, 1.0);
        let b = capsule(0.5, 1.0);
        let f = features(&a, Vec3::ZERO, &b, Vec3::new(2.0, 0.0, 0.0));
        assert!((f.separation - 1.0).abs() < EPS, "2 - 0.5 - 0.5");
        assert!((f.normal - Vec3::X).length() < EPS);
    }

    #[test]
    fn capsule_capsule_crossed_touches_through_sides() {
        let a = capsule(0.5, 1.0);
        let b = capsule(0.5, 1.0);
        let f = closest_features(
            &a,
            Vec3::ZERO,
            Quat::IDENTITY,
            &b,
            Vec3::new(0.0, 0.0, 0.8),
            Quat::from_rotation_z(core::f32::consts::FRAC_PI_2),
        );
        // `a` spans y in [-1, 1], `b` spans x in [-1, 1] at z = 0.8; the
        // segments cross at 0.8 apart, so the caps overlap by 0.2.
        assert!((f.separation + 0.2).abs() < EPS, "{}", f.separation);
        assert!((f.normal - Vec3::Z).length() < EPS, "{:?}", f.normal);
    }

    #[test]
    fn cylinder_collides_as_its_bounding_box() {
        let cyl = ColliderShape::Cylinder {
            radius: 0.5,
            half_height: 1.0,
        };
        let f = features(&sphere(0.5), Vec3::new(0.0, 2.0, 0.0), &cyl, Vec3::ZERO);
        // Box top at y = 1.0, sphere bottom at 1.5 -> gap 0.5.
        assert!((f.separation - 0.5).abs() < EPS, "{}", f.separation);
    }

    #[test]
    fn contact_between_uses_tolerance() {
        use crate::body::BodyDesc;
        use noxel_core::pool::Handle;
        let make = |y: f32| {
            BodyDesc::dynamic(sphere(0.5))
                .at(Vec3::new(0.0, y, 0.0))
                .build()
        };
        let a = make(0.0);
        let b = make(1.004);
        let c = make(1.006);
        let (ha, hb, hc) = (
            Handle::from_bits(1),
            Handle::from_bits(2),
            Handle::from_bits(3),
        );
        assert!(
            contact_between(ha, hb, &a, &b, 0.005).is_some(),
            "gap 0.004 <= 0.005"
        );
        assert!(
            contact_between(ha, hc, &a, &c, 0.005).is_none(),
            "gap 0.006 > 0.005"
        );
        let contact = contact_between(ha, hb, &a, &b, 0.005).unwrap();
        assert_eq!(contact.a, ha);
        assert_eq!(contact.b, hb);
        assert!(!contact.is_sensor);
        assert!((contact.normal - Vec3::Y).length() < EPS);
        assert!(
            (contact.penetration - 0.0).abs() < EPS,
            "not penetrating yet"
        );
    }

    #[test]
    fn segment_aabb_closest_point_is_exact_for_parallel_segment() {
        // Segment along Z at x = 3, y = 0: closest box point is x = 1.
        let (seg, b) = closest_point_segment_aabb(
            Vec3::new(3.0, 0.0, -5.0),
            Vec3::new(3.0, 0.0, 5.0),
            Vec3::splat(1.0),
        );
        assert!((b.x - 1.0).abs() < EPS, "{b:?}");
        assert!((seg.x - 3.0).abs() < EPS);
        assert!(((b - seg).length() - 2.0).abs() < EPS);
    }

    #[test]
    fn segment_aabb_closest_point_handles_inside_segment() {
        let (seg, b) = closest_point_segment_aabb(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.2, 0.0),
            Vec3::splat(1.0),
        );
        assert!((b - seg).length() < EPS, "fully inside: zero distance");
    }

    #[test]
    fn segment_aabb_closest_point_handles_diagonal() {
        // Segment from (5,5,0) down to (5,0,0) against a unit box. The nearest
        // box feature is the edge x = 1, z = 0 for any y in [-1, 1], so the
        // distance is 5 - 1 = 4.
        let (seg, b) = closest_point_segment_aabb(
            Vec3::new(5.0, 5.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::splat(1.0),
        );
        assert!(
            ((b - seg).length() - 4.0).abs() < 1e-3,
            "{}",
            (b - seg).length()
        );
        assert!((b.x - 1.0).abs() < EPS && b.z.abs() < EPS, "{b:?}");
    }

    #[test]
    fn closest_points_segments_handles_crossing_lines() {
        let (a, b) = closest_points_segments(
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert!((a - Vec3::ZERO).length() < EPS);
        assert!((b - Vec3::ZERO).length() < EPS);
    }

    #[test]
    fn normals_are_unit_for_every_pair() {
        let shapes = [
            boxx(0.5),
            sphere(0.5),
            capsule(0.3, 0.6),
            ColliderShape::Cylinder {
                radius: 0.4,
                half_height: 0.7,
            },
        ];
        for (i, a) in shapes.iter().enumerate() {
            for (j, b) in shapes.iter().enumerate() {
                let pa = Vec3::new(i as f32 * 0.3, 0.1, 0.0);
                let pb = Vec3::new(j as f32 * 0.25, 0.2, 0.1);
                let f = closest_features(
                    a,
                    pa,
                    Quat::from_rotation_y(0.3),
                    b,
                    pb,
                    Quat::from_rotation_y(-0.7),
                );
                assert!(
                    (f.normal.length() - 1.0).abs() < 1e-3,
                    "{a:?} vs {b:?}: {:?}",
                    f.normal
                );
                assert!(f.separation.is_finite(), "{a:?} vs {b:?}");
                assert!(f.point_a.is_finite() && f.point_b.is_finite());
            }
        }
    }
}
