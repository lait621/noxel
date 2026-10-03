//! Vector types.
//!
//! Hand-written rather than generated: Noxel only needs a small, fixed set of
//! operations and keeping them explicit makes the generated assembly easy to
//! reason about (every method here auto-vectorises).
//!
//! **Coordinate system** (fixed engine-wide, see `docs/adr/0001-coordinate-system.md`):
//! right-handed, `+Y` is up, `-Z` is forward for a camera with zero rotation,
//! `+X` is right. Angles are radians. A "top-down" camera therefore looks along
//! `-Y` and its up vector is `-Z`.

use core::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

use super::scalar::{EPSILON, approx_eq};

/// A 2-component `f32` vector. Used for UVs, screen space and grid coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vec2 {
    /// First component.
    pub x: f32,
    /// Second component.
    pub y: f32,
}

/// A 3-component `f32` vector: the primary world-space vector type.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vec3 {
    /// First component.
    pub x: f32,
    /// Second component.
    pub y: f32,
    /// Third component.
    pub z: f32,
}

/// A 4-component `f32` vector: homogeneous points and colours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vec4 {
    /// First component.
    pub x: f32,
    /// Second component.
    pub y: f32,
    /// Third component.
    pub z: f32,
    /// Fourth component.
    pub w: f32,
}

// ---------------------------------------------------------------------------
// Vec2
// ---------------------------------------------------------------------------

impl Vec2 {
    /// The zero vector.
    pub const ZERO: Self = Self::splat(0.0);
    /// The vector `(1, 1)`.
    pub const ONE: Self = Self::splat(1.0);
    /// The unit `+X` vector.
    pub const X: Self = Self::new(1.0, 0.0);
    /// The unit `+Y` vector.
    pub const Y: Self = Self::new(0.0, 1.0);

    /// Constructs a vector from components.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Constructs a vector with every component set to `v`.
    #[inline]
    #[must_use]
    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v }
    }

    /// Converts to a `Vec3` with `z = 0`.
    #[inline]
    #[must_use]
    pub const fn extend(self, z: f32) -> Vec3 {
        Vec3::new(self.x, self.y, z)
    }

    /// Squared length. Prefer this over [`Vec2::length`] in hot loops.
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// Euclidean length.
    #[inline]
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Euclidean distance to `other`.
    #[inline]
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (self - other).length()
    }

    /// Squared distance to `other`.
    #[inline]
    #[must_use]
    pub fn distance_squared(self, other: Self) -> f32 {
        (self - other).length_squared()
    }

    /// Returns a unit vector, or [`Vec2::ZERO`] when the length is degenerate.
    #[inline]
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq > EPSILON * EPSILON {
            self * (1.0 / len_sq.sqrt())
        } else {
            Self::ZERO
        }
    }

    /// Normalises in place, leaving a zero vector untouched.
    #[inline]
    pub fn normalize_or_zero_in_place(&mut self) {
        *self = self.normalize_or_zero();
    }

    /// Dot product.
    #[inline]
    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y
    }

    /// 2D cross product (the `z` component of the 3D cross product).
    #[inline]
    #[must_use]
    pub fn cross(self, rhs: Self) -> f32 {
        self.x * rhs.y - self.y * rhs.x
    }

    /// Component-wise minimum.
    #[inline]
    #[must_use]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y))
    }

    /// Component-wise maximum.
    #[inline]
    #[must_use]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y))
    }

    /// Clamps each component into `[min, max]`.
    #[inline]
    #[must_use]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self::new(self.x.clamp(min.x, max.x), self.y.clamp(min.y, max.y))
    }

    /// Linear interpolation; `t` is not clamped.
    #[inline]
    #[must_use]
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }

    /// Rotates the vector by `angle` radians counter-clockwise.
    #[inline]
    #[must_use]
    pub fn rotate(self, angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// The perpendicular vector `(-y, x)`.
    #[inline]
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Angle of the vector in radians, in `[-PI, PI]`.
    #[inline]
    #[must_use]
    pub fn angle(self) -> f32 {
        self.y.atan2(self.x)
    }

    /// Constructs a unit vector from an angle in radians.
    #[inline]
    #[must_use]
    pub fn from_angle(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(c, s)
    }

    /// True when both components are finite.
    #[inline]
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Component-wise approximate equality.
    #[inline]
    #[must_use]
    pub fn approx_eq(self, rhs: Self, eps: f32) -> bool {
        approx_eq(self.x, rhs.x, eps) && approx_eq(self.y, rhs.y, eps)
    }

    /// Converts to an array, e.g. for vertex upload.
    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }

    /// Builds from an array.
    #[inline]
    #[must_use]
    pub const fn from_array(a: [f32; 2]) -> Self {
        Self::new(a[0], a[1])
    }
}

impl Index<usize> for Vec2 {
    type Output = f32;
    #[inline]
    fn index(&self, i: usize) -> &f32 {
        match i {
            0 => &self.x,
            1 => &self.y,
            _ => panic!("Vec2 index {i} out of range"),
        }
    }
}

impl IndexMut<usize> for Vec2 {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => panic!("Vec2 index {i} out of range"),
        }
    }
}

// ---------------------------------------------------------------------------
// Vec3
// ---------------------------------------------------------------------------

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::splat(0.0);
    /// The vector `(1, 1, 1)`.
    pub const ONE: Self = Self::splat(1.0);
    /// The unit `+X` vector (right).
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// The unit `+Y` vector (**up**).
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// The unit `+Z` vector (backwards; a camera looking forward looks along `-Z`).
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);
    /// `-Y`: the direction a top-down camera looks in world space.
    pub const DOWN: Self = Self::new(0.0, -1.0, 0.0);
    /// `+Y`: opposite of [`Vec3::DOWN`].
    pub const UP: Self = Self::new(0.0, 1.0, 0.0);
    /// A vector with every component set to [`f32::NEG_INFINITY`]; the identity
    /// element for [`Aabb`](crate::math::Aabb) growth loops.
    pub const NEG_INFINITY: Self = Self::splat(f32::NEG_INFINITY);
    /// A vector with every component set to [`f32::INFINITY`].
    pub const INFINITY: Self = Self::splat(f32::INFINITY);

    /// Constructs a vector from components.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Constructs a vector with every component set to `v`.
    #[inline]
    #[must_use]
    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v, z: v }
    }

    /// Drops `z`, producing a [`Vec2`].
    #[inline]
    #[must_use]
    pub const fn truncate(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    /// Appends `w`, producing a [`Vec4`].
    #[inline]
    #[must_use]
    pub const fn extend(self, w: f32) -> Vec4 {
        Vec4::new(self.x, self.y, self.z, w)
    }

    /// A vector built from an angle on the XZ plane, pointing "forward".
    #[inline]
    #[must_use]
    pub fn from_yaw(yaw: f32) -> Self {
        let (s, c) = yaw.sin_cos();
        Self::new(-s, 0.0, -c)
    }

    /// Squared length.
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    /// Euclidean length.
    #[inline]
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Distance to `other`.
    #[inline]
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (self - other).length()
    }

    /// Squared distance to `other`.
    #[inline]
    #[must_use]
    pub fn distance_squared(self, other: Self) -> f32 {
        (self - other).length_squared()
    }

    /// Horizontal (XZ) length, ignoring `y`. Useful for top-down logic where
    /// height should not affect range checks.
    #[inline]
    #[must_use]
    pub fn length_xz(self) -> f32 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    /// Horizontal (XZ) squared length.
    #[inline]
    #[must_use]
    pub fn length_squared_xz(self) -> f32 {
        self.x * self.x + self.z * self.z
    }

    /// Returns a unit vector, or [`Vec3::ZERO`] when the length is degenerate.
    #[inline]
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq > EPSILON * EPSILON {
            self * (1.0 / len_sq.sqrt())
        } else {
            Self::ZERO
        }
    }

    /// Normalises in place, leaving a zero vector untouched.
    #[inline]
    pub fn normalize_or_zero_in_place(&mut self) {
        *self = self.normalize_or_zero();
    }

    /// Normalises, returning `None` when the vector is degenerate. Use this
    /// when a silently-zero normal would hide a bug.
    #[inline]
    #[must_use]
    pub fn try_normalize(self) -> Option<Self> {
        let len_sq = self.length_squared();
        if len_sq > EPSILON * EPSILON {
            Some(self * (1.0 / len_sq.sqrt()))
        } else {
            None
        }
    }

    /// Dot product.
    #[inline]
    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    /// Cross product (`self × rhs`).
    #[inline]
    #[must_use]
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    /// Component-wise minimum.
    #[inline]
    #[must_use]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    /// Component-wise maximum.
    #[inline]
    #[must_use]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    /// Component-wise absolute value.
    #[inline]
    #[must_use]
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    /// The largest component.
    #[inline]
    #[must_use]
    pub fn max_element(self) -> f32 {
        self.x.max(self.y).max(self.z)
    }

    /// The smallest component.
    #[inline]
    #[must_use]
    pub fn min_element(self) -> f32 {
        self.x.min(self.y).min(self.z)
    }

    /// The component with the largest absolute value.
    #[inline]
    #[must_use]
    pub fn max_abs_element(self) -> f32 {
        self.abs().max_element()
    }

    /// Reflects the vector about a normal.
    #[inline]
    #[must_use]
    pub fn reflect(self, normal: Self) -> Self {
        self - normal * (2.0 * self.dot(normal))
    }

    /// Clamps each component into `[min, max]`.
    #[inline]
    #[must_use]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self::new(
            self.x.clamp(min.x, max.x),
            self.y.clamp(min.y, max.y),
            self.z.clamp(min.z, max.z),
        )
    }

    /// Linear interpolation; `t` is not clamped.
    #[inline]
    #[must_use]
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }

    /// Moves `self` towards `rhs` by at most `max_delta`.
    #[inline]
    #[must_use]
    pub fn move_towards(self, rhs: Self, max_delta: f32) -> Self {
        let d = rhs - self;
        let len = d.length();
        if len <= max_delta || len <= EPSILON {
            rhs
        } else {
            self + d * (max_delta / len)
        }
    }

    /// Spherical linear interpolation between two directions.
    #[inline]
    #[must_use]
    pub fn slerp(self, rhs: Self, t: f32) -> Self {
        let a = self.normalize_or_zero();
        let b = rhs.normalize_or_zero();
        let dot = a.dot(b).clamp(-1.0, 1.0);
        let theta = dot.acos();
        if theta.abs() < 1e-4 {
            return a.lerp(b, t).normalize_or_zero();
        }
        let sin_theta = theta.sin();
        let w0 = ((1.0 - t) * theta).sin() / sin_theta;
        let w1 = (t * theta).sin() / sin_theta;
        a * w0 + b * w1
    }

    /// True when every component is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Component-wise approximate equality.
    #[inline]
    #[must_use]
    pub fn approx_eq(self, rhs: Self, eps: f32) -> bool {
        approx_eq(self.x, rhs.x, eps)
            && approx_eq(self.y, rhs.y, eps)
            && approx_eq(self.z, rhs.z, eps)
    }

    /// Converts to an array, e.g. for vertex upload.
    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// Builds from an array.
    #[inline]
    #[must_use]
    pub const fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }
}

impl Index<usize> for Vec3 {
    type Output = f32;
    #[inline]
    fn index(&self, i: usize) -> &f32 {
        match i {
            0 => &self.x,
            1 => &self.y,
            2 => &self.z,
            _ => panic!("Vec3 index {i} out of range"),
        }
    }
}

impl IndexMut<usize> for Vec3 {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            _ => panic!("Vec3 index {i} out of range"),
        }
    }
}

// ---------------------------------------------------------------------------
// Vec4
// ---------------------------------------------------------------------------

impl Vec4 {
    /// The zero vector.
    pub const ZERO: Self = Self::splat(0.0);
    /// The vector `(1, 1, 1, 1)`.
    pub const ONE: Self = Self::splat(1.0);

    /// Constructs a vector from components.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Constructs a vector with every component set to `v`.
    #[inline]
    #[must_use]
    pub const fn splat(v: f32) -> Self {
        Self {
            x: v,
            y: v,
            z: v,
            w: v,
        }
    }

    /// Drops `w`, producing a [`Vec3`].
    #[inline]
    #[must_use]
    pub const fn truncate(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Dot product.
    #[inline]
    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    /// Squared length.
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Euclidean length.
    #[inline]
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns a unit vector, or [`Vec4::ZERO`] when degenerate.
    #[inline]
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq > EPSILON * EPSILON {
            self * (1.0 / len_sq.sqrt())
        } else {
            Self::ZERO
        }
    }

    /// True when every component is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }

    /// Performs the perspective divide, returning a [`Vec3`].
    ///
    /// Returns `None` when `w` is too close to zero (the point is at or behind
    /// the projection plane).
    #[inline]
    #[must_use]
    pub fn perspective_divide(self) -> Option<Vec3> {
        if self.w.abs() <= 1e-7 {
            None
        } else {
            Some(Vec3::new(self.x / self.w, self.y / self.w, self.z / self.w))
        }
    }

    /// Builds from an RGB colour and an alpha value.
    #[inline]
    #[must_use]
    pub const fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::new(r, g, b, a)
    }

    /// Converts to an array.
    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }

    /// Builds from an array.
    #[inline]
    #[must_use]
    pub const fn from_array(a: [f32; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3])
    }
}

// ---------------------------------------------------------------------------
// Operator implementations
// ---------------------------------------------------------------------------

macro_rules! impl_vec_binary {
    ($t:ty) => {
        impl Add for $t {
            type Output = Self;
            #[inline]
            fn add(self, rhs: Self) -> Self {
                Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
            }
        }
        impl Sub for $t {
            type Output = Self;
            #[inline]
            fn sub(self, rhs: Self) -> Self {
                Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
            }
        }
        impl Mul for $t {
            type Output = Self;
            #[inline]
            fn mul(self, rhs: Self) -> Self {
                Self::new(self.x * rhs.x, self.y * rhs.y, self.z * rhs.z)
            }
        }
        impl Mul<f32> for $t {
            type Output = Self;
            #[inline]
            fn mul(self, rhs: f32) -> Self {
                Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
            }
        }
        impl Mul<$t> for f32 {
            type Output = $t;
            #[inline]
            fn mul(self, rhs: $t) -> $t {
                rhs * self
            }
        }
        impl Div<f32> for $t {
            type Output = Self;
            #[inline]
            fn div(self, rhs: f32) -> Self {
                Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
            }
        }
        impl Div for $t {
            type Output = Self;
            #[inline]
            fn div(self, rhs: Self) -> Self {
                Self::new(self.x / rhs.x, self.y / rhs.y, self.z / rhs.z)
            }
        }
        impl Neg for $t {
            type Output = Self;
            #[inline]
            fn neg(self) -> Self {
                Self::new(-self.x, -self.y, -self.z)
            }
        }
        impl AddAssign for $t {
            #[inline]
            fn add_assign(&mut self, rhs: Self) {
                *self = *self + rhs;
            }
        }
        impl SubAssign for $t {
            #[inline]
            fn sub_assign(&mut self, rhs: Self) {
                *self = *self - rhs;
            }
        }
        impl MulAssign<f32> for $t {
            #[inline]
            fn mul_assign(&mut self, rhs: f32) {
                *self = *self * rhs;
            }
        }
        impl DivAssign<f32> for $t {
            #[inline]
            fn div_assign(&mut self, rhs: f32) {
                *self = *self / rhs;
            }
        }
        impl MulAssign for $t {
            #[inline]
            fn mul_assign(&mut self, rhs: Self) {
                *self = *self * rhs;
            }
        }
        impl AddAssign<f32> for $t {
            /// Adds a scalar to every component. Convenient for `+= dt` style
            /// uniform motion, and for offsetting transform vectors.
            #[inline]
            fn add_assign(&mut self, rhs: f32) {
                *self = Self::new(self.x + rhs, self.y + rhs, self.z + rhs);
            }
        }
    };
}

impl_vec_binary!(Vec3);

impl Add for Vec4 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(
            self.x + rhs.x,
            self.y + rhs.y,
            self.z + rhs.z,
            self.w + rhs.w,
        )
    }
}

impl Sub for Vec4 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(
            self.x - rhs.x,
            self.y - rhs.y,
            self.z - rhs.z,
            self.w - rhs.w,
        )
    }
}

impl Mul<f32> for Vec4 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs, self.w * rhs)
    }
}

impl Neg for Vec4 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f32 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: Vec2) -> Vec2 {
        rhs * self
    }
}

impl Add for Vec2 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Div<f32> for Vec2 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl MulAssign<f32> for Vec2 {
    #[inline]
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl From<[f32; 2]> for Vec2 {
    #[inline]
    fn from(a: [f32; 2]) -> Self {
        Self::from_array(a)
    }
}

impl From<[f32; 3]> for Vec3 {
    #[inline]
    fn from(a: [f32; 3]) -> Self {
        Self::from_array(a)
    }
}

impl From<[f32; 4]> for Vec4 {
    #[inline]
    fn from(a: [f32; 4]) -> Self {
        Self::from_array(a)
    }
}

impl From<Vec3> for Vec2 {
    /// Truncates, dropping `z`.
    #[inline]
    fn from(v: Vec3) -> Self {
        v.truncate()
    }
}

impl From<Vec2> for Vec3 {
    /// Extends with `z = 0`.
    #[inline]
    fn from(v: Vec2) -> Self {
        v.extend(0.0)
    }
}

impl From<Vec3> for Vec4 {
    /// Extends with `w = 0`.
    #[inline]
    fn from(v: Vec3) -> Self {
        v.extend(0.0)
    }
}

impl From<(f32, f32)> for Vec2 {
    #[inline]
    fn from((x, y): (f32, f32)) -> Self {
        Self::new(x, y)
    }
}

impl From<(f32, f32, f32)> for Vec3 {
    #[inline]
    fn from((x, y, z): (f32, f32, f32)) -> Self {
        Self::new(x, y, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec3_basic_ops() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        assert_eq!(a + b, Vec3::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vec3::new(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(2.0 * a, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(-a, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(a.dot(b), 32.0);
    }

    #[test]
    fn vec3_cross_is_right_handed() {
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::Y.cross(Vec3::Z), Vec3::X);
        assert_eq!(Vec3::Z.cross(Vec3::X), Vec3::Y);
    }

    #[test]
    fn normalize_zero_is_zero() {
        assert_eq!(Vec3::ZERO.normalize_or_zero(), Vec3::ZERO);
        assert!(Vec3::new(0.0, 1e-30, 0.0).normalize_or_zero() == Vec3::ZERO);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).normalize_or_zero().length(), 1.0);
    }

    #[test]
    fn reflect_preserves_length() {
        let v = Vec3::new(1.0, -1.0, 0.0).normalize_or_zero();
        let n = Vec3::Y;
        let r = v.reflect(n);
        assert!((r.length() - 1.0).abs() < 1e-5);
        assert!(r.y > 0.0);
    }

    #[test]
    fn yaw_forward_is_negative_z() {
        let f = Vec3::from_yaw(0.0);
        assert!(f.approx_eq(Vec3::new(0.0, 0.0, -1.0), 1e-6));
    }

    #[test]
    fn vec2_rotation() {
        let r = Vec2::X.rotate(core::f32::consts::FRAC_PI_2);
        assert!(r.approx_eq(Vec2::Y, 1e-6));
    }

    #[test]
    fn perspective_divide_rejects_zero_w() {
        assert!(Vec4::new(1.0, 2.0, 3.0, 0.0).perspective_divide().is_none());
        let p = Vec4::new(2.0, 4.0, 6.0, 2.0).perspective_divide().unwrap();
        assert_eq!(p, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn index_access_matches_fields() {
        let v = Vec3::new(7.0, 8.0, 9.0);
        assert_eq!(v[0], 7.0);
        assert_eq!(v[1], 8.0);
        assert_eq!(v[2], 9.0);
    }
}
