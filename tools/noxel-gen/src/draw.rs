//! Small drawing primitives shared by every art routine.
//!
//! These exist so the tile functions read like drawing instructions — "bevel
//! the edges, scatter four blades of grass, outline the silhouette" — instead
//! of like index arithmetic. Nothing here panics: out-of-bounds coordinates are
//! clipped, exactly as [`noxel_asset::image::Image`] does.
//!
//! The only source of randomness is a **hash of the pixel coordinate** (see
//! [`hash01`]), never a shared mutable generator, so a routine can be evaluated
//! one pixel at a time and still be reproducible.

use noxel_asset::image::Image;
use noxel_core::math::Color8;
use noxel_core::rng::{hash_2d, hash_u64};

/// A deterministic hash of `(x, y, seed)` in `[0, 1)`.
#[must_use]
pub fn hash01(x: i32, y: i32, seed: u64) -> f32 {
    ((hash_2d(x, y, seed) >> 40) as f32) * (1.0 / (1u32 << 24) as f32)
}

/// A deterministic hash of `(x, y, seed)`.
#[must_use]
pub fn hash(x: i32, y: i32, seed: u64) -> u64 {
    hash_2d(x, y, seed)
}

/// True for `percent` percent of pixels, chosen by hash.
#[must_use]
pub fn chance(x: i32, y: i32, seed: u64, percent: u64) -> bool {
    hash(x, y, seed) % 100 < percent.min(100)
}

/// True when the pixel at `(x, y)` is fully opaque.
#[must_use]
pub fn opaque(img: &Image, x: u32, y: u32) -> bool {
    img.get_or_transparent(x, y).a != 0
}

/// Writes a pixel, clipped to the image.
pub fn put(img: &mut Image, x: i32, y: i32, color: Color8) {
    if x < 0 || y < 0 {
        return;
    }
    img.set(x as u32, y as u32, color);
}

/// Erases a pixel, clipped to the image.
pub fn clear(img: &mut Image, x: i32, y: i32) {
    put(img, x, y, Color8::TRANSPARENT);
}

/// Draws an inclusive horizontal run, clipped to the image.
pub fn hline(img: &mut Image, x0: i32, x1: i32, y: i32, color: Color8) {
    let (from, to) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    for x in from..=to {
        put(img, x, y, color);
    }
}

/// Draws an inclusive vertical run, clipped to the image.
pub fn vline(img: &mut Image, x: i32, y0: i32, y1: i32, color: Color8) {
    let (from, to) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    for y in from..=to {
        put(img, x, y, color);
    }
}

/// An inclusive pixel rectangle: the region a routine works over.
///
/// Taking a rectangle as one value keeps a call like
/// `scatter(&mut tile, TILE, FOAM, 5, seed)` readable, and makes the common
/// "the whole 16x16 tile" case a named constant instead of four magic numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    /// Left edge.
    pub x0: i32,
    /// Top edge.
    pub y0: i32,
    /// Right edge, inclusive.
    pub x1: i32,
    /// Bottom edge, inclusive.
    pub y1: i32,
}

impl Area {
    /// A rectangle from its inclusive corners.
    #[must_use]
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
}

/// The area of one 16x16 tile, the size nearly every ground and building tile
/// is drawn at.
pub const TILE: Area = Area::new(0, 0, 15, 15);

/// Fills an inclusive rectangle, clipped to the image.
pub fn fill(img: &mut Image, area: Area, color: Color8) {
    let (x0, x1) = ordered(area.x0, area.x1);
    let (y0, y1) = ordered(area.y0, area.y1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            put(img, x, y, color);
        }
    }
}

/// Draws an inclusive one-pixel rectangle outline, clipped to the image.
pub fn frame(img: &mut Image, area: Area, color: Color8) {
    hline(img, area.x0, area.x1, area.y0, color);
    hline(img, area.x0, area.x1, area.y1, color);
    vline(img, area.x0, area.y0, area.y1, color);
    vline(img, area.x1, area.y0, area.y1, color);
}

/// Scatters `color` across an area: `percent` percent of the pixels receive it,
/// chosen by hash.
pub fn scatter(img: &mut Image, area: Area, color: Color8, percent: u64, seed: u64) {
    for y in area.y0..=area.y1 {
        for x in area.x0..=area.x1 {
            if chance(x, y, seed, percent) {
                put(img, x, y, color);
            }
        }
    }
}

/// The two ends of a span, smaller first.
fn ordered(a: i32, b: i32) -> (i32, i32) {
    if a <= b { (a, b) } else { (b, a) }
}

/// A 2x2 checkerboard dither blended with a hash, so it does not read as a
/// perfect grid: two out of every three checker pixels are taken.
pub fn dither(img: &mut Image, area: Area, color: Color8, seed: u64) {
    for y in area.y0..=area.y1 {
        for x in area.x0..=area.x1 {
            if (x + y).rem_euclid(2) == 0 && hash01(x, y, seed) < 0.7 {
                put(img, x, y, color);
            }
        }
    }
}

/// Draws a vertical grass blade: a darker base pixel with a lighter tip.
pub fn blade(img: &mut Image, x: i32, y: i32, height: i32, base: Color8, tip: Color8) {
    for step in 0..height.max(1) {
        let c = if step == height - 1 { tip } else { base };
        put(img, x, y - step, c);
    }
}

/// Applies the house lighting convention: the top edge catches the light and
/// the bottom edge falls into shadow, with the left and right columns shaded
/// for the half of the tile facing away from the light.
///
/// Standalone tiles have no neighbours to shade against, so every material
/// approximates the same fixed top-light/bottom-shadow sun.
pub fn bevel(img: &mut Image, light: Color8, shade: Color8) {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return;
    }
    for x in 0..w {
        if x % 3 != 2 && opaque(img, x, 0) {
            img.set(x, 0, light);
        }
        if x % 3 != 1 && opaque(img, x, h - 1) {
            img.set(x, h - 1, shade);
        }
    }
    for y in 0..h {
        if y < h / 2 && y % 2 == 0 && opaque(img, 0, y) {
            img.set(0, y, light);
        }
        if y >= h / 2 && y % 2 == 1 && opaque(img, w - 1, y) {
            img.set(w - 1, y, shade);
        }
    }
}

/// Draws a dark outline on the transparent pixels touching the sprite.
///
/// A strong silhouette is what makes a 16-pixel prop readable on a busy ground
/// tile, so every prop and character is outlined before it is placed.
pub fn outline(img: &mut Image, color: Color8) {
    let (w, h) = (img.width(), img.height());
    let mut marks: Vec<(u32, u32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if opaque(img, x, y) {
                continue;
            }
            let touching = (-1i64..=1).any(|dy| {
                (-1i64..=1).any(|dx| {
                    (dx != 0 || dy != 0)
                        && img
                            .get_at(i64::from(x) + dx, i64::from(y) + dy)
                            .is_some_and(|pixel| pixel.a != 0)
                })
            });
            if touching {
                marks.push((x, y));
            }
        }
    }
    for (x, y) in marks {
        img.set(x, y, color);
    }
}

/// A stable hash of a string, used to seed per-material routines.
#[must_use]
pub fn seed_of(label: &str) -> u64 {
    hash_u64(noxel_core::rng::hash_str(label))
}

/// True when `img` has no fully transparent pixel: a ground tile.
#[must_use]
#[cfg(test)]
pub fn is_opaque(img: &Image) -> bool {
    img.pixels.iter().all(|pixel| pixel.a != 0)
}

/// The number of distinct colours in `img`.
#[must_use]
#[cfg(test)]
pub fn distinct_colors(img: &Image) -> usize {
    img.unique_colors().len()
}
