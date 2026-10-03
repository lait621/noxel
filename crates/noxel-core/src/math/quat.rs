//! Unit quaternions for rotation.
//!
//! Stored as `(x, y, z, w)` with `w` as the scalar part, matching the
//! convention used by glTF, wgpu and every quaternion tutorial a contributor
//! will look up.

use core::ops::Mul;

use super::mat::Mat3;
use super::scalar::{EPSILON, angle_delta, wrap_angle};
use super::vec::Vec3;

/// A rotation represented as a unit quaternion.
///
/// Noxel deliberately does **not** normalise on every operation: that would cost
/// a square root in the hottest maths path for no visible benefit. Call
/// [`Quat::normalize`] after accumulating many multiplications (the engine does
/// this once per frame for animated transforms).
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Quat {
    /// Vector part, x component.
    pub x: f32,
    /// Vector part, y component.
    pub y: f32,
    /// Vector part, z component.
    pub z: f32,
    /// Scalar part.
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    /// The identity rotation.
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    /// Zero quaternion; not a valid rotation, useful as an accumulator init.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };

    /// Constructs a quaternion from components.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Constructs a quaternion from an axis vector and an angle.
    #[inline]
    #[must_use]
    pub const fn from_xyzw(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Rotation of `angle` radians about `axis` (normalised internally).
    ///
    /// Returns the identity when `axis` is degenerate.
    #[inline]
    #[must_use]
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        let Some(axis) = axis.try_normalize() else {
            return Self::IDENTITY;
        };
        let (s, c) = (angle * 0.5).sin_cos();
        Self::new(axis.x * s, axis.y * s, axis.z * s, c)
    }

    /// Rotation about the X axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_x(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::new(s, 0.0, 0.0, c)
    }

    /// Rotation about the Y axis (yaw).
    #[inline]
    #[must_use]
    pub fn from_rotation_y(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::new(0.0, s, 0.0, c)
    }

    /// Rotation about the Z axis.
    #[inline]
    #[must_use]
    pub fn from_rotation_z(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::new(0.0, 0.0, s, c)
    }

    /// Rotation from intrinsic yaw (Y), pitch (X) and roll (Z) angles.
    ///
    /// Applied in Z→X→Y order, which is the convention the camera rigs and the
    /// world generator assume. For a top-down game only `yaw` is usually needed.
    #[inline]
    #[must_use]
    pub fn from_euler(yaw: f32, pitch: f32, roll: f32) -> Self {
        Self::from_rotation_y(yaw) * Self::from_rotation_x(pitch) * Self::from_rotation_z(roll)
    }

    /// Rotation that turns `-Z` (the engine's forward) into `forward`.
    ///
    /// `up` breaks the roll ambiguity. Falls back to a Y-aligned result when
    /// `forward` is degenerate.
    #[inline]
    #[must_use]
    pub fn look_rotation(forward: Vec3, up: Vec3) -> Self {
        // `Mat3::from_forward_up` is the single source of truth for the
        // orientation convention; share it rather than duplicating the algebra.
        Self::from_mat3(Mat3::from_forward_up(forward, up))
    }

    /// Builds the rotation part of a 3×3 matrix.
    ///
    /// Assumes the matrix is a pure rotation with an orthonormal basis.
    #[inline]
    #[must_use]
    pub fn from_mat3(m: Mat3) -> Self {
        let trace = m.get(0, 0) + m.get(1, 1) + m.get(2, 2);
        if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Self::new(
                (m.get(2, 1) - m.get(1, 2)) / s,
                (m.get(0, 2) - m.get(2, 0)) / s,
                (m.get(1, 0) - m.get(0, 1)) / s,
                0.25 * s,
            )
        } else if m.get(0, 0) > m.get(1, 1) && m.get(0, 0) > m.get(2, 2) {
            let s = (1.0 + m.get(0, 0) - m.get(1, 1) - m.get(2, 2)).sqrt() * 2.0;
            Self::new(
                0.25 * s,
                (m.get(0, 1) + m.get(1, 0)) / s,
                (m.get(0, 2) + m.get(2, 0)) / s,
                (m.get(2, 1) - m.get(1, 2)) / s,
            )
        } else if m.get(1, 1) > m.get(2, 2) {
            let s = (1.0 + m.get(1, 1) - m.get(0, 0) - m.get(2, 2)).sqrt() * 2.0;
            Self::new(
                (m.get(0, 1) + m.get(1, 0)) / s,
                0.25 * s,
                (m.get(1, 2) + m.get(2, 1)) / s,
                (m.get(0, 2) - m.get(2, 0)) / s,
            )
        } else {
            let s = (1.0 + m.get(2, 2) - m.get(0, 0) - m.get(1, 1)).sqrt() * 2.0;
            Self::new(
                (m.get(0, 2) + m.get(2, 0)) / s,
                (m.get(1, 2) + m.get(2, 1)) / s,
                0.25 * s,
                (m.get(1, 0) - m.get(0, 1)) / s,
            )
        }
        .normalize()
    }

    /// Converts to a 3×3 rotation matrix.
    #[inline]
    #[must_use]
    pub fn to_mat3(self) -> Mat3 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        Mat3::from_cols(
            Vec3::new(1.0 - (yy + zz), xy + wz, xz - wy),
            Vec3::new(xy - wz, 1.0 - (xx + zz), yz + wx),
            Vec3::new(xz + wy, yz - wx, 1.0 - (xx + yy)),
        )
    }

    /// Squared length.
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w
    }

    /// Returns a unit quaternion (identity when degenerate).
    #[inline]
    #[must_use]
    pub fn normalize(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq < EPSILON * EPSILON {
            Self::IDENTITY
        } else {
            let inv = 1.0 / len_sq.sqrt();
            Self::new(self.x * inv, self.y * inv, self.z * inv, self.w * inv)
        }
    }

    /// The conjugate; for a unit quaternion this is the inverse rotation.
    #[inline]
    #[must_use]
    pub const fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    /// The inverse rotation, or `None` when degenerate.
    #[inline]
    #[must_use]
    pub fn inverse(self) -> Option<Self> {
        let len_sq = self.length_squared();
        if len_sq < EPSILON * EPSILON {
            None
        } else {
            let inv = 1.0 / len_sq;
            Some(Self::new(-self.x * inv, -self.y * inv, -self.z * inv, self.w * inv))
        }
    }

    /// Dot product, used for the shortest-arc decision in slerp.
    #[inline]
    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    /// Rotates a vector by this quaternion.
    #[inline]
    #[must_use]
    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        // v + 2w(q×v) + 2q×(q×v)
        let q = Vec3::new(self.x, self.y, self.z);
        let t = q.cross(v) * 2.0;
        v + t * self.w + q.cross(t)
    }

    /// Rotates a vector by the inverse of this quaternion.
    #[inline]
    #[must_use]
    pub fn inverse_rotate_vec3(self, v: Vec3) -> Vec3 {
        self.conjugate().rotate_vec3(v)
    }

    /// Shortest-arc spherical interpolation.
    #[inline]
    #[must_use]
    pub fn slerp(self, rhs: Self, t: f32) -> Self {
        let mut rhs = rhs;
        let mut dot = self.dot(rhs);
        if dot < 0.0 {
            rhs = rhs.conjugate();
            dot = -dot;
        }
        if dot > 0.999_5 {
            // Nearly parallel: nlerp avoids a division by ~0.
            return Self::new(
                self.x + (rhs.x - self.x) * t,
                self.y + (rhs.y - self.y) * t,
                self.z + (rhs.z - self.z) * t,
                self.w + (rhs.w - self.w) * t,
            )
            .normalize();
        }
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin_theta = theta.sin();
        let w0 = ((1.0 - t) * theta).sin() / sin_theta;
        let w1 = (t * theta).sin() / sin_theta;
        Self::new(
            self.x * w0 + rhs.x * w1,
            self.y * w0 + rhs.y * w1,
            self.z * w0 + rhs.z * w1,
            self.w * w0 + rhs.w * w1,
        )
    }

    /// Extracts yaw (Y axis rotation) in radians.
    ///
    /// This is the hot accessor for a top-down game: an NPC "facing" is a yaw.
    #[inline]
    #[must_use]
    pub fn to_yaw(self) -> f32 {
        // Forward is -Z and yaw is measured about +Y, so
        // `Vec3::from_yaw(yaw) == (-sin yaw, 0, -cos yaw)`; invert that.
        let f = self.rotate_vec3(Vec3::new(0.0, 0.0, -1.0));
        wrap_angle((-f.x).atan2(-f.z))
    }

    /// Extracts `(yaw, pitch, roll)`, matching [`Quat::from_euler`].
    #[inline]
    #[must_use]
    pub fn to_euler(self) -> (f32, f32, f32) {
        // For M = Ry(yaw) * Rx(pitch) * Rz(roll):
        //   m12 = -sin(pitch)
        //   m02 =  sin(yaw)cos(pitch),  m22 = cos(yaw)cos(pitch)
        //   m10 =  cos(pitch)sin(roll), m11 = cos(pitch)cos(roll)
        let m = self.to_mat3();
        let sin_pitch = -m.get(1, 2).clamp(-1.0, 1.0);
        let pitch = sin_pitch.asin();
        if sin_pitch.abs() > 0.999_9 {
            // Gimbal lock: yaw and roll become the same degree of freedom, so
            // fold roll into yaw. The sign of `sin_pitch` selects which of the
            // two equivalent decompositions keeps the result continuous.
            let s = if sin_pitch >= 0.0 { 1.0 } else { -1.0 };
            (wrap_angle((m.get(2, 0) * s).atan2(m.get(0, 0))), pitch, 0.0)
        } else {
            let yaw = wrap_angle(m.get(0, 2).atan2(m.get(2, 2)));
            let roll = wrap_angle(m.get(1, 0).atan2(m.get(1, 1)));
            (yaw, pitch, roll)
        }
    }

    /// Rotates towards `target` by at most `max_step` radians.
    ///
    /// The quaternion analogue of `scalar::rotate_towards`; the NPC steering
    /// code uses it so a crowd turns smoothly without overshooting.
    #[inline]
    #[must_use]
    pub fn rotate_towards(self, target: Self, max_step: f32) -> Self {
        let mut target = target;
        let mut dot = self.dot(target);
        if dot < 0.0 {
            target = target.conjugate();
            dot = -dot;
        }
        if dot > 0.999_5 {
            return target;
        }
        // `dot` is cos(half-angle); callers think in full rotation angle.
        let half_angle = dot.clamp(-1.0, 1.0).acos();
        let angle = 2.0 * half_angle;
        if angle <= max_step {
            return target;
        }
        self.slerp(target, max_step / angle)
    }

    /// True when every component is finite.
    #[inline]
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }

    /// Yaw-only helper: builds a rotation about Y from an angle.
    #[inline]
    #[must_use]
    pub fn yaw(angle: f32) -> Self {
        Self::from_rotation_y(angle)
    }

    /// The signed yaw difference between two rotations, in `(-PI, PI]`.
    #[inline]
    #[must_use]
    pub fn yaw_delta(self, other: Self) -> f32 {
        angle_delta(self.to_yaw(), other.to_yaw())
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

    /// Converts to a `Vec4` for uniform upload.
    #[inline]
    #[must_use]
    pub const fn to_vec4(self) -> super::vec::Vec4 {
        super::vec::Vec4::new(self.x, self.y, self.z, self.w)
    }
}

impl Mul for Quat {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        )
    }
}

impl Mul<Vec3> for Quat {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.rotate_vec3(rhs)
    }
}

impl From<Quat> for Mat3 {
    #[inline]
    fn from(q: Quat) -> Self {
        q.to_mat3()
    }
}

impl From<Mat3> for Quat {
    #[inline]
    fn from(m: Mat3) -> Self {
        Self::from_mat3(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rotates_nothing() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert!(Quat::IDENTITY.rotate_vec3(v).approx_eq(v, 1e-6));
    }

    #[test]
    fn yaw_rotation_matches_matrix() {
        let q = Quat::from_rotation_y(0.9);
        let m = Mat3::from_rotation_y(0.9);
        let v = Vec3::new(1.0, 0.0, 2.0);
        assert!((q.rotate_vec3(v) - m * v).length() < 1e-5);
    }

    #[test]
    fn conjugate_is_inverse() {
        let q = Quat::from_axis_angle(Vec3::new(1.0, 2.0, 0.5), 1.1).normalize();
        let v = Vec3::new(-3.0, 0.5, 2.0);
        let back = q.conjugate().rotate_vec3(q.rotate_vec3(v));
        assert!((back - v).length() < 1e-4);
    }

    #[test]
    fn composition_order_matches_matrices() {
        let a = Quat::from_rotation_y(0.4);
        let b = Quat::from_rotation_x(0.8);
        let v = Vec3::new(0.3, -1.0, 2.0);
        let q = (a * b).rotate_vec3(v);
        let m = (a.to_mat3() * b.to_mat3()) * v;
        assert!((q - m).length() < 1e-4, "{q:?} vs {m:?}");
    }

    #[test]
    fn to_yaw_roundtrips() {
        for i in -6..=6 {
            let yaw = i as f32 * 0.5;
            let q = Quat::from_euler(yaw, 0.0, 0.0);
            assert!(angle_delta(yaw, q.to_yaw()).abs() < 1e-3, "{yaw} -> {}", q.to_yaw());
        }
    }

    #[test]
    fn euler_roundtrip() {
        let (y, p, r) = (0.7, 0.3, -0.2);
        let q = Quat::from_euler(y, p, r);
        let (y2, p2, r2) = q.to_euler();
        assert!(angle_delta(y, y2).abs() < 1e-3);
        assert!((p - p2).abs() < 1e-3);
        assert!(angle_delta(r, r2).abs() < 1e-3);
    }

    #[test]
    fn slerp_endpoints() {
        let a = Quat::from_rotation_y(0.0);
        let b = Quat::from_rotation_y(1.0);
        assert!(a.slerp(b, 0.0).to_yaw().abs() < 1e-4);
        assert!((a.slerp(b, 1.0).to_yaw() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn slerp_takes_short_way_across_pi() {
        let a = Quat::from_rotation_y(3.0);
        let b = Quat::from_rotation_y(-3.0);
        let mid = a.slerp(b, 0.5).to_yaw();
        // Short way is through PI (or -PI), never through 0.
        assert!(mid.abs() > 3.0, "mid yaw {mid}");
    }

    #[test]
    fn rotate_towards_respects_step() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_y(1.0);
        let step = a.rotate_towards(b, 0.25).to_yaw();
        assert!((step - 0.25).abs() < 1e-3, "{step}");
    }

    #[test]
    fn look_rotation_points_forward() {
        let q = Quat::look_rotation(Vec3::new(1.0, 0.0, 0.0), Vec3::Y);
        let f = q.rotate_vec3(Vec3::new(0.0, 0.0, -1.0));
        assert!(f.approx_eq(Vec3::X, 1e-4), "{f:?}");
    }
}
