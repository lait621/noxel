//! Geometric primitives: bounds, rays, planes, frusta and transforms.
//!
//! The physics, visibility and ray-tracing crates all speak this vocabulary, so
//! an intersection routine written once here is reused verbatim everywhere.

use super::mat::Mat4;
use super::quat::Quat;
use super::scalar::{EPSILON, RAY_EPSILON};
use super::vec::{Vec2, Vec3};

/// An axis-aligned bounding box.
///
/// The workhorse of Noxel's acceleration structures: every mesh, collider,
/// chunk and occluder is reduced to one of these, and all three spatial
/// structures in [`crate::spatial`] are built on them.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Aabb {
    /// Minimum corner (component-wise).
    pub min: Vec3,
    /// Maximum corner (component-wise).
    pub max: Vec3,
}

impl Aabb {
    /// An empty box that [`Aabb::grow`] can expand. The min corner starts at
    /// `+INF` and the max at `-INF`, so the first grown point defines the box.
    pub const EMPTY: Self = Self { min: Vec3::INFINITY, max: Vec3::NEG_INFINITY };

    /// Constructs a box from explicit corners, ordering them if necessary.
    #[inline]
    #[must_use]
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self { min: a.min(b), max: a.max(b) }
    }

    /// Constructs a box from a centre and half-extents.
    #[inline]
    #[must_use]
    pub fn from_center_half_extents(center: Vec3, half: Vec3) -> Self {
        let half = half.abs();
        Self { min: center - half, max: center + half }
    }

    /// Constructs a cube from a centre and an edge length.
    #[inline]
    #[must_use]
    pub fn from_center_size(center: Vec3, size: f32) -> Self {
        let h = size * 0.5;
        Self { min: center - Vec3::splat(h), max: center + Vec3::splat(h) }
    }

    /// Wraps a slice of points (returns [`Aabb::EMPTY`] when empty).
    #[inline]
    #[must_use]
    pub fn from_points(points: &[Vec3]) -> Self {
        let mut b = Self::EMPTY;
        for &p in points {
            b.grow(p);
        }
        b
    }

    /// Builds a box covering an XZ footprint extruded vertically.
    ///
    /// The standard way to derive a collider or occluder from a top-down
    /// footprint: `x0/z0` to `x1/z1` on the ground, `height` tall from `base_y`.
    #[inline]
    #[must_use]
    pub fn from_footprint(x0: f32, z0: f32, x1: f32, z1: f32, base_y: f32, height: f32) -> Self {
        Self::new(
            Vec3::new(x0.min(x1), base_y, z0.min(z1)),
            Vec3::new(x0.max(x1), base_y + height, z0.max(z1)),
        )
    }

    /// Grows the box to include `p`.
    #[inline]
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    /// Grows the box to include `other`.
    #[inline]
    pub fn grow_aabb(&mut self, other: &Aabb) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// The smallest box containing both inputs.
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Aabb) -> Self {
        Self { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    /// The overlap of both boxes, or `None` when they are disjoint.
    #[inline]
    #[must_use]
    pub fn intersection(&self, other: &Aabb) -> Option<Self> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        if min.x <= max.x && min.y <= max.y && min.z <= max.z { Some(Self { min, max }) } else { None }
    }

    /// Expands every side by `amount` (which may be negative to shrink).
    #[inline]
    #[must_use]
    pub fn expanded(&self, amount: f32) -> Self {
        let a = Vec3::splat(amount);
        Self { min: self.min - a, max: self.max + a }
    }

    /// Expands every side in place.
    #[inline]
    pub fn expand(&mut self, amount: f32) {
        *self = self.expanded(amount);
    }

    /// Translates the box.
    #[inline]
    #[must_use]
    pub fn translated(&self, t: Vec3) -> Self {
        Self { min: self.min + t, max: self.max + t }
    }

    /// A box that also covers `y = y` for every XZ point of `self`.
    #[inline]
    #[must_use]
    pub fn with_y_range(&self, y_min: f32, y_max: f32) -> Self {
        Self { min: Vec3::new(self.min.x, y_min, self.min.z), max: Vec3::new(self.max.x, y_max, self.max.z) }
    }

    /// Centre point.
    #[inline]
    #[must_use]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Half-extents (half the size).
    #[inline]
    #[must_use]
    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Full size.
    #[inline]
    #[must_use]
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// Volume; zero or negative for an empty/inverted box.
    #[inline]
    #[must_use]
    pub fn volume(&self) -> f32 {
        let s = self.size();
        if s.x <= 0.0 || s.y <= 0.0 || s.z <= 0.0 { 0.0 } else { s.x * s.y * s.z }
    }

    /// Surface area, the standard cost estimate for a BVH node.
    #[inline]
    #[must_use]
    pub fn surface_area(&self) -> f32 {
        let s = self.size().max(Vec3::ZERO);
        2.0 * (s.x * s.y + s.y * s.z + s.z * s.x)
    }

    /// Radius of the bounding sphere centred on the box centre.
    #[inline]
    #[must_use]
    pub fn bounding_sphere_radius(&self) -> f32 {
        self.half_extents().length()
    }

    /// Index of the longest axis (0 = X, 1 = Y, 2 = Z). Used to choose the BVH
    /// split plane and the broadphase cell layout.
    #[inline]
    #[must_use]
    pub fn longest_axis(&self) -> usize {
        let s = self.size();
        if s.x >= s.y && s.x >= s.z { 0 } else if s.y >= s.z { 1 } else { 2 }
    }

    /// True when the box has no extent, or was never grown.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// True when every corner is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }

    /// True when `p` lies inside or on the box.
    #[inline]
    #[must_use]
    pub fn contains_point(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    /// True when `other` lies entirely inside `self`.
    #[inline]
    #[must_use]
    pub fn contains_aabb(&self, other: &Aabb) -> bool {
        self.contains_point(other.min) && self.contains_point(other.max)
    }

    /// Overlap test.
    #[inline]
    #[must_use]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// Overlap test that also reports the overlap depth per axis.
    #[inline]
    #[must_use]
    pub fn penetration(&self, other: &Aabb) -> Option<Vec3> {
        if !self.intersects(other) {
            return None;
        }
        let overlap = (self.max.min(other.max) - self.min.max(other.min)).max(Vec3::ZERO);
        Some(overlap)
    }

    /// Sphere/box overlap.
    #[inline]
    #[must_use]
    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.distance_squared_to_point(center) <= radius * radius
    }

    /// The point on or inside the box closest to `p`.
    #[inline]
    #[must_use]
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        p.clamp(self.min, self.max)
    }

    /// Squared distance from `p` to the box (zero when inside).
    #[inline]
    #[must_use]
    pub fn distance_squared_to_point(&self, p: Vec3) -> f32 {
        let d = p - self.closest_point(p);
        d.length_squared()
    }

    /// Approximate distance from `p` to the box.
    #[inline]
    #[must_use]
    pub fn distance_to_point(&self, p: Vec3) -> f32 {
        self.distance_squared_to_point(p).sqrt()
    }

    /// The eight corners, in the order `(min,min,min) .. (max,max,max)` with
    /// `x` varying fastest.
    #[inline]
    #[must_use]
    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }

    /// The bounds of this box after transforming every corner by `m`.
    ///
    /// This is the exact (not conservative) result; the renderer and occlusion
    /// system both rely on not over-estimating, because an over-large occluder
    /// would incorrectly hide geometry behind it.
    #[inline]
    #[must_use]
    pub fn transform(&self, m: &Mat4) -> Self {
        let mut out = Self::EMPTY;
        for c in self.corners() {
            out.grow(m.transform_point3(c));
        }
        out
    }

    /// Slab-method ray intersection.
    ///
    /// Returns `(t_enter, t_exit)`; `t_enter` is negative when the ray starts
    /// inside the box. `None` when the ray misses or the box is degenerate.
    #[inline]
    #[must_use]
    pub fn intersect_ray(&self, origin: Vec3, dir: Vec3) -> Option<(f32, f32)> {
        let inv = Vec3::new(
            safe_inv(dir.x),
            safe_inv(dir.y),
            safe_inv(dir.z),
        );
        let t0 = (self.min - origin) * inv;
        let t1 = (self.max - origin) * inv;
        let tmin = t0.min(t1);
        let tmax = t0.max(t1);
        let t_enter = tmin.x.max(tmin.y).max(tmin.z).max(0.0);
        let t_exit = tmax.x.min(tmax.y).min(tmax.z);
        if t_enter <= t_exit { Some((t_enter, t_exit)) } else { None }
    }

    /// Convenience ray test that ignores the hit distances.
    #[inline]
    #[must_use]
    pub fn intersects_ray(&self, origin: Vec3, dir: Vec3, max_t: f32) -> bool {
        match self.intersect_ray(origin, dir) {
            Some((t_enter, t_exit)) => t_enter <= max_t && t_exit >= 0.0,
            None => false,
        }
    }

    /// Conservative frustum test: `true` when the box is at least partly inside.
    ///
    /// A `false` result is always correct (the box is definitely outside), which
    /// is what makes it safe to use for culling. Objects exactly on a plane may
    /// be kept.
    #[inline]
    #[must_use]
    pub fn intersects_frustum(&self, frustum: &Frustum) -> bool {
        frustum.intersects_aabb(self)
    }

    /// A unit cube (`[-0.5, 0.5]³`), the canonical mesh bounds.
    #[inline]
    #[must_use]
    pub fn unit_cube() -> Self {
        Self::new(Vec3::splat(-0.5), Vec3::splat(0.5))
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Reciprocal that maps `0.0` to a very large finite number instead of `inf`,
/// so `0.0 * inf = NaN` never poisons the slab test.
#[inline]
fn safe_inv(v: f32) -> f32 {
    if v.abs() < 1e-20 { 1e20_f32.copysign(v) } else { 1.0 / v }
}

/// A sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Sphere {
    /// Centre point.
    pub center: Vec3,
    /// Radius (must be non-negative for the tests to be meaningful).
    pub radius: f32,
}

impl Sphere {
    /// Constructs a sphere.
    #[inline]
    #[must_use]
    pub const fn new(center: Vec3, radius: f32) -> Self {
        Self { center, radius }
    }

    /// The smallest sphere containing `b`.
    #[inline]
    #[must_use]
    pub fn from_aabb(b: &Aabb) -> Self {
        Self::new(b.center(), b.bounding_sphere_radius())
    }

    /// True when `p` is inside.
    #[inline]
    #[must_use]
    pub fn contains_point(&self, p: Vec3) -> bool {
        self.center.distance_squared(p) <= self.radius * self.radius
    }

    /// Sphere/sphere overlap.
    #[inline]
    #[must_use]
    pub fn intersects_sphere(&self, other: &Sphere) -> bool {
        let r = self.radius + other.radius;
        self.center.distance_squared(other.center) <= r * r
    }

    /// Sphere/box overlap.
    #[inline]
    #[must_use]
    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        b.intersects_sphere(self.center, self.radius)
    }

    /// The nearest and farthest intersection distances along a ray, when hit.
    #[inline]
    #[must_use]
    pub fn intersect_ray(&self, origin: Vec3, dir: Vec3) -> Option<(f32, f32)> {
        let oc = origin - self.center;
        let b = oc.dot(dir);
        let c = oc.dot(oc) - self.radius * self.radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let sq = disc.sqrt();
        let t0 = -b - sq;
        let t1 = -b + sq;
        if t1 < 0.0 { None } else { Some((t0, t1)) }
    }

    /// Volume.
    #[inline]
    #[must_use]
    pub fn volume(&self) -> f32 {
        (4.0 / 3.0) * core::f32::consts::PI * self.radius.powi(3)
    }
}

/// A ray with a maximum travel distance.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Ray {
    /// Start point.
    pub origin: Vec3,
    /// Direction. Callers should normalise it; every routine here assumes unit
    /// length so that `t` is a distance.
    pub dir: Vec3,
    /// Maximum distance to consider.
    pub max_t: f32,
}

impl Ray {
    /// An infinite ray starting at the origin.
    pub const INFINITE: Self =
        Self { origin: Vec3::ZERO, dir: Vec3::new(0.0, 0.0, -1.0), max_t: f32::INFINITY };

    /// Constructs a ray of unlimited length, normalising `dir`.
    #[inline]
    #[must_use]
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self { origin, dir: dir.normalize_or_zero(), max_t: f32::INFINITY }
    }

    /// Constructs a ray with a distance limit.
    #[inline]
    #[must_use]
    pub fn with_max_t(origin: Vec3, dir: Vec3, max_t: f32) -> Self {
        Self { origin, dir: dir.normalize_or_zero(), max_t }
    }

    /// The point at distance `t`.
    #[inline]
    #[must_use]
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }

    /// The ray offset slightly along its direction, to avoid self-intersection
    /// against the surface it was cast from.
    #[inline]
    #[must_use]
    pub fn offset(&self, amount: f32) -> Self {
        Self { origin: self.origin + self.dir * amount, ..*self }
    }

    /// Box intersection within `max_t`.
    #[inline]
    #[must_use]
    pub fn intersect_aabb(&self, b: &Aabb) -> Option<f32> {
        let (t0, t1) = b.intersect_ray(self.origin, self.dir)?;
        if t0 > self.max_t || t1 < 0.0 { None } else { Some(t0.max(0.0)) }
    }

    /// Sphere intersection within `max_t`.
    #[inline]
    #[must_use]
    pub fn intersect_sphere(&self, s: &Sphere) -> Option<f32> {
        let (t0, t1) = s.intersect_ray(self.origin, self.dir)?;
        if t0 > self.max_t || t1 < 0.0 { None } else { Some(t0.max(0.0)) }
    }

    /// Plane intersection within `max_t`.
    #[inline]
    #[must_use]
    pub fn intersect_plane(&self, p: &Plane) -> Option<f32> {
        let denom = p.normal.dot(self.dir);
        if denom.abs() < RAY_EPSILON {
            return None;
        }
        let t = (p.d - p.normal.dot(self.origin)) / denom;
        if (0.0..=self.max_t).contains(&t) { Some(t) } else { None }
    }

    /// Möller–Trumbore triangle intersection within `max_t`.
    ///
    /// Returns `(t, u, v)` where `u`/`v` are barycentric coordinates; the third
    /// is `1 - u - v`. The ray tracer's inner loop, so it is deliberately
    /// branch-light and allocation-free.
    #[inline]
    #[must_use]
    pub fn intersect_triangle(&self, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, f32, f32)> {
        const EPS: f32 = 1e-8;
        let e1 = b - a;
        let e2 = c - a;
        let pvec = self.dir.cross(e2);
        let det = e1.dot(pvec);
        if det.abs() < EPS {
            return None;
        }
        let inv_det = 1.0 / det;
        let tvec = self.origin - a;
        let u = tvec.dot(pvec) * inv_det;
        if !(-EPS..=1.0 + EPS).contains(&u) {
            return None;
        }
        let qvec = tvec.cross(e1);
        let v = self.dir.dot(qvec) * inv_det;
        if v < -EPS || u + v > 1.0 + EPS {
            return None;
        }
        let t = e2.dot(qvec) * inv_det;
        if t < RAY_EPSILON || t > self.max_t { None } else { Some((t, u, v)) }
    }
}

impl Default for Ray {
    fn default() -> Self {
        Self::INFINITE
    }
}

/// An oriented plane in the form `dot(normal, p) == d`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Plane {
    /// Unit normal.
    pub normal: Vec3,
    /// Signed distance from the origin along `normal`.
    pub d: f32,
}

impl Plane {
    /// Constructs a plane from a normal and an offset.
    #[inline]
    #[must_use]
    pub fn new(normal: Vec3, d: f32) -> Self {
        Self { normal: normal.normalize_or_zero(), d }
    }

    /// Constructs a plane through three points (counter-clockwise winding).
    #[inline]
    #[must_use]
    pub fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Self {
        let n = (b - a).cross(c - a).normalize_or_zero();
        Self { normal: n, d: n.dot(a) }
    }

    /// Constructs a plane with the given normal that passes through `p`.
    #[inline]
    #[must_use]
    pub fn from_point_normal(p: Vec3, normal: Vec3) -> Self {
        let n = normal.normalize_or_zero();
        Self { normal: n, d: n.dot(p) }
    }

    /// Signed distance from `p`; positive on the normal's side.
    #[inline]
    #[must_use]
    pub fn distance_to_point(&self, p: Vec3) -> f32 {
        self.normal.dot(p) - self.d
    }

    /// The closest point on the plane to `p`.
    #[inline]
    #[must_use]
    pub fn project_point(&self, p: Vec3) -> Vec3 {
        p - self.normal * self.distance_to_point(p)
    }

    /// Intersection distance with a ray, if any.
    #[inline]
    #[must_use]
    pub fn intersect_ray(&self, ray: &Ray) -> Option<f32> {
        ray.intersect_plane(self)
    }

    /// The line where two planes meet, or `None` when they are parallel.
    #[inline]
    #[must_use]
    pub fn intersect_plane(&self, other: &Plane) -> Option<(Vec3, Vec3)> {
        let dir = self.normal.cross(other.normal);
        let len_sq = dir.length_squared();
        if len_sq < EPSILON * EPSILON {
            return None;
        }
        let p = (dir.cross(other.normal) * self.d + self.normal.cross(dir) * other.d) / len_sq;
        Some((p, dir.normalize_or_zero()))
    }
}

/// A view frustum, stored as six inward-facing planes.
///
/// Built by [`Frustum::from_view_projection`] and used for the first (cheapest)
/// stage of visibility: anything fully outside is discarded before it reaches
/// either renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// Order: left, right, bottom, top, near, far.
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Everything; a frustum that contains the whole world.
    pub const INFINITE: Self = Self {
        planes: [
            Plane { normal: Vec3::new(1.0, 0.0, 0.0), d: f32::INFINITY },
            Plane { normal: Vec3::new(-1.0, 0.0, 0.0), d: f32::INFINITY },
            Plane { normal: Vec3::new(0.0, 1.0, 0.0), d: f32::INFINITY },
            Plane { normal: Vec3::new(0.0, -1.0, 0.0), d: f32::INFINITY },
            Plane { normal: Vec3::new(0.0, 0.0, 1.0), d: f32::INFINITY },
            Plane { normal: Vec3::new(0.0, 0.0, -1.0), d: f32::INFINITY },
        ],
    };

    /// Extracts the six planes of a **column-major** view-projection matrix
    /// producing depth in `[0, 1]`.
    #[must_use]
    pub fn from_view_projection(vp: &Mat4) -> Self {
        // Row i of a column-major matrix: (m[i][0], m[i][1], m[i][2], m[i][3]).
        let row = |i: usize| {
            super::vec::Vec4::new(vp.get(i, 0), vp.get(i, 1), vp.get(i, 2), vp.get(i, 3))
        };
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let mk = |v: super::vec::Vec4| {
            let n = Vec3::new(v.x, v.y, v.z);
            let len = n.length();
            // Clip-space inside-test is `n·p + w >= 0`, i.e. `n·p >= -w`.
            // `Plane::distance_to_point` is `normal·p - d`, so `d = -w / |n|`.
            if len < 1e-12 {
                Plane { normal: Vec3::ZERO, d: f32::NEG_INFINITY }
            } else {
                Plane { normal: n / len, d: -v.w / len }
            }
        };
        Self {
            planes: [
                mk(r3 + r0), // left:   clip.x >= -w
                mk(r3 - r0), // right:  clip.x <=  w
                mk(r3 + r1), // bottom: clip.y >= -w
                mk(r3 - r1), // top:    clip.y <=  w
                mk(r2),      // near:   clip.z >=  0  ([0,1] depth range)
                mk(r3 - r2), // far:    clip.z <=  w
            ],
        }
    }

    /// True when `p` is inside or on the frustum.
    #[inline]
    #[must_use]
    pub fn contains_point(&self, p: Vec3) -> bool {
        self.planes.iter().all(|pl| pl.distance_to_point(p) >= -RAY_EPSILON)
    }

    /// Conservative AABB test.
    ///
    /// Uses the "positive vertex" trick: for each plane only the box corner
    /// furthest along the plane normal needs testing. Returns `true` when the
    /// box may be visible, `false` only when it is definitely outside, which is
    /// exactly the guarantee a culler needs.
    #[inline]
    #[must_use]
    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        for pl in &self.planes {
            let positive = Vec3::new(
                if pl.normal.x >= 0.0 { b.max.x } else { b.min.x },
                if pl.normal.y >= 0.0 { b.max.y } else { b.min.y },
                if pl.normal.z >= 0.0 { b.max.z } else { b.min.z },
            );
            if pl.distance_to_point(positive) < 0.0 {
                return false;
            }
        }
        true
    }

    /// Conservative sphere test.
    #[inline]
    #[must_use]
    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes.iter().all(|pl| pl.distance_to_point(center) >= -radius)
    }

    /// True when `b` is entirely inside (used to promote a node to "always
    /// visible" in the octree/BVH walk).
    #[inline]
    #[must_use]
    pub fn contains_aabb(&self, b: &Aabb) -> bool {
        b.corners().iter().all(|&c| self.contains_point(c))
    }
}

impl Default for Frustum {
    fn default() -> Self {
        Self::INFINITE
    }
}

/// A 2D axis-aligned rectangle. Screen-space regions, UV rects and tile spans.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Rect {
    /// Top-left corner.
    pub min: Vec2,
    /// Bottom-right corner.
    pub max: Vec2,
}

impl Rect {
    /// An empty rectangle.
    pub const EMPTY: Self = Self { min: Vec2::new(f32::INFINITY, f32::INFINITY), max: Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY) };

    /// Constructs a rectangle from two corners, ordering them.
    #[inline]
    #[must_use]
    pub fn new(a: Vec2, b: Vec2) -> Self {
        Self { min: a.min(b), max: a.max(b) }
    }

    /// Constructs a rectangle from a position and a size.
    #[inline]
    #[must_use]
    pub fn from_pos_size(pos: Vec2, size: Vec2) -> Self {
        Self { min: pos, max: pos + size }
    }

    /// Constructs a rectangle from explicit edges.
    #[inline]
    #[must_use]
    pub const fn from_min_max(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// Width.
    #[inline]
    #[must_use]
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// Height.
    #[inline]
    #[must_use]
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Size vector.
    #[inline]
    #[must_use]
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// Centre point.
    #[inline]
    #[must_use]
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// True when the rectangle has no area.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.max.x <= self.min.x || self.max.y <= self.min.y
    }

    /// Area.
    #[inline]
    #[must_use]
    pub fn area(&self) -> f32 {
        if self.is_empty() { 0.0 } else { self.width() * self.height() }
    }

    /// True when `p` is inside.
    #[inline]
    #[must_use]
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Overlap test.
    #[inline]
    #[must_use]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min.x <= other.max.x && self.max.x >= other.min.x && self.min.y <= other.max.y && self.max.y >= other.min.y
    }

    /// Overlap region, or `None`.
    #[inline]
    #[must_use]
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        if min.x <= max.x && min.y <= max.y { Some(Rect { min, max }) } else { None }
    }

    /// Smallest rectangle containing both.
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Rect) -> Rect {
        Rect { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    /// Grows the rectangle to include `p`.
    #[inline]
    pub fn grow(&mut self, p: Vec2) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    /// Expands by `amount` on every side.
    #[inline]
    #[must_use]
    pub fn expanded(&self, amount: Vec2) -> Rect {
        Rect { min: self.min - amount, max: self.max + amount }
    }

    /// The integer pixel rectangle fully containing this one.
    ///
    /// The rasterizer uses this to clip a triangle to a tile: `ceil` on the max
    /// edge is what guarantees no partially-covered pixel is skipped.
    #[inline]
    #[must_use]
    pub fn to_pixel_bounds(&self) -> (i32, i32, i32, i32) {
        (self.min.x.floor() as i32, self.min.y.floor() as i32, self.max.x.ceil() as i32, self.max.y.ceil() as i32)
    }

    /// Normalised UV coordinates of `p` inside this rectangle.
    #[inline]
    #[must_use]
    pub fn uv(&self, p: Vec2) -> Vec2 {
        let s = self.size();
        Vec2::new(
            if s.x.abs() < EPSILON { 0.0 } else { (p.x - self.min.x) / s.x },
            if s.y.abs() < EPSILON { 0.0 } else { (p.y - self.min.y) / s.y },
        )
    }
}

/// A position, rotation and scale triple: the transform component of an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Transform {
    /// World or parent-relative position, in metres.
    pub translation: Vec3,
    /// Rotation.
    pub rotation: Quat,
    /// Non-uniform scale. Keep it uniform unless you have a reason: physics and
    /// the normal matrix both get more expensive otherwise.
    pub scale: Vec3,
}

impl Transform {
    /// Position at the origin, no rotation, unit scale.
    pub const IDENTITY: Self = Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    /// Constructs a transform.
    #[inline]
    #[must_use]
    pub const fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self { translation, rotation, scale }
    }

    /// Translation only.
    #[inline]
    #[must_use]
    pub const fn from_translation(t: Vec3) -> Self {
        Self { translation: t, ..Self::IDENTITY }
    }

    /// Rotation only.
    #[inline]
    #[must_use]
    pub const fn from_rotation(r: Quat) -> Self {
        Self { rotation: r, ..Self::IDENTITY }
    }

    /// Uniform scale only.
    #[inline]
    #[must_use]
    pub const fn from_scale(s: f32) -> Self {
        Self { scale: Vec3::splat(s), ..Self::IDENTITY }
    }

    /// Position and yaw, the common case for top-down entities.
    #[inline]
    #[must_use]
    pub fn from_yaw(t: Vec3, yaw: f32) -> Self {
        Self { translation: t, rotation: Quat::from_rotation_y(yaw), scale: Vec3::ONE }
    }

    /// The equivalent 4×4 matrix.
    #[inline]
    #[must_use]
    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// The inverse transform, or `None` when the scale has a zero component.
    #[inline]
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let inv_scale = Vec3::new(1.0 / self.scale.x, 1.0 / self.scale.y, 1.0 / self.scale.z);
        if !inv_scale.is_finite() {
            return None;
        }
        let inv_rot = self.rotation.inverse()?;
        let inv_t = inv_rot.rotate_vec3(-self.translation * inv_scale);
        Some(Self { translation: inv_t, rotation: inv_rot, scale: inv_scale })
    }

    /// Transforms a point through this transform.
    #[inline]
    #[must_use]
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        self.rotation.rotate_vec3(p * self.scale) + self.translation
    }

    /// Transforms a direction through this transform (ignores translation).
    #[inline]
    #[must_use]
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        self.rotation.rotate_vec3(v * self.scale)
    }

    /// Composes two transforms: `self` applied after `child`.
    #[inline]
    #[must_use]
    pub fn mul_transform(&self, child: &Transform) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: self.rotation * child.rotation,
            scale: self.scale * child.scale,
        }
    }

    /// The world-space forward direction (`-Z` rotated by this transform).
    #[inline]
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::new(0.0, 0.0, -1.0))
    }

    /// The world-space right direction (`+X` rotated by this transform).
    #[inline]
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::new(1.0, 0.0, 0.0))
    }

    /// The world-space up direction (`+Y` rotated by this transform).
    #[inline]
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::new(0.0, 1.0, 0.0))
    }

    /// Renormalises the rotation, for transforms accumulated over many frames.
    #[inline]
    pub fn orthonormalize(&mut self) {
        self.rotation = self.rotation.normalize();
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_empty_then_grow() {
        let mut b = Aabb::EMPTY;
        assert!(b.is_empty());
        b.grow(Vec3::new(1.0, 2.0, 3.0));
        assert!(!b.is_empty());
        assert_eq!(b.min, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(b.max, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn aabb_intersection_matches_point_test() {
        let a = Aabb::new(Vec3::ZERO, Vec3::ONE);
        let b = Aabb::new(Vec3::splat(0.5), Vec3::splat(1.5));
        let c = Aabb::new(Vec3::splat(2.0), Vec3::splat(3.0));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        assert_eq!(a.intersection(&b).unwrap().min, Vec3::splat(0.5));
        assert!(a.intersection(&c).is_none());
    }

    #[test]
    fn aabb_ray_hit_and_miss() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let hit = b.intersect_ray(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        assert!(hit.is_some());
        let (t0, _) = hit.unwrap();
        assert!((t0 - 4.0).abs() < 1e-4);
        let miss = b.intersect_ray(Vec3::new(5.0, 5.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        assert!(miss.is_none());
    }

    #[test]
    fn aabb_axis_aligned_ray_does_not_produce_nan() {
        // dir has zero components -> naive 1/0 would yield inf*0 = NaN.
        let b = Aabb::new(Vec3::ZERO, Vec3::ONE);
        let r = b.intersect_ray(Vec3::new(0.5, 0.5, -5.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(r.is_some(), "axis-aligned ray must hit");
    }

    #[test]
    fn aabb_transform_is_exact_for_rotation() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let m = Mat4::from_rotation_y(core::f32::consts::FRAC_PI_4);
        let t = b.transform(&m);
        // A unit cube rotated 45 degrees has half-extent sqrt(2) on X and Z.
        assert!((t.half_extents().x - core::f32::consts::SQRT_2).abs() < 1e-4);
        assert!((t.half_extents().y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn aabb_footprint() {
        let b = Aabb::from_footprint(0.0, 0.0, 4.0, 3.0, 0.0, 2.0);
        assert_eq!(b.size(), Vec3::new(4.0, 2.0, 3.0));
        assert!(b.contains_point(Vec3::new(2.0, 1.0, 1.5)));
    }

    #[test]
    fn sphere_ray() {
        let s = Sphere::new(Vec3::ZERO, 2.0);
        assert!(s.intersect_ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X).is_some());
        assert!(s.intersect_ray(Vec3::new(-5.0, 3.0, 0.0), Vec3::X).is_none());
    }

    #[test]
    fn ray_triangle_hit() {
        let ray = Ray::new(Vec3::new(0.25, 0.25, 1.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = ray.intersect_triangle(Vec3::ZERO, Vec3::X, Vec3::Y);
        assert!(hit.is_some());
        let (t, u, v) = hit.unwrap();
        assert!((t - 1.0).abs() < 1e-5);
        assert!((u - 0.25).abs() < 1e-5 && (v - 0.25).abs() < 1e-5);
    }

    #[test]
    fn ray_triangle_backface_miss_above_plane() {
        let ray = Ray::new(Vec3::new(2.0, 2.0, 1.0), Vec3::new(0.0, 0.0, -1.0));
        assert!(ray.intersect_triangle(Vec3::ZERO, Vec3::X, Vec3::Y).is_none());
    }

    #[test]
    fn frustum_keeps_inside_and_rejects_outside() {
        let proj = Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 1.0, 100.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 50.0, 0.0), Vec3::ZERO, Vec3::Z);
        let f = Frustum::from_view_projection(&(proj * view));

        let inside = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!(f.intersects_aabb(&inside), "origin should be visible from above");

        let far_away = Aabb::new(Vec3::splat(500.0), Vec3::splat(501.0));
        assert!(!f.intersects_aabb(&far_away), "far box should be culled");

        let behind_camera = Aabb::new(Vec3::new(-1.0, 200.0, -1.0), Vec3::new(1.0, 201.0, 1.0));
        assert!(!f.intersects_aabb(&behind_camera), "box above the camera should be culled");
    }

    #[test]
    fn frustum_contains_aabb() {
        let proj = Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 1.0, 100.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 50.0, 0.0), Vec3::ZERO, Vec3::Z);
        let f = Frustum::from_view_projection(&(proj * view));
        assert!(f.contains_aabb(&Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0))));
        assert!(!f.contains_aabb(&Aabb::new(Vec3::splat(-50.0), Vec3::splat(50.0))));
    }

    #[test]
    fn rect_pixel_bounds_cover_edges() {
        let r = Rect::new(Vec2::new(1.2, 2.7), Vec2::new(5.1, 9.9));
        assert_eq!(r.to_pixel_bounds(), (1, 2, 6, 10));
    }

    #[test]
    fn rect_uv() {
        let r = Rect::new(Vec2::ZERO, Vec2::new(10.0, 20.0));
        assert_eq!(r.uv(Vec2::new(5.0, 10.0)), Vec2::new(0.5, 0.5));
    }

    #[test]
    fn transform_roundtrip() {
        let t = Transform::new(
            Vec3::new(3.0, -1.0, 2.0),
            Quat::from_rotation_y(0.6),
            Vec3::splat(2.0),
        );
        let p = Vec3::new(1.0, 1.0, 0.0);
        let w = t.transform_point(p);
        let back = t.inverse().unwrap().transform_point(w);
        assert!(back.approx_eq(p, 1e-4), "{back:?}");
    }

    #[test]
    fn transform_composition() {
        let parent = Transform::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let child = Transform::from_translation(Vec3::new(0.0, 0.0, 5.0));
        let world = parent.mul_transform(&child);
        assert!(world.translation.approx_eq(Vec3::new(10.0, 0.0, 5.0), 1e-5));
    }

    #[test]
    fn plane_line_intersection() {
        let a = Plane::new(Vec3::X, 0.0);
        let b = Plane::new(Vec3::Y, 0.0);
        let (p, d) = a.intersect_plane(&b).unwrap();
        assert!(p.z.abs() < 1e-5);
        assert!(d.approx_eq(Vec3::Z, 1e-5) || d.approx_eq(-Vec3::Z, 1e-5));
    }
}
