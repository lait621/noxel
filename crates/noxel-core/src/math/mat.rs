//! Matrix types: [`Mat3`] for normals/2D transforms and [`Mat4`] for the
//! transform stack and the camera projection.
//!
//! Both are **column-major**, matching the convention used by wgpu, OpenGL,
//! Vulkan and every sprite/vertex shader a contributor is likely to write.
//! `Mat4::to_cols_array()` can therefore be uploaded verbatim.

use core::ops::Mul;

use super::quat::Quat;
use super::scalar::EPSILON;
use super::vec::{Vec3, Vec4};

/// A 3×3 column-major matrix.
///
/// Used for the inverse-transpose normal matrix, for 2D rotation in sprite and
/// billboard code, and for inertia tensors in the physics crate.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Mat3 {
    /// Columns of the matrix. `cols[c][r]` is row `r`, column `c`.
    pub cols: [Vec3; 3],
}

/// A 4×4 column-major matrix: the engine's model/view/projection type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Mat4 {
    /// Columns of the matrix. `cols[c][r]` is row `r`, column `c`.
    pub cols: [Vec4; 4],
}

impl Default for Mat3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat3 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        cols: [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)],
    };

    /// The zero matrix.
    pub const ZERO: Self = Self { cols: [Vec3::ZERO; 3] };

    /// Builds from three column vectors.
    #[inline]
    #[must_use]
    pub const fn from_cols(x: Vec3, y: Vec3, z: Vec3) -> Self {
        Self { cols: [x, y, z] }
    }

    /// Builds from a flat column-major array (9 elements).
    #[inline]
    #[must_use]
    pub const fn from_cols_array(m: &[f32; 9]) -> Self {
        Self::from_cols(
            Vec3::new(m[0], m[1], m[2]),
            Vec3::new(m[3], m[4], m[5]),
            Vec3::new(m[6], m[7], m[8]),
        )
    }

    /// Flattens to a column-major array.
    #[inline]
    #[must_use]
    pub const fn to_cols_array(&self) -> [f32; 9] {
        [
            self.cols[0].x,
            self.cols[0].y,
            self.cols[0].z,
            self.cols[1].x,
            self.cols[1].y,
            self.cols[1].z,
            self.cols[2].x,
            self.cols[2].y,
            self.cols[2].z,
        ]
    }

    /// Reads an element as `m[row][col]`.
    #[inline]
    #[must_use]
    pub const fn get(&self, row: usize, col: usize) -> f32 {
        let c = match col {
            0 => self.cols[0],
            1 => self.cols[1],
            _ => self.cols[2],
        };
        match row {
            0 => c.x,
            1 => c.y,
            _ => c.z,
        }
    }

    /// Rotation about the X axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_x(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, c, s), Vec3::new(0.0, -s, c))
    }

    /// Rotation about the Y axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_y(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(Vec3::new(c, 0.0, -s), Vec3::new(0.0, 1.0, 0.0), Vec3::new(s, 0.0, c))
    }

    /// Rotation about the Z axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_z(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), Vec3::new(0.0, 0.0, 1.0))
    }

    /// Rotation of `angle` radians about an arbitrary axis.
    ///
    /// Returns the identity when `axis` is degenerate.
    #[inline]
    #[must_use]
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        Quat::from_axis_angle(axis, angle).to_mat3()
    }

    /// Builds the rotation part of a quaternion.
    #[inline]
    #[must_use]
    pub fn from_quat(q: Quat) -> Self {
        q.to_mat3()
    }

    /// Builds the **orientation matrix** of something facing `forward`.
    ///
    /// The result is a proper rotation (`det = +1`): local `-Z` maps to
    /// `forward`, matching the engine-wide "forward is `-Z`" convention, and
    /// local `+Y` maps as close to `up_hint` as possible.
    ///
    /// `forward` need not be normalised. When it is parallel to `up_hint` the
    /// hint is replaced by `+Z`, so this never returns NaNs.
    ///
    /// ```
    /// use noxel_core::math::{Mat3, Vec3};
    /// let m = Mat3::from_forward_up(Vec3::X, Vec3::Y);
    /// let f = m * Vec3::new(0.0, 0.0, -1.0);
    /// assert!(f.approx_eq(Vec3::X, 1e-5));
    /// assert!((m.determinant() - 1.0).abs() < 1e-5, "must be a rotation");
    /// ```
    #[inline]
    #[must_use]
    pub fn from_forward_up(forward: Vec3, up_hint: Vec3) -> Self {
        let f = forward.try_normalize().unwrap_or(Vec3::new(0.0, 0.0, -1.0));
        let hint = if f.cross(up_hint).length_squared() < EPSILON * EPSILON {
            Vec3::Z
        } else {
            up_hint
        };
        let right = f.cross(hint).normalize_or_zero();
        let up = right.cross(f);
        // Column 2 is *backward* (+Z), which is what makes -Z come out as
        // `forward` and keeps the determinant at +1.
        Self::from_cols(right, up, -f)
    }

    /// Uniform or non-uniform scale matrix.
    #[inline]
    #[must_use]
    pub const fn from_scale(v: Vec3) -> Self {
        Self::from_cols(Vec3::new(v.x, 0.0, 0.0), Vec3::new(0.0, v.y, 0.0), Vec3::new(0.0, 0.0, v.z))
    }

    /// Extracts the upper-left 3×3 block of a [`Mat4`].
    #[inline]
    #[must_use]
    pub const fn from_mat4(m: &Mat4) -> Self {
        let c = &m.cols;
        Self::from_cols(
            Vec3::new(c[0].x, c[0].y, c[0].z),
            Vec3::new(c[1].x, c[1].y, c[1].z),
            Vec3::new(c[2].x, c[2].y, c[2].z),
        )
    }

    /// Transposed matrix.
    #[inline]
    #[must_use]
    pub fn transpose(&self) -> Self {
        Self::from_cols(
            Vec3::new(self.get(0, 0), self.get(0, 1), self.get(0, 2)),
            Vec3::new(self.get(1, 0), self.get(1, 1), self.get(1, 2)),
            Vec3::new(self.get(2, 0), self.get(2, 1), self.get(2, 2)),
        )
    }

    /// Determinant.
    #[inline]
    #[must_use]
    pub fn determinant(&self) -> f32 {
        let c = &self.cols;
        c[0].x * (c[1].y * c[2].z - c[1].z * c[2].y)
            - c[1].x * (c[0].y * c[2].z - c[0].z * c[2].y)
            + c[2].x * (c[0].y * c[1].z - c[0].z * c[1].y)
    }

    /// Inverse, or `None` when the matrix is singular.
    #[inline]
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        let (a, b, c) = (self.cols[0], self.cols[1], self.cols[2]);
        let c00 = b.y * c.z - b.z * c.y;
        let c01 = b.z * c.x - b.x * c.z;
        let c02 = b.x * c.y - b.y * c.x;
        let c10 = a.z * c.y - a.y * c.z;
        let c11 = a.x * c.z - a.z * c.x;
        let c12 = a.y * c.x - a.x * c.y;
        let c20 = a.y * b.z - a.z * b.y;
        let c21 = a.z * b.x - a.x * b.z;
        let c22 = a.x * b.y - a.y * b.x;
        Some(Self::from_cols(
            Vec3::new(c00, c10, c20) * inv_det,
            Vec3::new(c01, c11, c21) * inv_det,
            Vec3::new(c02, c12, c22) * inv_det,
        ))
    }

    /// Inverse transpose: the matrix used to transform normals under
    /// non-uniform scale.
    ///
    /// Falls back to `self` when the matrix is singular.
    #[inline]
    #[must_use]
    pub fn inverse_transpose(&self) -> Self {
        self.inverse().map_or(*self, |m| m.transpose())
    }

    /// Transforms a column vector.
    #[inline]
    #[must_use]
    pub fn mul_vec3(&self, v: Vec3) -> Vec3 {
        self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z
    }

    /// True when every element is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.cols.iter().all(|c| c.is_finite())
    }
}

impl Mul for Mat3 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self::from_cols(
            self.mul_vec3(rhs.cols[0]),
            self.mul_vec3(rhs.cols[1]),
            self.mul_vec3(rhs.cols[2]),
        )
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.mul_vec3(rhs)
    }
}

impl Mat4 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        cols: [
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ],
    };

    /// The zero matrix.
    pub const ZERO: Self = Self { cols: [Vec4::ZERO; 4] };

    /// Builds from four column vectors.
    #[inline]
    #[must_use]
    pub const fn from_cols(x: Vec4, y: Vec4, z: Vec4, w: Vec4) -> Self {
        Self { cols: [x, y, z, w] }
    }

    /// Builds from a flat column-major array (16 elements).
    #[inline]
    #[must_use]
    pub const fn from_cols_array(m: &[f32; 16]) -> Self {
        Self::from_cols(
            Vec4::new(m[0], m[1], m[2], m[3]),
            Vec4::new(m[4], m[5], m[6], m[7]),
            Vec4::new(m[8], m[9], m[10], m[11]),
            Vec4::new(m[12], m[13], m[14], m[15]),
        )
    }

    /// Flattens to a column-major array, ready for GPU upload.
    #[inline]
    #[must_use]
    pub const fn to_cols_array(&self) -> [f32; 16] {
        [
            self.cols[0].x,
            self.cols[0].y,
            self.cols[0].z,
            self.cols[0].w,
            self.cols[1].x,
            self.cols[1].y,
            self.cols[1].z,
            self.cols[1].w,
            self.cols[2].x,
            self.cols[2].y,
            self.cols[2].z,
            self.cols[2].w,
            self.cols[3].x,
            self.cols[3].y,
            self.cols[3].z,
            self.cols[3].w,
        ]
    }

    /// Reads an element as `m[row][col]`.
    #[inline]
    #[must_use]
    pub const fn get(&self, row: usize, col: usize) -> f32 {
        let c = match col {
            0 => self.cols[0],
            1 => self.cols[1],
            2 => self.cols[2],
            _ => self.cols[3],
        };
        match row {
            0 => c.x,
            1 => c.y,
            2 => c.z,
            _ => c.w,
        }
    }

    /// Translation matrix.
    #[inline]
    #[must_use]
    pub const fn from_translation(t: Vec3) -> Self {
        Self::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(t.x, t.y, t.z, 1.0),
        )
    }

    /// Scale matrix.
    #[inline]
    #[must_use]
    pub const fn from_scale(s: Vec3) -> Self {
        Self::from_cols(
            Vec4::new(s.x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, s.y, 0.0, 0.0),
            Vec4::new(0.0, 0.0, s.z, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Rotation about the X axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_x(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, c, s, 0.0),
            Vec4::new(0.0, -s, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Rotation about the Y axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_y(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(
            Vec4::new(c, 0.0, -s, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(s, 0.0, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Rotation about the Z axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_z(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self::from_cols(
            Vec4::new(c, s, 0.0, 0.0),
            Vec4::new(-s, c, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Rotation matrix from a unit quaternion.
    #[inline]
    #[must_use]
    pub fn from_quat(q: Quat) -> Self {
        let m = q.to_mat3();
        Self::from_cols(
            m.cols[0].extend(0.0),
            m.cols[1].extend(0.0),
            m.cols[2].extend(0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Full TRS composition (scale, then rotate, then translate).
    #[inline]
    #[must_use]
    pub fn from_scale_rotation_translation(scale: Vec3, rotation: Quat, translation: Vec3) -> Self {
        let mut m = Self::from_quat(rotation);
        m.cols[0] = m.cols[0] * scale.x;
        m.cols[1] = m.cols[1] * scale.y;
        m.cols[2] = m.cols[2] * scale.z;
        m.cols[3] = translation.extend(1.0);
        m
    }

    /// Right-handed view matrix.
    ///
    /// The camera sits at `eye` looking at `target`; `up` is the world-up hint.
    /// `forward` is `-Z` in view space, as everywhere else in Noxel.
    #[inline]
    #[must_use]
    pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let f = (target - eye).try_normalize().unwrap_or(Vec3::new(0.0, 0.0, -1.0));
        // Degenerate up hint (camera looking straight down) -> pick a safe axis
        // so a perfectly top-down camera does not produce a NaN matrix.
        let up = if f.cross(up).length_squared() < EPSILON * EPSILON { Vec3::Z } else { up };
        let s = f.cross(up).normalize_or_zero();
        let u = s.cross(f);
        Self::from_cols(
            Vec4::new(s.x, u.x, -f.x, 0.0),
            Vec4::new(s.y, u.y, -f.y, 0.0),
            Vec4::new(s.z, u.z, -f.z, 0.0),
            Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
        )
    }

    /// Right-handed perspective projection with **depth in `[0, 1]`**.
    ///
    /// The `[0, 1]` range (rather than OpenGL's `[-1, 1]`) matches wgpu, D3D and
    /// Metal, and is what the software rasterizer expects.
    #[inline]
    #[must_use]
    pub fn perspective_rh(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fov_y_radians * 0.5).tan();
        let nf = 1.0 / (near - far);
        Self::from_cols(
            Vec4::new(f / aspect, 0.0, 0.0, 0.0),
            Vec4::new(0.0, f, 0.0, 0.0),
            Vec4::new(0.0, 0.0, far * nf, -1.0),
            Vec4::new(0.0, 0.0, near * far * nf, 0.0),
        )
    }

    /// Right-handed orthographic projection with **depth in `[0, 1]`**.
    ///
    /// This is the projection the top-down camera uses in its default
    /// pixel-art mode: no perspective distortion, so a tile grid stays a
    /// perfect grid on screen.
    #[inline]
    #[must_use]
    pub fn orthographic_rh(
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let rl = 1.0 / (right - left);
        let tb = 1.0 / (top - bottom);
        let fnear = 1.0 / (far - near);
        Self::from_cols(
            Vec4::new(2.0 * rl, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 * tb, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -fnear, 0.0),
            Vec4::new(
                -(right + left) * rl,
                -(top + bottom) * tb,
                -near * fnear,
                1.0,
            ),
        )
    }

    /// Transposed matrix.
    #[inline]
    #[must_use]
    pub fn transpose(&self) -> Self {
        Self::from_cols(
            Vec4::new(self.get(0, 0), self.get(0, 1), self.get(0, 2), self.get(0, 3)),
            Vec4::new(self.get(1, 0), self.get(1, 1), self.get(1, 2), self.get(1, 3)),
            Vec4::new(self.get(2, 0), self.get(2, 1), self.get(2, 2), self.get(2, 3)),
            Vec4::new(self.get(3, 0), self.get(3, 1), self.get(3, 2), self.get(3, 3)),
        )
    }

    /// Determinant.
    #[inline]
    #[must_use]
    pub fn determinant(&self) -> f32 {
        let m = self;
        let (m00, m01, m02, m03) = (m.get(0, 0), m.get(0, 1), m.get(0, 2), m.get(0, 3));
        let (m10, m11, m12, m13) = (m.get(1, 0), m.get(1, 1), m.get(1, 2), m.get(1, 3));
        let (m20, m21, m22, m23) = (m.get(2, 0), m.get(2, 1), m.get(2, 2), m.get(2, 3));
        let (m30, m31, m32, m33) = (m.get(3, 0), m.get(3, 1), m.get(3, 2), m.get(3, 3));

        let s0 = m00 * m11 - m10 * m01;
        let s1 = m00 * m12 - m10 * m02;
        let s2 = m00 * m13 - m10 * m03;
        let s3 = m01 * m12 - m11 * m02;
        let s4 = m01 * m13 - m11 * m03;
        let s5 = m02 * m13 - m12 * m03;
        let c5 = m22 * m33 - m32 * m23;
        let c4 = m21 * m33 - m31 * m23;
        let c3 = m21 * m32 - m31 * m22;
        let c2 = m20 * m33 - m30 * m23;
        let c1 = m20 * m32 - m30 * m22;
        let c0 = m20 * m31 - m30 * m21;
        s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0
    }

    /// General 4×4 inverse, or `None` when the matrix is singular.
    #[inline]
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let m = self;
        let (m00, m01, m02, m03) = (m.get(0, 0), m.get(0, 1), m.get(0, 2), m.get(0, 3));
        let (m10, m11, m12, m13) = (m.get(1, 0), m.get(1, 1), m.get(1, 2), m.get(1, 3));
        let (m20, m21, m22, m23) = (m.get(2, 0), m.get(2, 1), m.get(2, 2), m.get(2, 3));
        let (m30, m31, m32, m33) = (m.get(3, 0), m.get(3, 1), m.get(3, 2), m.get(3, 3));

        let s0 = m00 * m11 - m10 * m01;
        let s1 = m00 * m12 - m10 * m02;
        let s2 = m00 * m13 - m10 * m03;
        let s3 = m01 * m12 - m11 * m02;
        let s4 = m01 * m13 - m11 * m03;
        let s5 = m02 * m13 - m12 * m03;
        let c5 = m22 * m33 - m32 * m23;
        let c4 = m21 * m33 - m31 * m23;
        let c3 = m21 * m32 - m31 * m22;
        let c2 = m20 * m33 - m30 * m23;
        let c1 = m20 * m32 - m30 * m22;
        let c0 = m20 * m31 - m30 * m21;

        let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;

        let i00 = (m11 * c5 - m12 * c4 + m13 * c3) * inv;
        let i01 = (-m01 * c5 + m02 * c4 - m03 * c3) * inv;
        let i02 = (m31 * s5 - m32 * s4 + m33 * s3) * inv;
        let i03 = (-m21 * s5 + m22 * s4 - m23 * s3) * inv;

        let i10 = (-m10 * c5 + m12 * c2 - m13 * c1) * inv;
        let i11 = (m00 * c5 - m02 * c2 + m03 * c1) * inv;
        let i12 = (-m30 * s5 + m32 * s2 - m33 * s1) * inv;
        let i13 = (m20 * s5 - m22 * s2 + m23 * s1) * inv;

        let i20 = (m10 * c4 - m11 * c2 + m13 * c0) * inv;
        let i21 = (-m00 * c4 + m01 * c2 - m03 * c0) * inv;
        let i22 = (m30 * s4 - m31 * s2 + m33 * s0) * inv;
        let i23 = (-m20 * s4 + m21 * s2 - m23 * s0) * inv;

        let i30 = (-m10 * c3 + m11 * c1 - m12 * c0) * inv;
        let i31 = (m00 * c3 - m01 * c1 + m02 * c0) * inv;
        let i32 = (-m30 * s3 + m31 * s1 - m32 * s0) * inv;
        let i33 = (m20 * s3 - m21 * s1 + m22 * s0) * inv;

        Some(Self::from_cols(
            Vec4::new(i00, i10, i20, i30),
            Vec4::new(i01, i11, i21, i31),
            Vec4::new(i02, i12, i22, i32),
            Vec4::new(i03, i13, i23, i33),
        ))
    }

    /// Extracts the translation component (column 3).
    #[inline]
    #[must_use]
    pub const fn translation(&self) -> Vec3 {
        self.cols[3].truncate()
    }

    /// Transforms a homogeneous point (implicit `w = 1`).
    #[inline]
    #[must_use]
    pub fn transform_point3(&self, p: Vec3) -> Vec3 {
        let v = self.transform_point4(Vec4::new(p.x, p.y, p.z, 1.0));
        v.truncate()
    }

    /// Transforms a homogeneous vector (implicit `w = 0`): translation is
    /// ignored.
    #[inline]
    #[must_use]
    pub fn transform_vector3(&self, v: Vec3) -> Vec3 {
        let x = self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z;
        x.truncate()
    }

    /// Transforms a homogeneous vector without a perspective divide.
    #[inline]
    #[must_use]
    pub fn transform_point4(&self, p: Vec4) -> Vec4 {
        self.cols[0] * p.x + self.cols[1] * p.y + self.cols[2] * p.z + self.cols[3] * p.w
    }

    /// Builds the matrix that pulls a 2D axis-aligned rectangle in world space
    /// onto a unit quad.
    ///
    /// The billboard renderer uses this to keep sprite quads axis-aligned under
    /// the top-down camera: the quad is built flat in the XZ plane, so pixel art
    /// keeps its crisp grid alignment instead of shearing.
    #[inline]
    #[must_use]
    pub fn billboard_xz(center: Vec3, size: super::vec::Vec2, rotation: f32) -> Self {
        let mut m = Self::from_rotation_y(rotation);
        m.cols[0] = m.cols[0] * size.x;
        m.cols[2] = m.cols[2] * size.y;
        m.cols[3] = center.extend(1.0);
        m
    }

    /// True when every element is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.cols.iter().all(|c| c.is_finite())
    }
}

impl Mul for Mat4 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        let mut out = Self::ZERO;
        for c in 0..4 {
            let col = rhs.cols[c];
            out.cols[c] = self.cols[0] * col.x
                + self.cols[1] * col.y
                + self.cols[2] * col.z
                + self.cols[3] * col.w;
        }
        out
    }
}

impl Mul<Vec4> for Mat4 {
    type Output = Vec4;
    #[inline]
    fn mul(self, rhs: Vec4) -> Vec4 {
        self.transform_point4(rhs)
    }
}

impl Mul<Mat4> for f32 {
    type Output = Mat4;
    #[inline]
    fn mul(self, rhs: Mat4) -> Mat4 {
        let mut m = rhs;
        for c in m.cols.iter_mut() {
            *c = *c * self;
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Quat;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn mat4_identity_is_neutral() {
        let m = Mat4::from_rotation_y(0.7);
        assert_eq!(m * Mat4::IDENTITY, m);
        assert_eq!(Mat4::IDENTITY * m, m);
    }

    #[test]
    fn mat4_translation_transforms_point_not_vector() {
        let t = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(t.transform_point3(Vec3::ZERO), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(t.transform_vector3(Vec3::X), Vec3::X);
    }

    #[test]
    fn mat4_inverse_roundtrip() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 0.5, 1.5),
            Quat::from_rotation_y(0.9),
            Vec3::new(-4.0, 1.0, 7.0),
        );
        let inv = m.inverse().expect("invertible");
        let p = Vec3::new(3.0, -2.0, 0.5);
        let back = inv.transform_point3(m.transform_point3(p));
        assert!(back.approx_eq(p, 1e-3), "{back:?}");
    }

    #[test]
    fn mat4_singular_has_no_inverse() {
        assert!(Mat4::ZERO.inverse().is_none());
    }

    #[test]
    fn look_at_places_target_on_negative_z() {
        let view = Mat4::look_at_rh(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::Z);
        let p = view.transform_point3(Vec3::ZERO);
        assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4);
        assert!(p.z < 0.0, "target should be in front (-Z), got {p:?}");
    }

    #[test]
    fn perfectly_top_down_look_at_is_finite() {
        // up == view direction would be degenerate for a naive implementation.
        let view = Mat4::look_at_rh(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0));
        assert!(view.is_finite(), "{view:?}");
    }

    #[test]
    fn perspective_maps_near_to_zero_and_far_to_one() {
        let proj = Mat4::perspective_rh(1.0, 1.6, 0.1, 100.0);
        let near = proj.transform_point4(Vec4::new(0.0, 0.0, -0.1, 1.0));
        let far = proj.transform_point4(Vec4::new(0.0, 0.0, -100.0, 1.0));
        assert!(approx(near.z / near.w, 0.0), "{}", near.z / near.w);
        assert!(approx(far.z / far.w, 1.0), "{}", far.z / far.w);
    }

    #[test]
    fn orthographic_maps_near_to_zero_and_far_to_one() {
        let proj = Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, 0.5, 50.0);
        let near = proj.transform_point4(Vec4::new(0.0, 0.0, -0.5, 1.0));
        let far = proj.transform_point4(Vec4::new(0.0, 0.0, -50.0, 1.0));
        assert!(approx(near.z, 0.0));
        assert!(approx(far.z, 1.0));
    }

    #[test]
    fn transpose_swaps_elements() {
        let mut m = Mat4::IDENTITY;
        m.cols[0].y = 3.0; // row 1, column 0
        let t = m.transpose();
        assert!(approx(t.get(0, 1), 3.0), "element must move to row 0, column 1");
        assert!(approx(t.get(1, 0), 0.0));
    }

    #[test]
    fn mat3_rotation_y_maps_z_to_x() {
        // Positive yaw about +Y takes -Z (forward) towards +X (right).
        let r = Mat3::from_rotation_y(core::f32::consts::FRAC_PI_2);
        let v = r * Vec3::Z;
        assert!(v.approx_eq(Vec3::X, 1e-5), "{v:?}");
        let f = r * Vec3::new(0.0, 0.0, -1.0);
        assert!(f.approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-5), "{f:?}");
    }

    #[test]
    fn mat3_inverse_transpose_handles_nonuniform_scale() {
        let m = Mat3::from_scale(Vec3::new(2.0, 4.0, 8.0));
        let n = m.inverse_transpose();
        let scaled = m * Vec3::new(1.0, 1.0, 1.0);
        let normal = n * scaled;
        assert!(normal.is_finite());
    }

    #[test]
    fn from_forward_up_is_a_proper_rotation() {
        let cases = [
            (Vec3::X, Vec3::Y),
            (Vec3::new(1.0, -1.0, 0.5), Vec3::Y),
            (Vec3::new(0.0, 1.0, 0.0), Vec3::Y), // parallel to the hint
            (Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        ];
        for (fwd, up) in cases {
            let m = Mat3::from_forward_up(fwd, up);
            let (x, y, z) = (m.cols[0], m.cols[1], m.cols[2]);
            assert!(approx(x.length(), 1.0) && approx(y.length(), 1.0) && approx(z.length(), 1.0));
            assert!(approx(x.dot(y), 0.0) && approx(y.dot(z), 0.0) && approx(x.dot(z), 0.0));
            // A rotation basis must have determinant +1, not -1 (a mirror would
            // flip winding order and normals).
            assert!(approx(m.determinant(), 1.0), "det = {}", m.determinant());
            // And local -Z must come out as `forward`.
            let got = m * Vec3::new(0.0, 0.0, -1.0);
            let want = fwd.normalize_or_zero();
            assert!((got - want).length() < 1e-4, "{fwd:?} -> {got:?}");
        }
    }
}
