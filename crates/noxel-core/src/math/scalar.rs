//! Scalar helpers shared by every maths type.

/// Generic "close enough" tolerance for `f32` comparisons in game code.
///
/// This is deliberately loose: gameplay code compares positions and normals
/// that have been through several transform stacks, and a tighter epsilon
/// produces spurious jitter in the character controller.
pub const EPSILON: f32 = 1e-5;

/// Tolerance used by ray/bounds intersection tests, in world units.
///
/// Kept separate from [`EPSILON`] because a ray origin offset must be scaled to
/// the world (1 unit == 1 metre), not to normalised vectors.
pub const RAY_EPSILON: f32 = 1e-4;

/// `TAU`, i.e. a full turn in radians.
pub const TAU: f32 = core::f32::consts::TAU;

/// Converts degrees to radians.
#[inline]
#[must_use]
pub const fn to_radians(deg: f32) -> f32 {
    deg * (core::f32::consts::PI / 180.0)
}

/// Converts radians to degrees.
#[inline]
#[must_use]
pub const fn to_degrees(rad: f32) -> f32 {
    rad * (180.0 / core::f32::consts::PI)
}

/// Approximate equality with an explicit tolerance.
#[inline]
#[must_use]
pub fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

/// Wraps `angle` into `(-PI, PI]`.
#[inline]
#[must_use]
pub fn wrap_angle(angle: f32) -> f32 {
    let mut a = angle % TAU;
    if a > core::f32::consts::PI {
        a -= TAU;
    } else if a <= -core::f32::consts::PI {
        a += TAU;
    }
    a
}

/// The shortest signed difference `to - from`, wrapped into `(-PI, PI]`.
///
/// This is the function that makes turrets, NPCs and follow cameras turn the
/// short way round instead of spinning the long way. Use it instead of a naive
/// subtraction anywhere an angle is interpolated.
#[inline]
#[must_use]
pub fn angle_delta(from: f32, to: f32) -> f32 {
    wrap_angle(to - from)
}

/// Rotates `current` towards `target` by at most `max_step` radians.
#[inline]
#[must_use]
pub fn rotate_towards(current: f32, target: f32, max_step: f32) -> f32 {
    let d = angle_delta(current, target);
    if d.abs() <= max_step {
        target
    } else {
        wrap_angle(current + max_step * d.signum())
    }
}

/// Linear interpolation.
#[inline]
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolation with `t` clamped to `[0, 1]`.
#[inline]
#[must_use]
pub fn lerp_clamped(a: f32, b: f32, t: f32) -> f32 {
    lerp(a, b, t.clamp(0.0, 1.0))
}

/// Inverse lerp: the `t` such that `lerp(a, b, t) == v`. Returns 0 when `a == b`.
#[inline]
#[must_use]
pub fn inv_lerp(a: f32, b: f32, v: f32) -> f32 {
    if (b - a).abs() < f32::MIN_POSITIVE {
        0.0
    } else {
        (v - a) / (b - a)
    }
}

/// Smooth step between two edges.
#[inline]
#[must_use]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = inv_lerp(edge0, edge1, x).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smoother (Perlin) step between two edges.
#[inline]
#[must_use]
pub fn smootherstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = inv_lerp(edge0, edge1, x).clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// An exponential smoothing factor that is independent of the frame rate.
///
/// `smoothing` is the fraction of the remaining distance left after one second;
/// smaller values snap faster. Use this for camera and UI smoothing so a change
/// in frame rate does not change the feel.
///
/// ```
/// use noxel_core::math::damp_factor;
/// // The *remaining* fraction is what composes: two half-second steps leave the
/// // same residue as one one-second step, so the feel is frame-rate independent.
/// let a = damp_factor(0.1, 0.5);
/// let b = damp_factor(0.1, 0.5);
/// let one_step = damp_factor(0.1, 1.0);
/// assert!(((1.0 - a) * (1.0 - b) - (1.0 - one_step)).abs() < 1e-5);
/// ```
#[inline]
#[must_use]
pub fn damp_factor(smoothing: f32, dt: f32) -> f32 {
    if smoothing <= 0.0 {
        0.0
    } else {
        1.0 - smoothing.clamp(0.0, 1.0).powf(dt.max(0.0))
    }
}

/// Frame-rate independent exponential approach of `current` towards `target`.
#[inline]
#[must_use]
pub fn damp(current: f32, target: f32, smoothing: f32, dt: f32) -> f32 {
    lerp(current, target, damp_factor(smoothing, dt))
}

/// Clamps `v` into `[min, max]` **without panicking** when `min > max`.
///
/// `f32::clamp` panics on a reversed range, which is a real hazard in gameplay
/// code where the bounds come from data files.
#[inline]
#[must_use]
pub fn clamp_safe(v: f32, min: f32, max: f32) -> f32 {
    if min > max {
        (min + max) * 0.5
    } else {
        v.clamp(min, max)
    }
}

/// Integer floor division that rounds towards negative infinity, unlike `/`.
#[inline]
#[must_use]
pub const fn div_floor(a: i32, b: i32) -> i32 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// A positive integer modulus that wraps negative inputs correctly.
#[inline]
#[must_use]
pub const fn rem_euclid_i32(a: i32, b: i32) -> i32 {
    let r = a % b;
    if r < 0 { r + b.abs() } else { r }
}

/// Converts a linear colour component to sRGB for display.
#[inline]
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Converts an sRGB colour component to linear space.
#[inline]
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Reinhard tone mapping for a single channel.
#[inline]
#[must_use]
pub fn tonemap_reinhard(c: f32) -> f32 {
    c / (1.0 + c)
}

/// Filmic ACES approximation (Narkowicz). The default tone curve in the
/// rasterizer's post-processing stage.
#[inline]
#[must_use]
pub fn tonemap_aces(x: f32) -> f32 {
    const A: f32 = 2.51;
    const B: f32 = 0.03;
    const C: f32 = 2.43;
    const D: f32 = 0.59;
    const E: f32 = 0.14;
    ((x * (A * x + B)) / (x * (C * x + D) + E)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_angle_bounds() {
        assert!(approx_eq(wrap_angle(0.0), 0.0, 1e-6));
        assert!(approx_eq(wrap_angle(TAU), 0.0, 1e-6));
        assert!(wrap_angle(core::f32::consts::PI * 3.0).abs() <= core::f32::consts::PI);
        assert!(wrap_angle(-core::f32::consts::PI * 3.0).abs() <= core::f32::consts::PI);
    }

    #[test]
    fn angle_delta_takes_short_way() {
        let d = angle_delta(0.1, -0.1);
        assert!(d < 0.0 && d > -1.0);
        // The short way from -3 rad to +3 rad runs *backwards* through -PI.
        let d = angle_delta(-3.0, 3.0);
        assert!(d < 0.0 && d > -1.0, "got {d}");
        // Forward through zero is the short way for small angles.
        let d = angle_delta(0.2, 0.4);
        assert!((d - 0.2).abs() < 1e-6);
    }

    #[test]
    fn rotate_towards_respects_step() {
        let r = rotate_towards(0.0, 3.0, 0.5);
        assert!(approx_eq(r, 0.5, 1e-6));
        let r = rotate_towards(0.0, 0.2, 0.5);
        assert!(approx_eq(r, 0.2, 1e-6));
    }

    #[test]
    fn div_floor_and_rem() {
        assert_eq!(div_floor(-1, 16), -1);
        assert_eq!(div_floor(16, 16), 1);
        assert_eq!(rem_euclid_i32(-1, 16), 15);
        assert_eq!(rem_euclid_i32(17, 16), 1);
    }

    #[test]
    fn clamp_safe_handles_reversed_range() {
        assert_eq!(clamp_safe(5.0, 10.0, 0.0), 5.0);
        assert_eq!(clamp_safe(-5.0, 0.0, 10.0), 0.0);
    }

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=10 {
            let c = i as f32 / 10.0;
            let back = srgb_to_linear(linear_to_srgb(c));
            assert!(approx_eq(c, back, 1e-3), "{c} -> {back}");
        }
    }
}
