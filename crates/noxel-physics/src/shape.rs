//! Collider shapes and their local-space geometry.
//!
//! A [`ColliderShape`] is defined in the **local space of its body**: the shape
//! is centred on the body's origin and is placed in the world by the body's
//! `position` and `rotation`. All routines here are `#![forbid(unsafe_code)]`
//! safe maths with no allocation.
//!
//! # Cylinders
//!
//! `Cylinder` has exact `aabb`, `support`, `contains_point` and ray tests, but
//! the *narrowphase* treats it as its local bounding box (`r`, `hh`, `r`)
//! because that keeps one consistent contact model for every non-sphere pair.
//! See [`ColliderShape::collision_shape`] and the crate docs.

use noxel_core::math::{Aabb, Quat, Vec3};

/// Directions shorter than this are treated as degenerate.
pub(crate) const DIR_EPSILON: f32 = 1e-8;

/// Distance below which two features are considered coincident.
pub(crate) const CONTACT_EPSILON: f32 = 1e-6;

/// A convex collider shape, expressed in the body's local space.
///
/// ```
/// use noxel_core::math::{Quat, Vec3};
/// use noxel_physics::ColliderShape;
///
/// let b = ColliderShape::Box { half_extents: Vec3::splat(0.5) };
/// // A unit box centred at (0.5, 0.5, 0.5) exactly covers the unit cube.
/// let aabb = b.aabb(Vec3::splat(0.5), Quat::IDENTITY);
/// assert_eq!(aabb.min, Vec3::ZERO);
/// assert_eq!(aabb.max, Vec3::ONE);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColliderShape {
    /// An oriented box.
    Box {
        /// Half extents along the local X, Y and Z axes, in metres.
        half_extents: Vec3,
    },
    /// A sphere.
    Sphere {
        /// Radius in metres.
        radius: f32,
    },
    /// A capsule: the segment `±half_height` on local Y, swept by a sphere.
    Capsule {
        /// Radius of the swept sphere in metres.
        radius: f32,
        /// Half the length of the central segment in metres.
        half_height: f32,
    },
    /// A cylinder centred on the local origin and aligned with local Y.
    Cylinder {
        /// Radius in metres.
        radius: f32,
        /// Half the height in metres.
        half_height: f32,
    },
}

impl ColliderShape {
    /// The world-space axis-aligned bounding box of this shape at `position`
    /// with `rotation`.
    ///
    /// The result always *contains* the shape (it is exact for spheres, boxes,
    /// capsules and cylinders), which is what the broadphase needs.
    #[must_use]
    pub fn aabb(&self, position: Vec3, rotation: Quat) -> Aabb {
        match *self {
            Self::Sphere { radius } => {
                Aabb::from_center_half_extents(position, Vec3::splat(radius.abs()))
            }
            Self::Box { half_extents } => {
                let h = half_extents.abs();
                let m = rotation.to_mat3();
                let e = Vec3::new(
                    m.get(0, 0).abs() * h.x + m.get(0, 1).abs() * h.y + m.get(0, 2).abs() * h.z,
                    m.get(1, 0).abs() * h.x + m.get(1, 1).abs() * h.y + m.get(1, 2).abs() * h.z,
                    m.get(2, 0).abs() * h.x + m.get(2, 1).abs() * h.y + m.get(2, 2).abs() * h.z,
                );
                Aabb::from_center_half_extents(position, e)
            }
            Self::Capsule { radius, half_height } => {
                let axis = rotation.rotate_vec3(Vec3::Y) * half_height.abs();
                Aabb::new(position - axis, position + axis).expanded(radius.abs())
            }
            Self::Cylinder { radius, half_height } => {
                // Exact extent of a cylinder along world axis `e`:
                //   hh * |axis·e| + r * sqrt(1 - (axis·e)^2)
                // because the circular cross-section projects to an ellipse
                // whose support along `e` is `r * sin(angle)`.
                let r = radius.abs();
                let hh = half_height.abs();
                let a = rotation.rotate_vec3(Vec3::Y).normalize_or_zero();
                let extent = |v: f32| hh * v.abs() + r * (1.0 - v * v).max(0.0).sqrt();
                Aabb::from_center_half_extents(
                    position,
                    Vec3::new(extent(a.x), extent(a.y), extent(a.z)),
                )
            }
        }
    }

    /// Radius of the smallest sphere centred on the body origin that contains
    /// the whole shape.
    #[must_use]
    pub fn bounding_radius(&self) -> f32 {
        match *self {
            Self::Box { half_extents } => half_extents.abs().length(),
            Self::Sphere { radius } => radius.abs(),
            Self::Capsule { radius, half_height } => half_height.abs() + radius.abs(),
            Self::Cylinder { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                (r * r + hh * hh).sqrt()
            }
        }
    }

    /// The volume of the shape in cubic metres.
    #[must_use]
    pub fn volume(&self) -> f32 {
        let pi = core::f32::consts::PI;
        match *self {
            Self::Box { half_extents } => {
                let h = half_extents.abs();
                8.0 * h.x * h.y * h.z
            }
            Self::Sphere { radius } => {
                let r = radius.abs();
                4.0 / 3.0 * pi * r * r * r
            }
            Self::Capsule { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                pi * r * r * 2.0 * hh + 4.0 / 3.0 * pi * r * r * r
            }
            Self::Cylinder { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                pi * r * r * 2.0 * hh
            }
        }
    }

    /// True when `local` (a point in the shape's local space) is inside or on
    /// the surface of the shape.
    #[must_use]
    pub fn contains_point(&self, local: Vec3) -> bool {
        match *self {
            Self::Box { half_extents } => {
                let h = half_extents.abs();
                local.x.abs() <= h.x && local.y.abs() <= h.y && local.z.abs() <= h.z
            }
            Self::Sphere { radius } => local.length_squared() <= radius.abs() * radius.abs(),
            Self::Capsule { radius, half_height } => {
                let r = radius.abs();
                let closest = closest_point_on_y_segment(local, half_height.abs());
                (local - closest).length_squared() <= r * r
            }
            Self::Cylinder { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                local.y.abs() <= hh && local.x * local.x + local.z * local.z <= r * r
            }
        }
    }

    /// The point of the shape furthest along `dir`, in local space.
    ///
    /// This is the support function used by GJK-style algorithms and by the
    /// character controller's ground probes. `dir` does not need to be unit
    /// length; a zero direction returns the local origin offset for spheres and
    /// the segment centre for capsules.
    #[must_use]
    pub fn support(&self, dir: Vec3) -> Vec3 {
        match *self {
            Self::Box { half_extents } => {
                let h = half_extents.abs();
                Vec3::new(
                    if dir.x >= 0.0 { h.x } else { -h.x },
                    if dir.y >= 0.0 { h.y } else { -h.y },
                    if dir.z >= 0.0 { h.z } else { -h.z },
                )
            }
            Self::Sphere { radius } => dir.normalize_or_zero() * radius.abs(),
            Self::Capsule { radius, half_height } => {
                let y = if dir.y >= 0.0 { half_height.abs() } else { -half_height.abs() };
                Vec3::new(0.0, y, 0.0) + dir.normalize_or_zero() * radius.abs()
            }
            Self::Cylinder { radius, half_height } => {
                let y = if dir.y >= 0.0 { half_height.abs() } else { -half_height.abs() };
                let radial = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero() * radius.abs();
                Vec3::new(0.0, y, 0.0) + radial
            }
        }
    }

    /// The shape actually used by collision detection.
    ///
    /// Cylinders collide as their local bounding box, so every non-sphere pair
    /// shares one contact model. Sphere-vs-cylinder and every ray query stay
    /// analytic. This is a deliberate, documented approximation: a square
    /// cross-section is a good fit for the crates, barrels and pillars a
    /// top-down RPG actually places.
    #[must_use]
    pub fn collision_shape(&self) -> Self {
        match *self {
            Self::Cylinder { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                Self::Box { half_extents: Vec3::new(r, hh, r) }
            }
            other => other,
        }
    }

    /// Half extents of the shape's local bounding box (centred on the origin).
    #[must_use]
    pub fn local_half_extents(&self) -> Vec3 {
        match *self {
            Self::Box { half_extents } => half_extents.abs(),
            Self::Sphere { radius } => Vec3::splat(radius.abs()),
            Self::Capsule { radius, half_height } => {
                Vec3::new(radius.abs(), half_height.abs() + radius.abs(), radius.abs())
            }
            Self::Cylinder { radius, half_height } => {
                Vec3::new(radius.abs(), half_height.abs(), radius.abs())
            }
        }
    }

    /// Intersects the shape with a ray expressed in the shape's local space.
    ///
    /// `dir` must be unit length. Returns `(t, local_normal)`. A ray whose
    /// origin is already inside the shape reports `t = 0` and the normal
    /// `-dir`, because there is no meaningful surface normal in that case.
    #[must_use]
    pub(crate) fn raycast_local(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<(f32, Vec3)> {
        match *self {
            Self::Sphere { radius } => raycast_sphere(origin, dir, radius.abs(), max_t),
            Self::Box { half_extents } => raycast_box(origin, dir, half_extents.abs(), max_t),
            Self::Capsule { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                // A capsule is the union of a cylinder body and two end spheres,
                // so the entry point is the smallest valid hit of those three.
                let inside = {
                    let closest = closest_point_on_y_segment(origin, hh);
                    (origin - closest).length_squared() <= r * r
                };
                if inside {
                    return Some((0.0, -dir));
                }
                let mut best: Option<(f32, Vec3)> = None;
                if let Some(hit) = raycast_cylinder_side(origin, dir, r, hh, max_t) {
                    best = Some(hit);
                }
                for sign in [1.0f32, -1.0] {
                    let center = Vec3::new(0.0, sign * hh, 0.0);
                    if let Some((t, n)) = raycast_sphere(origin - center, dir, r, max_t)
                        && best.is_none_or(|(bt, _)| t < bt)
                    {
                        best = Some((t, n));
                    }
                }
                best
            }
            Self::Cylinder { radius, half_height } => {
                let (r, hh) = (radius.abs(), half_height.abs());
                let inside = origin.y.abs() <= hh
                    && origin.x * origin.x + origin.z * origin.z <= r * r;
                if inside {
                    return Some((0.0, -dir));
                }
                let mut best = raycast_cylinder_side(origin, dir, r, hh, max_t);
                if dir.y.abs() > DIR_EPSILON {
                    for sign in [1.0f32, -1.0] {
                        let t = (sign * hh - origin.y) / dir.y;
                        if t >= 0.0 && t <= max_t {
                            let p = origin + dir * t;
                            if p.x * p.x + p.z * p.z <= r * r && best.is_none_or(|(bt, _)| t < bt) {
                                best = Some((t, Vec3::new(0.0, sign, 0.0)));
                            }
                        }
                    }
                }
                best
            }
        }
    }

}

/// The point on the segment `(0, -hh, 0)..(0, +hh, 0)` closest to `p`.
#[inline]
#[must_use]
pub(crate) fn closest_point_on_y_segment(p: Vec3, half_height: f32) -> Vec3 {
    Vec3::new(0.0, p.y.clamp(-half_height, half_height), 0.0)
}

/// Ray vs sphere centred on the local origin.
fn raycast_sphere(origin: Vec3, dir: Vec3, radius: f32, max_t: f32) -> Option<(f32, Vec3)> {
    let c = origin.length_squared() - radius * radius;
    if c <= 0.0 {
        // Origin inside (or exactly on) the sphere.
        return Some((0.0, -dir));
    }
    let b = origin.dot(dir);
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    if !(0.0..=max_t).contains(&t) {
        return None;
    }
    let point = origin + dir * t;
    Some((t, point.normalize_or_zero()))
}

/// Ray vs axis-aligned box centred on the local origin (slab method).
fn raycast_box(origin: Vec3, dir: Vec3, half: Vec3, max_t: f32) -> Option<(f32, Vec3)> {
    if origin.x.abs() <= half.x && origin.y.abs() <= half.y && origin.z.abs() <= half.z {
        return Some((0.0, -dir));
    }
    let mut t_min = 0.0f32;
    let mut t_max = max_t;
    let mut axis = 0usize;
    for i in 0..3 {
        let (o, d, h) = (origin[i], dir[i], half[i]);
        if d.abs() <= DIR_EPSILON {
            if o < -h || o > h {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let t1 = (-h - o) * inv;
        let t2 = (h - o) * inv;
        let (near, far) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        if near > t_min {
            t_min = near;
            axis = i;
        }
        if far < t_max {
            t_max = far;
        }
        if t_min > t_max {
            return None;
        }
    }
    if t_min > max_t {
        return None;
    }
    let mut normal = Vec3::ZERO;
    normal[axis] = -dir[axis].signum();
    Some((t_min, normal))
}

/// Ray vs the *lateral* surface of a Y-aligned cylinder (no caps).
fn raycast_cylinder_side(
    origin: Vec3,
    dir: Vec3,
    radius: f32,
    half_height: f32,
    max_t: f32,
) -> Option<(f32, Vec3)> {
    let a = dir.x * dir.x + dir.z * dir.z;
    if a <= DIR_EPSILON {
        return None;
    }
    let b = origin.x * dir.x + origin.z * dir.z;
    let c = origin.x * origin.x + origin.z * origin.z - radius * radius;
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    for t in [(-b - sq) / a, (-b + sq) / a] {
        if !(0.0..=max_t).contains(&t) {
            continue;
        }
        let p = origin + dir * t;
        if p.y.abs() > half_height {
            continue;
        }
        let n = Vec3::new(p.x, 0.0, p.z).normalize_or_zero();
        return Some((t, n));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    #[test]
    fn box_aabb_is_tight_when_axis_aligned() {
        let s = ColliderShape::Box { half_extents: Vec3::new(1.0, 2.0, 3.0) };
        let b = s.aabb(Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY);
        assert!((b.min - Vec3::new(0.0, -2.0, -3.0)).length() < EPS);
        assert!((b.max - Vec3::new(2.0, 2.0, 3.0)).length() < EPS);
    }

    #[test]
    fn box_aabb_grows_with_yaw() {
        // A 1x1x1 box yawed 45 degrees has a half-diagonal footprint
        // sqrt(0.5^2 + 0.5^2) = 0.7071 on X and Z, and 0.5 on Y.
        let s = ColliderShape::Box { half_extents: Vec3::splat(0.5) };
        let b = s.aabb(Vec3::ZERO, Quat::from_rotation_y(core::f32::consts::FRAC_PI_4));
        let expected = (0.5f32 * 0.5 + 0.5 * 0.5).sqrt();
        assert!((b.half_extents().x - expected).abs() < EPS);
        assert!((b.half_extents().z - expected).abs() < EPS);
        assert!((b.half_extents().y - 0.5).abs() < EPS);
    }

    #[test]
    fn capsule_aabb_includes_caps() {
        let s = ColliderShape::Capsule { radius: 0.5, half_height: 1.0 };
        let b = s.aabb(Vec3::ZERO, Quat::IDENTITY);
        assert!((b.half_extents().y - 1.5).abs() < EPS);
        assert!((b.half_extents().x - 0.5).abs() < EPS);
    }

    #[test]
    fn cylinder_aabb_is_exact_on_axis() {
        let s = ColliderShape::Cylinder { radius: 0.5, half_height: 2.0 };
        let b = s.aabb(Vec3::ZERO, Quat::from_rotation_z(core::f32::consts::FRAC_PI_2));
        // Rotated 90 degrees about Z the axis points along -X, so the extent is
        // 2.0 on X, 0.5 on Y and 0.5 on Z.
        assert!((b.half_extents().x - 2.0).abs() < EPS, "{:?}", b.half_extents());
        assert!((b.half_extents().y - 0.5).abs() < EPS, "{:?}", b.half_extents());
        assert!((b.half_extents().z - 0.5).abs() < EPS, "{:?}", b.half_extents());
    }

    #[test]
    fn bounding_radius_contains_shape() {
        let shapes = [
            ColliderShape::Box { half_extents: Vec3::new(1.0, 0.5, 2.0) },
            ColliderShape::Sphere { radius: 1.5 },
            ColliderShape::Capsule { radius: 0.25, half_height: 0.75 },
            ColliderShape::Cylinder { radius: 0.4, half_height: 1.2 },
        ];
        for s in shapes {
            let r = s.bounding_radius();
            for i in 0..16 {
                let a = i as f32 * 0.4;
                let p = s.support(Vec3::new(a.cos(), (a * 0.7).sin(), a.sin()));
                assert!(p.length() <= r + EPS, "{s:?} support {p:?} radius {r}");
            }
        }
    }

    #[test]
    fn volumes_match_closed_forms() {
        let pi = core::f32::consts::PI;
        let cube = ColliderShape::Box { half_extents: Vec3::splat(0.5) };
        assert!((cube.volume() - 1.0).abs() < EPS);
        let ball = ColliderShape::Sphere { radius: 1.0 };
        assert!((ball.volume() - 4.0 / 3.0 * pi).abs() < EPS);
        let cyl = ColliderShape::Cylinder { radius: 1.0, half_height: 1.0 };
        assert!((cyl.volume() - 2.0 * pi).abs() < EPS);
        let cap = ColliderShape::Capsule { radius: 1.0, half_height: 1.0 };
        assert!((cap.volume() - (2.0 * pi + 4.0 / 3.0 * pi)).abs() < EPS);
    }

    #[test]
    fn contains_point_matches_shape_definition() {
        let b = ColliderShape::Box { half_extents: Vec3::splat(1.0) };
        assert!(b.contains_point(Vec3::new(1.0, -1.0, 0.0)));
        assert!(!b.contains_point(Vec3::new(1.01, 0.0, 0.0)));

        let s = ColliderShape::Sphere { radius: 1.0 };
        assert!(s.contains_point(Vec3::new(0.0, 1.0, 0.0)));
        assert!(!s.contains_point(Vec3::new(0.0, 1.001, 0.0)));

        // Capsule: a point 0.4 off the axis is inside (r = 0.5), but a point
        // beyond the cap at y = 1.4 is outside (1.4 > 1.0 + 0.5).
        let c = ColliderShape::Capsule { radius: 0.5, half_height: 1.0 };
        assert!(c.contains_point(Vec3::new(0.4, 0.9, 0.0)));
        // The cap sphere reaches y = 1.0 + 0.5; 1.4 is still inside it.
        assert!(c.contains_point(Vec3::new(0.0, 1.4, 0.0)));
        assert!(!c.contains_point(Vec3::new(0.0, 1.6, 0.0)));

        let cy = ColliderShape::Cylinder { radius: 0.5, half_height: 1.0 };
        assert!(cy.contains_point(Vec3::new(0.4, 0.9, 0.0)));
        assert!(!cy.contains_point(Vec3::new(0.4, 0.9, 0.4)));
    }

    #[test]
    fn support_is_extremal() {
        let dir = Vec3::new(1.0, -0.5, 0.25);
        let shapes = [
            ColliderShape::Box { half_extents: Vec3::new(1.0, 0.5, 2.0) },
            ColliderShape::Sphere { radius: 1.5 },
            ColliderShape::Capsule { radius: 0.25, half_height: 0.75 },
            ColliderShape::Cylinder { radius: 0.4, half_height: 1.2 },
        ];
        for s in shapes {
            let p = s.support(dir);
            // No sampled surface point may project further along `dir`.
            for i in 0..64 {
                let a = i as f32 * 0.31;
                let d = Vec3::new(a.cos(), (a * 1.7).sin(), (a * 0.3).cos());
                let q = s.support(d);
                assert!(
                    q.dot(dir) <= p.dot(dir) + EPS,
                    "{s:?}: {} > {}",
                    q.dot(dir),
                    p.dot(dir)
                );
            }
        }
    }

    #[test]
    fn ray_hits_sphere_at_surface() {
        let s = ColliderShape::Sphere { radius: 1.0 };
        let (t, n) = s.raycast_local(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).unwrap();
        assert!((t - 4.0).abs() < EPS);
        assert!((n - Vec3::X).length() < EPS);
    }

    #[test]
    fn ray_misses_sphere() {
        let s = ColliderShape::Sphere { radius: 1.0 };
        assert!(s.raycast_local(Vec3::new(5.0, 2.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).is_none());
        assert!(s.raycast_local(Vec3::new(5.0, 0.0, 0.0), Vec3::X, 100.0).is_none());
    }

    #[test]
    fn ray_hits_box_face_and_reports_axis_normal() {
        let b = ColliderShape::Box { half_extents: Vec3::splat(1.0) };
        let (t, n) = b.raycast_local(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).unwrap();
        assert!((t - 4.0).abs() < EPS);
        assert!((n - Vec3::X).length() < EPS);
        let (t, n) = b.raycast_local(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 100.0).unwrap();
        assert!((t - 4.0).abs() < EPS);
        assert!((n - Vec3::Y).length() < EPS);
        assert!(b.raycast_local(Vec3::new(5.0, 5.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).is_none());
    }

    #[test]
    fn ray_hits_cylinder_side_and_cap() {
        let c = ColliderShape::Cylinder { radius: 1.0, half_height: 1.0 };
        let (t, n) = c.raycast_local(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).unwrap();
        assert!((t - 4.0).abs() < EPS);
        assert!((n - Vec3::X).length() < EPS);
        let (t, n) = c.raycast_local(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 100.0).unwrap();
        assert!((t - 4.0).abs() < EPS);
        assert!((n - Vec3::Y).length() < EPS);
    }

    #[test]
    fn ray_hits_capsule_side_and_cap() {
        let c = ColliderShape::Capsule { radius: 0.5, half_height: 1.0 };
        let (t, n) = c.raycast_local(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), 100.0).unwrap();
        assert!((t - 4.5).abs() < EPS);
        assert!((n - Vec3::X).length() < EPS);
        // Straight down onto the top cap: the sphere centre sits at y = 1.0.
        let (t, n) = c.raycast_local(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 100.0).unwrap();
        assert!((t - 3.5).abs() < EPS);
        assert!((n - Vec3::Y).length() < EPS);
    }

    #[test]
    fn ray_from_inside_reports_zero() {
        let b = ColliderShape::Box { half_extents: Vec3::splat(1.0) };
        let (t, _) = b.raycast_local(Vec3::ZERO, Vec3::X, 100.0).unwrap();
        assert_eq!(t, 0.0);
    }

    #[test]
    fn collision_shape_maps_cylinder_to_box() {
        let c = ColliderShape::Cylinder { radius: 0.5, half_height: 2.0 };
        assert_eq!(c.collision_shape(), ColliderShape::Box { half_extents: Vec3::new(0.5, 2.0, 0.5) });
        // The box's bounding radius is its half-diagonal: sqrt(0.5^2+2^2+0.5^2).
        let expected = (0.25f32 + 4.0 + 0.25).sqrt();
        assert!((c.collision_shape().bounding_radius() - expected).abs() < 1e-5);
    }

    #[test]
    fn negative_extents_are_treated_as_magnitudes() {
        let b = ColliderShape::Box { half_extents: Vec3::splat(-1.0) };
        let aabb = b.aabb(Vec3::ZERO, Quat::IDENTITY);
        assert!((aabb.max - Vec3::splat(1.0)).length() < EPS);
        assert!(b.contains_point(Vec3::new(0.5, 0.5, 0.5)));
    }
}
