//! Sampling utilities for the ray tracer.
//!
//! Everything here is **deterministic**: samples come from a low-discrepancy
//! sequence indexed by `(pixel, sample)`, never from a global RNG. Two runs of
//! the same frame therefore produce byte-identical images, which is what makes
//! the golden-image tests in `tools/` meaningful.
//!
//! The sequences are the standard ones (Hammersley/van der Corput for the base
//! sequence, a branchless orthonormal basis after Duff et al.), chosen so the
//! result is easy to check against the literature.

use noxel_core::math::{Vec2, Vec3};
use noxel_core::rng::{Pcg32, hash_2d};

/// The van der Corput radical inverse in base 2.
///
/// Bit-reverses `bits`, producing the low-discrepancy sequence
/// `0, 0.5, 0.25, 0.75, 0.125, ...`.
#[must_use]
pub fn radical_inverse_vdc(bits: u32) -> f32 {
    let reversed = bits.reverse_bits();
    (reversed as f64 * 2.328_306_436_538_696_3e-10) as f32 // (1 / 2^32)
}

/// The `i`th point of the `n`-point 2D Hammersley set, in `[0, 1)²`.
///
/// `i` past `n` still produces a valid point (it degenerates to the van der
/// Corput sequence in the second component), which keeps the progressive
/// accumulation path simple.
#[must_use]
pub fn hammersley(i: u32, n: u32) -> Vec2 {
    let n = n.max(1);
    Vec2::new(i as f32 / n as f32, radical_inverse_vdc(i))
}

/// A stratified 2D sample: the Hammersley point, jittered inside its stratum.
///
/// Pure Hammersley produces a visibly regular pattern at low sample counts;
/// jittering across `sqrt(n)` strata removes the structure without losing the
/// low discrepancy.
#[must_use]
pub fn stratified(i: u32, n: u32, seed: u64) -> Vec2 {
    let n = n.max(1);
    let side = (n as f32).sqrt().ceil().max(1.0) as u32;
    // Wrap the index so asking for more samples than `n` stays in the unit
    // square instead of walking off the top row.
    let i = i % n;
    let (sx, sy) = (i % side, (i / side) % side);
    let jitter_x = hash_to_unit(sx, sy, seed);
    let jitter_y = hash_to_unit(sx, sy, seed ^ 0x9E37_79B9);
    Vec2::new(
        (sx as f32 + jitter_x) / side as f32,
        (sy as f32 + jitter_y) / side as f32,
    )
}

/// A deterministic value in `[0, 1)` from two integers and a seed.
#[must_use]
pub fn hash_to_unit(x: u32, y: u32, seed: u64) -> f32 {
    ((hash_2d(x as i32, y as i32, seed) >> 40) as f32) * (1.0 / (1u32 << 24) as f32)
}

/// Builds a right-handed orthonormal basis around `n`.
///
/// The branchless construction from Duff et al., "Building an Orthonormal Basis,
/// Revisited" — it has no singularity anywhere on the sphere, which matters
/// because a computing-intensive tracer will eventually hit whatever degenerate
/// case a naive `cross` implementation has.
#[must_use]
pub fn build_onb(n: Vec3) -> (Vec3, Vec3) {
    let sign = if n.z >= 0.0 { 1.0f32 } else { -1.0f32 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (
        Vec3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        Vec3::new(b, sign + n.y * n.y * a, -n.y),
    )
}

/// A direction uniformly distributed over the hemisphere around `+Z`.
///
/// `u` must be a 2D sample in `[0, 1)²`.
#[must_use]
pub fn uniform_hemisphere(u: Vec2) -> Vec3 {
    let z = u.x;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = u.y * noxel_core::math::TAU;
    Vec3::new(r * phi.cos(), r * phi.sin(), z)
}

/// A direction cosine-weighted over the hemisphere around `+Z`.
///
/// The right distribution for ambient occlusion and diffuse indirect light: it
/// concentrates samples where the cosine falloff says they matter, which cuts
/// the variance of a diffuse integral dramatically.
#[must_use]
pub fn cosine_hemisphere(u: Vec2) -> Vec3 {
    let r = u.x.sqrt();
    let phi = u.y * noxel_core::math::TAU;
    let (s, c) = phi.sin_cos();
    Vec3::new(r * c, r * s, (1.0 - u.x).max(0.0).sqrt())
}

/// A direction uniformly distributed on the unit sphere.
#[must_use]
pub fn uniform_sphere(u: Vec2) -> Vec3 {
    let z = 1.0 - 2.0 * u.x;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = u.y * noxel_core::math::TAU;
    let (s, c) = phi.sin_cos();
    Vec3::new(r * c, r * s, z)
}

/// A direction in the spherical cap around `+Z` with half-angle
/// `acos(cos_max)`.
///
/// One sample of a soft shadow or a glossy reflection lobe.
#[must_use]
pub fn cap_hemisphere(u: Vec2, cos_max: f32) -> Vec3 {
    let cos_theta = 1.0 - u.x * (1.0 - cos_max.clamp(0.0, 1.0));
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let phi = u.y * noxel_core::math::TAU;
    let (s, c) = phi.sin_cos();
    Vec3::new(sin_theta * c, sin_theta * s, cos_theta)
}

/// A point uniformly distributed in the unit disc (concentric mapping).
#[must_use]
pub fn concentric_disk(u: Vec2) -> Vec2 {
    let a = 2.0 * u.x - 1.0;
    let b = 2.0 * u.y - 1.0;
    if a == 0.0 && b == 0.0 {
        return Vec2::ZERO;
    }
    let (r, phi) = if a * a > b * b {
        (a, core::f32::consts::FRAC_PI_4 * (b / a))
    } else {
        (
            b,
            core::f32::consts::FRAC_PI_2 - core::f32::consts::FRAC_PI_4 * (a / b),
        )
    };
    Vec2::new(r * phi.cos(), r * phi.sin())
}

/// Rotates a tangent-space direction into world space given a basis.
#[must_use]
pub fn tangent_to_world(dir: Vec3, tangent: Vec3, bitangent: Vec3, normal: Vec3) -> Vec3 {
    (tangent * dir.x + bitangent * dir.y + normal * dir.z).normalize_or_zero()
}

/// A per-pixel, per-sample stream of 2D samples.
///
/// The tracer builds one of these per pixel and pulls `next()` for each sample;
/// it is a pure function of the pixel coordinates and the frame index, so the
/// whole render stays reproducible.
#[derive(Clone, Debug)]
pub struct Sampler {
    rng: Pcg32,
    seed: u64,
    index: u32,
    total: u32,
}

impl Sampler {
    /// Creates a sampler for a pixel, for `total` samples this frame.
    #[must_use]
    pub fn for_pixel(x: u32, y: u32, frame: u64, total: u32) -> Self {
        let seed = hash_2d(x as i32, y as i32, frame);
        Self {
            rng: Pcg32::new(seed),
            seed,
            index: 0,
            total: total.max(1),
        }
    }

    /// The next 2D sample.
    pub fn next2(&mut self) -> Vec2 {
        let s = stratified(self.index, self.total, self.rng.next_u64());
        self.index += 1;
        s
    }

    /// The next scalar sample.
    pub fn next1(&mut self) -> f32 {
        self.next2().x
    }

    /// The index of the sample about to be produced.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Restarts the sequence, including the underlying RNG, so the samples
    /// after a reset are exactly the ones before it.
    pub fn reset(&mut self) {
        self.rng = Pcg32::new(self.seed);
        self.index = 0;
    }
}

/// The fraction of a hemisphere's solid angle a cone with the given cosine
/// covers — used to weight occlusion estimates.
#[must_use]
pub fn cone_solid_angle_fraction(cos_max: f32) -> f32 {
    1.0 - cos_max.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radical_inverse_is_low_discrepancy() {
        assert!((radical_inverse_vdc(0) - 0.0).abs() < 1e-6);
        assert!((radical_inverse_vdc(1) - 0.5).abs() < 1e-6);
        assert!((radical_inverse_vdc(2) - 0.25).abs() < 1e-6);
        assert!((radical_inverse_vdc(3) - 0.75).abs() < 1e-6);
        // Every value must be in [0, 1).
        for i in 0..1000 {
            let v = radical_inverse_vdc(i);
            assert!((0.0..1.0).contains(&v), "{i} -> {v}");
        }
    }

    #[test]
    fn hammersley_stays_in_the_unit_square() {
        for i in 0..256 {
            let p = hammersley(i, 256);
            assert!((0.0..1.0).contains(&p.x), "{p:?}");
            assert!((0.0..1.0).contains(&p.y), "{p:?}");
        }
    }

    #[test]
    fn hammersley_is_evenly_spread() {
        // 64 samples across 16 buckets of the y axis should be near-uniform.
        let n = 64;
        let mut buckets = [0u32; 16];
        for i in 0..n {
            let p = hammersley(i, n);
            buckets[(p.y * 16.0) as usize % 16] += 1;
        }
        assert!(buckets.iter().all(|b| *b >= 2 && *b <= 8), "{buckets:?}");
    }

    #[test]
    fn hammersley_handles_zero_n() {
        let p = hammersley(0, 0);
        assert!(p.x.is_finite() && p.y.is_finite());
    }

    #[test]
    fn stratified_is_deterministic_and_stratified() {
        let a = stratified(5, 64, 7);
        let b = stratified(5, 64, 7);
        assert_eq!(a, b);
        // Distinct strata for distinct indices in the first 64.
        let mut seen = std::collections::HashSet::new();
        for i in 0..64 {
            let p = stratified(i, 64, 1);
            assert!((0.0..1.0).contains(&p.x) && (0.0..1.0).contains(&p.y));
            seen.insert(((p.x * 8.0) as i32, (p.y * 8.0) as i32));
        }
        assert!(seen.len() >= 32, "poor stratification: {}", seen.len());
    }

    #[test]
    fn onb_is_orthonormal() {
        for dir in [
            Vec3::Y,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.3, -0.9, 0.2).normalize_or_zero(),
            Vec3::new(-1.0, -1.0, -1.0).normalize_or_zero(),
        ] {
            let (t, b) = build_onb(dir);
            assert!((t.length() - 1.0).abs() < 1e-4, "{dir:?} t={t:?}");
            assert!((b.length() - 1.0).abs() < 1e-4, "{dir:?} b={b:?}");
            assert!(t.dot(b).abs() < 1e-4, "{dir:?}");
            assert!(t.dot(dir).abs() < 1e-4, "{dir:?}");
            assert!(b.dot(dir).abs() < 1e-4, "{dir:?}");
            // Right-handed: t x b == n
            assert!((t.cross(b) - dir).length() < 1e-4, "{dir:?}");
        }
    }

    #[test]
    fn hemisphere_samples_point_outward_and_are_unit_length() {
        for i in 0..500 {
            let u = hammersley(i, 500);
            for d in [uniform_hemisphere(u), cosine_hemisphere(u)] {
                assert!((d.length() - 1.0).abs() < 1e-4, "{d:?}");
                assert!(d.z >= -1e-6, "hemisphere samples must have z >= 0: {d:?}");
            }
        }
    }

    #[test]
    fn cosine_hemisphere_is_cosine_weighted() {
        // The mean of z over cosine-weighted samples must be 2/3.
        let n = 20_000;
        let mut sum = 0.0;
        for i in 0..n {
            sum += cosine_hemisphere(hammersley(i, n)).z;
        }
        let mean = sum / n as f32;
        assert!((mean - 2.0 / 3.0).abs() < 0.01, "mean z = {mean}");
    }

    #[test]
    fn uniform_hemisphere_is_uniform_in_z() {
        let n = 20_000;
        let mut sum = 0.0;
        for i in 0..n {
            sum += uniform_hemisphere(hammersley(i, n)).z;
        }
        let mean = sum / n as f32;
        assert!((mean - 0.5).abs() < 0.01, "mean z = {mean}");
    }

    #[test]
    fn uniform_sphere_covers_both_hemispheres() {
        let n = 4000;
        let mut above = 0;
        for i in 0..n {
            let d = uniform_sphere(hammersley(i, n));
            assert!((d.length() - 1.0).abs() < 1e-4);
            if d.z > 0.0 {
                above += 1;
            }
        }
        assert!(
            (above as f32 / n as f32 - 0.5).abs() < 0.05,
            "{}",
            above as f32 / n as f32
        );
    }

    #[test]
    fn cap_hemisphere_respects_the_angle() {
        let cos_max = 0.5f32; // 60 degrees
        for i in 0..1000 {
            let d = cap_hemisphere(hammersley(i, 1000), cos_max);
            assert!(d.z >= cos_max - 1e-4, "z = {} < {cos_max}", d.z);
            assert!((d.length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn cap_hemisphere_half_sphere_matches_uniform() {
        let n = 5000;
        let mut sum = 0.0;
        for i in 0..n {
            sum += cap_hemisphere(hammersley(i, n), 0.0).z;
        }
        let mean = sum / n as f32;
        assert!((mean - 0.5).abs() < 0.02, "mean z = {mean}");
    }

    #[test]
    fn concentric_disk_is_inside_and_uniform_in_area() {
        let n = 10_000;
        let mut inner = 0;
        for i in 0..n {
            let p = concentric_disk(hammersley(i, n));
            assert!(p.length() <= 1.0 + 1e-5, "{p:?}");
            if p.length() < 0.5 {
                inner += 1;
            }
        }
        // A quarter of the disc's area is inside r = 0.5.
        let ratio = inner as f32 / n as f32;
        assert!((ratio - 0.25).abs() < 0.02, "ratio {ratio}");
    }

    #[test]
    fn concentric_disk_centre_case() {
        assert_eq!(concentric_disk(Vec2::new(0.5, 0.5)), Vec2::ZERO);
    }

    #[test]
    fn tangent_to_world_round_trips() {
        let n = Vec3::new(0.2, 0.9, -0.3).normalize_or_zero();
        let (t, b) = build_onb(n);
        let local = Vec3::new(0.3, 0.4, 0.866).normalize_or_zero();
        let world = tangent_to_world(local, t, b, n);
        assert!((world.length() - 1.0).abs() < 1e-4);
        // Projecting back must recover the local coordinates.
        assert!((world.dot(n) - local.z).abs() < 1e-4);
        assert!((world.dot(t) - local.x).abs() < 1e-4);
        assert!((world.dot(b) - local.y).abs() < 1e-4);
    }

    #[test]
    fn sampler_is_deterministic_per_pixel() {
        let mut a = Sampler::for_pixel(10, 20, 0, 16);
        let mut b = Sampler::for_pixel(10, 20, 0, 16);
        for _ in 0..16 {
            assert_eq!(a.next2(), b.next2());
        }
        let mut c = Sampler::for_pixel(11, 20, 0, 16);
        assert_ne!(Sampler::for_pixel(10, 20, 0, 16).next2(), c.next2());
    }

    #[test]
    fn sampler_advances_and_resets() {
        let mut s = Sampler::for_pixel(0, 0, 0, 8);
        assert_eq!(s.index(), 0);
        let _ = s.next1();
        assert_eq!(s.index(), 1);
        s.reset();
        assert_eq!(s.index(), 0);
        assert_eq!(Sampler::for_pixel(0, 0, 0, 8).next1(), s.next1());
    }

    #[test]
    fn sampler_output_is_in_range() {
        let mut s = Sampler::for_pixel(3, 7, 2, 64);
        for _ in 0..128 {
            let v = s.next2();
            assert!(
                (0.0..1.0).contains(&v.x) && (0.0..1.0).contains(&v.y),
                "{v:?}"
            );
        }
    }

    #[test]
    fn frame_index_changes_the_samples() {
        let a = Sampler::for_pixel(1, 1, 0, 16).next2();
        let b = Sampler::for_pixel(1, 1, 1, 16).next2();
        assert_ne!(a, b, "temporal accumulation needs per-frame variation");
    }

    #[test]
    fn cone_solid_angle_fraction_bounds() {
        assert!((cone_solid_angle_fraction(1.0) - 0.0).abs() < 1e-6);
        assert!((cone_solid_angle_fraction(0.0) - 1.0).abs() < 1e-6);
        assert!((cone_solid_angle_fraction(-5.0) - 1.0).abs() < 1e-6);
    }
}
