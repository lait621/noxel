//! Deterministic coherent noise.
//!
//! Every generator here is **hash-based**: the value at `(x, y)` is a pure
//! function of the coordinates and the seed. There is no permutation table to
//! initialise, no state to keep, and — crucially for a streaming world — asking
//! for a single sample at a far-away coordinate costs the same as asking for one
//! next to the origin. That is what lets Noxel generate chunk `(900, -400)`
//! without generating the 2 million chunks before it.
//!
//! # Which function to use
//!
//! | Function | Look | Typical use |
//! |---|---|---|
//! | [`value_2d`] | blocky, hard edges | tile-grid variation, cheap masks |
//! | [`perlin_2d`] | smooth, round blobs | terrain height, biome moisture |
//! | [`simplex_2d`] | smooth, fewer axis artefacts | terrain where grid bias shows |
//! | [`worley_2d`] | cellular, cracked | stone/plaster texture, region borders |
//! | [`fbm_2d`] | layered detail | the main terrain signal |
//! | [`ridged_2d`] | sharp ridges | mountain ranges, cliff lines |
//!
//! All of them return values in `[-1, 1]` except [`worley_2d`], which returns
//! the distance to the nearest feature point (normalised to roughly `[0, 1]`),
//! and the `*_01` helpers, which map to `[0, 1]`.

use super::vec::Vec2;
use crate::rng::{hash_2d, hash_3d};

/// Hashes a 2D lattice point into `[0, 1)`.
#[inline]
#[must_use]
pub fn hash01_2d(x: i32, y: i32, seed: u64) -> f32 {
    // Use the high 24 bits: the low bits of a hash are the weakest.
    ((hash_2d(x, y, seed) >> 40) as f32) * (1.0 / (1u32 << 24) as f32)
}

/// Hashes a 3D lattice point into `[0, 1)`.
#[inline]
#[must_use]
pub fn hash01_3d(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    ((hash_3d(x, y, z, seed) >> 40) as f32) * (1.0 / (1u32 << 24) as f32)
}

/// Hashes a 2D lattice point into `[-1, 1)`.
#[inline]
#[must_use]
pub fn hash_signed_2d(x: i32, y: i32, seed: u64) -> f32 {
    hash01_2d(x, y, seed) * 2.0 - 1.0
}

/// Blocky value noise in `[-1, 1]` with smoothstep interpolation.
#[must_use]
pub fn value_2d(x: f32, y: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (ix, iy) = (x0 as i32, y0 as i32);
    let v00 = hash_signed_2d(ix, iy, seed);
    let v10 = hash_signed_2d(ix + 1, iy, seed);
    let v01 = hash_signed_2d(ix, iy + 1, seed);
    let v11 = hash_signed_2d(ix + 1, iy + 1, seed);
    let sx = super::scalar::smoothstep(0.0, 1.0, fx);
    let sy = super::scalar::smoothstep(0.0, 1.0, fy);
    let a = v00 + (v10 - v00) * sx;
    let b = v01 + (v11 - v01) * sx;
    a + (b - a) * sy
}

/// Gradient (Perlin) noise in roughly `[-1, 1]`.
///
/// Smoother and less blocky than [`value_2d`]; the classic choice for terrain
/// and for cloud/foliage masks.
#[must_use]
pub fn perlin_2d(x: f32, y: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let (ix, iy) = (x0 as i32, y0 as i32);
    let fx = x - x0;
    let fy = y - y0;

    let d00 = gradient_dot(ix, iy, fx, fy, seed);
    let d10 = gradient_dot(ix + 1, iy, fx - 1.0, fy, seed);
    let d01 = gradient_dot(ix, iy + 1, fx, fy - 1.0, seed);
    let d11 = gradient_dot(ix + 1, iy + 1, fx - 1.0, fy - 1.0, seed);

    let u = smoother(fx);
    let v = smoother(fy);
    let a = d00 + (d10 - d00) * u;
    let b = d01 + (d11 - d01) * u;
    // Perlin's theoretical range for 2D gradients is about ±0.707; scale it so
    // callers get a genuine [-1, 1].
    (a + (b - a) * v) * 1.414_213_6
}

/// 8 evenly spaced gradient directions; picking with a hash avoids a table.
#[inline]
fn gradient_dot(ix: i32, iy: i32, dx: f32, dy: f32, seed: u64) -> f32 {
    let h = hash_2d(ix, iy, seed) >> 61; // 3 bits -> 8 directions
    // Angles at 45 degree steps, precomputed as (cos, sin).
    const G: [(f32, f32); 8] = [
        (1.0, 0.0),
        (0.707_106_77, 0.707_106_77),
        (0.0, 1.0),
        (-0.707_106_77, 0.707_106_77),
        (-1.0, 0.0),
        (-0.707_106_77, -0.707_106_77),
        (0.0, -1.0),
        (0.707_106_77, -0.707_106_77),
    ];
    let (gx, gy) = G[(h as usize) & 7];
    gx * dx + gy * dy
}

/// Perlin's quintic smoothing curve, with zero first and second derivatives at
/// the ends — this is what removes the visible lattice creases.
#[inline]
fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Simplex noise in roughly `[-1, 1]`.
///
/// Better isotropy than Perlin (no visible axis creases) at a slightly higher
/// cost. Use it when a Perlin grid bias would be visible in the final art.
#[must_use]
pub fn simplex_2d(x: f32, y: f32, seed: u64) -> f32 {
    const F2: f32 = 0.366_025_42; // (sqrt(3) - 1) / 2
    const G2: f32 = 0.211_324_87; // (3 - sqrt(3)) / 6

    let s = (x + y) * F2;
    let i = (x + s).floor() as i32;
    let j = (y + s).floor() as i32;
    let t = (i + j) as f32 * G2;
    let x0 = x - (i as f32 - t);
    let y0 = y - (j as f32 - t);

    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let x1 = x0 - i1 as f32 + G2;
    let y1 = y0 - j1 as f32 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;

    let mut n = 0.0;
    n += corner_contribution(i, j, x0, y0, seed);
    n += corner_contribution(i + i1, j + j1, x1, y1, seed);
    n += corner_contribution(i + 1, j + 1, x2, y2, seed);
    // Empirically normalises the sum into about [-1, 1].
    (n * 70.0).clamp(-1.0, 1.0)
}

#[inline]
fn corner_contribution(ix: i32, iy: i32, dx: f32, dy: f32, seed: u64) -> f32 {
    let t = 0.5 - dx * dx - dy * dy;
    if t <= 0.0 {
        return 0.0;
    }
    let t2 = t * t;
    t2 * t2 * gradient_dot(ix, iy, dx, dy, seed)
}

/// Worley (cellular) noise: `(distance to nearest feature point, feature id)`.
///
/// The distance is normalised so that a typical value is around `0.5`. The id
/// is stable for the cell the point falls in and is what you use to give each
/// "cell" of a stone texture its own tint.
#[must_use]
pub fn worley_2d(x: f32, y: f32, seed: u64) -> (f32, u32) {
    let cx = x.floor() as i32;
    let cy = y.floor() as i32;
    let mut best = f32::INFINITY;
    let mut best_id = 0u32;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let gx = cx + ox;
            let gy = cy + oy;
            let h = hash_2d(gx, gy, seed);
            let jx = ((h >> 8) & 0xFFFF) as f32 * (1.0 / 65536.0);
            let jy = ((h >> 24) & 0xFFFF) as f32 * (1.0 / 65536.0);
            let jz = ((h >> 40) & 0xFFFF) as f32 * (1.0 / 65536.0);
            let px = gx as f32 + jx;
            let py = gy as f32 + jy;
            let d = (px - x) * (px - x) + (py - y) * (py - y);
            if d < best {
                best = d;
                best_id = hash_2d(gx, gy, seed ^ 0x9E37_79B9) as u32;
            }
            let _ = jz;
        }
    }
    (best.sqrt().min(1.0), best_id)
}

/// Fractal Brownian motion: `octaves` layers of [`perlin_2d`].
///
/// `lacunarity` scales the frequency between octaves (2.0 is standard) and
/// `gain` scales the amplitude (0.5 is standard). The result is normalised to
/// `[-1, 1]` regardless of the octave count, so changing `octaves` changes the
/// detail without changing the overall range.
#[must_use]
pub fn fbm_2d(x: f32, y: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..octaves.max(1) {
        sum += perlin_2d(
            x * freq,
            y * freq,
            seed.wrapping_add(o as u64 * 0x9E37_79B9),
        ) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    if norm > 0.0 {
        (sum / norm).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Like [`fbm_2d`] but using [`simplex_2d`] for each octave.
#[must_use]
pub fn fbm_simplex_2d(x: f32, y: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..octaves.max(1) {
        sum += simplex_2d(
            x * freq,
            y * freq,
            seed.wrapping_add(o as u64 * 0x9E37_79B9),
        ) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    if norm > 0.0 {
        (sum / norm).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Ridged multifractal: sharp crests, good for mountain ranges and cliff lines.
///
/// Returns `[0, 1]` with 1 at a ridge.
#[must_use]
pub fn ridged_2d(x: f32, y: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..octaves.max(1) {
        let n = 1.0
            - perlin_2d(
                x * freq,
                y * freq,
                seed.wrapping_add(o as u64 * 0x85EB_CA6B),
            )
            .abs();
        sum += n * n * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    if norm > 0.0 {
        (sum / norm).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Domain-warped fbm: samples `fbm` at coordinates displaced by another `fbm`.
///
/// This is what turns a generic noise field into terrain with rivers, bays and
/// believable coastlines. `warp_strength` is in noise units; 0.5–2.0 works well.
#[must_use]
pub fn warped_fbm_2d(x: f32, y: f32, seed: u64, octaves: u32, warp_strength: f32) -> f32 {
    let wx = fbm_2d(x + 5.2, y + 1.3, seed ^ 0x1234, 3, 2.0, 0.5);
    let wy = fbm_2d(x + 9.7, y + 4.8, seed ^ 0x5678, 3, 2.0, 0.5);
    fbm_2d(
        x + wx * warp_strength,
        y + wy * warp_strength,
        seed,
        octaves,
        2.0,
        0.5,
    )
}

/// Value noise in `[-1, 1]` on a 3D lattice, for cave systems and ore veins.
#[must_use]
pub fn value_3d(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let z0 = z.floor();
    let (ix, iy, iz) = (x0 as i32, y0 as i32, z0 as i32);
    let (fx, fy, fz) = (x - x0, y - y0, z - z0);
    let (sx, sy, sz) = (smoother(fx), smoother(fy), smoother(fz));

    let mut acc = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let w = (if dx == 0 { 1.0 - sx } else { sx })
                    * (if dy == 0 { 1.0 - sy } else { sy })
                    * (if dz == 0 { 1.0 - sz } else { sz });
                let v = hash01_3d(ix + dx, iy + dy, iz + dz, seed) * 2.0 - 1.0;
                acc += w * v;
            }
        }
    }
    acc
}

/// A **tileable** value-noise sample.
///
/// Wraps the lattice into a `period × period` torus so a texture generated by
/// tiling this function has no seam. Used by `noxel-gen` to emit seamlessly
/// repeating ground textures.
#[must_use]
pub fn tileable_value_2d(x: f32, y: f32, period: i32, seed: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (fx, fy) = (x - x0, y - y0);
    let wrap = |v: i32| v.rem_euclid(period.max(1));
    let v00 = hash_signed_2d(wrap(ix), wrap(iy), seed);
    let v10 = hash_signed_2d(wrap(ix + 1), wrap(iy), seed);
    let v01 = hash_signed_2d(wrap(ix), wrap(iy + 1), seed);
    let v11 = hash_signed_2d(wrap(ix + 1), wrap(iy + 1), seed);
    let sx = smoother(fx);
    let sy = smoother(fy);
    let a = v00 + (v10 - v00) * sx;
    let b = v01 + (v11 - v01) * sx;
    a + (b - a) * sy
}

/// The smallest distance from `p` to the segment `a`–`b`, in 2D.
///
/// Used by the road generator to snap props away from a road centreline.
#[must_use]
pub fn distance_to_segment_2d(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-9 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_in_range() {
        for i in -20..20 {
            for j in -20..20 {
                let v = hash01_2d(i, j, 7);
                assert!((0.0..1.0).contains(&v), "{v}");
                let s = hash_signed_2d(i, j, 7);
                assert!((-1.0..1.0).contains(&s), "{s}");
            }
        }
    }

    #[test]
    fn noise_is_deterministic() {
        for (x, y) in [(0.0, 0.0), (1.5, -2.25), (1000.0, 1000.0)] {
            assert_eq!(perlin_2d(x, y, 3), perlin_2d(x, y, 3));
            assert_eq!(value_2d(x, y, 3), value_2d(x, y, 3));
            assert_eq!(simplex_2d(x, y, 3), simplex_2d(x, y, 3));
        }
    }

    #[test]
    fn seeds_change_the_field() {
        let a = perlin_2d(1.5, 2.5, 1);
        let b = perlin_2d(1.5, 2.5, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn lattice_points_are_zero_for_perlin() {
        // Gradient noise is exactly zero on the integer lattice.
        for i in -3..3 {
            for j in -3..3 {
                assert!(perlin_2d(i as f32, j as f32, 11).abs() < 1e-5, "{i},{j}");
            }
        }
    }

    #[test]
    fn noise_is_continuous() {
        // A tiny step must produce a tiny change: no popping between cells.
        let mut prev = perlin_2d(0.0, 0.3, 5);
        for i in 1..500 {
            let x = i as f32 * 0.01;
            let v = perlin_2d(x, 0.3, 5);
            assert!(
                (v - prev).abs() < 0.2,
                "discontinuity at x={x}: {prev} -> {v}"
            );
            prev = v;
        }
    }

    #[test]
    fn perlin_stays_in_range() {
        for i in 0..2000 {
            let x = i as f32 * 0.37;
            let y = (i as f32 * 1.13).sin() * 50.0;
            let v = perlin_2d(x, y, 9);
            assert!((-1.05..=1.05).contains(&v), "{v}");
        }
    }

    #[test]
    fn simplex_stays_in_range() {
        for i in 0..2000 {
            let x = i as f32 * 0.21 - 200.0;
            let y = i as f32 * -0.09 + 100.0;
            let v = simplex_2d(x, y, 4);
            assert!((-1.001..=1.001).contains(&v), "{v}");
        }
    }

    #[test]
    fn fbm_is_normalised_regardless_of_octaves() {
        for octaves in [1u32, 2, 4, 8] {
            let mut hi: f32 = 0.0;
            for i in 0..2000 {
                let v = fbm_2d(
                    i as f32 * 0.05,
                    (i as f32 * 0.013).cos() * 20.0,
                    2,
                    octaves,
                    2.0,
                    0.5,
                );
                hi = hi.max(v.abs());
            }
            assert!(hi <= 1.001, "octaves {octaves} produced {hi}");
            assert!(hi > 0.1, "octaves {octaves} produced a flat field ({hi})");
        }
    }

    #[test]
    fn fbm_has_more_detail_than_a_single_octave() {
        // Roughness metric: mean absolute difference between neighbouring samples.
        let rough = |octaves: u32| {
            let mut acc = 0.0;
            let n = 500;
            for i in 0..n {
                let x = i as f32 * 0.02;
                acc += (fbm_2d(x + 0.02, 0.5, 1, octaves, 2.0, 0.5)
                    - fbm_2d(x, 0.5, 1, octaves, 2.0, 0.5))
                .abs();
            }
            acc / n as f32
        };
        assert!(rough(6) > rough(1), "more octaves must add detail");
    }

    #[test]
    fn ridged_is_in_unit_range() {
        for i in 0..1000 {
            let v = ridged_2d(i as f32 * 0.07, i as f32 * 0.031, 6, 5, 2.0, 0.5);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn worley_distance_and_id() {
        for i in 0..500 {
            let (d, _id) = worley_2d(i as f32 * 0.13, i as f32 * 0.77, 3);
            assert!((0.0..=1.0).contains(&d), "{d}");
        }
        // The same sample must give the same feature id every time, and
        // different seeds must reshuffle the cells.
        assert_eq!(worley_2d(0.2, 0.2, 3).1, worley_2d(0.2, 0.2, 3).1);
        assert_ne!(worley_2d(0.2, 0.2, 3).1, worley_2d(0.2, 0.2, 4).1);
        // Moving the sample within one cell keeps the same nearest feature most
        // of the time; this is what makes it usable as a cell id.
        let (_, id) = worley_2d(0.5, 0.5, 3);
        let same = (0..16)
            .filter(|i| {
                let t = *i as f32 / 16.0 * 0.9 + 0.05;
                worley_2d(t, t, 3).1 == id
            })
            .count();
        assert!(same >= 8, "cell id should be locally stable, got {same}/16");
    }

    #[test]
    fn warped_fbm_differs_but_stays_bounded() {
        let plain = fbm_2d(12.0, 7.0, 1, 4, 2.0, 0.5);
        let warped = warped_fbm_2d(12.0, 7.0, 1, 4, 1.0);
        assert!(
            (warped - plain).abs() > 1e-4,
            "warping must change the field"
        );
        assert!(warped.abs() <= 1.001);
    }

    #[test]
    fn value_3d_is_bounded_and_continuous() {
        let mut prev = value_3d(0.0, 0.0, 0.0, 2);
        for i in 1..300 {
            let v = value_3d(i as f32 * 0.05, 0.5, 0.25, 2);
            assert!((-1.001..=1.001).contains(&v));
            assert!((v - prev).abs() < 0.35, "discontinuity: {prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn tileable_noise_wraps_seamlessly() {
        let period = 8;
        for i in 0..40 {
            let t = i as f32 * 0.2;
            let a = tileable_value_2d(t, 3.0, period, 1);
            let b = tileable_value_2d(t + period as f32, 3.0, period, 1);
            assert!((a - b).abs() < 1e-4, "{a} vs {b} at t={t}");
        }
    }

    #[test]
    fn distance_to_segment_matches_expectations() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        assert!((distance_to_segment_2d(Vec2::new(5.0, 3.0), a, b) - 3.0).abs() < 1e-5);
        assert!((distance_to_segment_2d(Vec2::new(-4.0, 0.0), a, b) - 4.0).abs() < 1e-5);
        assert!((distance_to_segment_2d(Vec2::new(14.0, 0.0), a, b) - 4.0).abs() < 1e-5);
        // Degenerate segment.
        assert!((distance_to_segment_2d(Vec2::new(3.0, 4.0), a, a) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn distant_samples_do_not_require_neighbours() {
        // The whole point of hash-based noise: a sample 10^6 away is as cheap as
        // one at the origin, and both are stable.
        let v1 = perlin_2d(1_000_000.5, -1_000_000.25, 42);
        let v2 = perlin_2d(1_000_000.5, -1_000_000.25, 42);
        assert_eq!(v1, v2);
        assert!(v1.is_finite());
    }
}
