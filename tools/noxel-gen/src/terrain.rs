//! The terrain atlas: sixteen 16x16 ground tiles.
//!
//! Two of the tiles are special. `water_shallow`/`water_deep` and `road` are
//! drawn from **pure periodic functions** ([`water_pixel`], [`road_pixel`])
//! instead of from a one-shot routine over a 16x16 buffer. A world draws its
//! ground as a grid of these tiles, so a seam is a visible grid line across the
//! whole map; expressing the pattern as `f(x, y)` with period 16 makes wrapping
//! a property of the function rather than a hope, and lets a test prove it by
//! sampling `f` on both sides of the seam.

use noxel_asset::image::Image;
use noxel_core::math::{Color8, tileable_value_2d};

use crate::draw::{self, Area};
use crate::palette::*;

/// Edge length of one terrain tile, in pixels.
pub const SIZE: u32 = 16;

/// The tiles, in atlas order. Index == atlas column.
pub const NAMES: [&str; 16] = [
    "grass",
    "grass_dark",
    "grass_flowers",
    "dirt",
    "dirt_dark",
    "sand",
    "rock",
    "rock_dark",
    "cliff",
    "water_shallow",
    "water_deep",
    "road",
    "road_edge",
    "plaza",
    "bridge",
    "snow",
];

/// Seed for the water field. Constant: water looks the same in every run.
const WATER_SEED: u64 = 0x77_4E_0A_1D;
/// Seed for the cobble field.
const ROAD_SEED: u64 = 0x52_0D_11_CE;
/// Seed for the plaza slabs.
const PLAZA_SEED: u64 = 0x9A_2B_C0_11;
/// Seed for the bridge planks.
const BRIDGE_SEED: u64 = 0x8D_16_3F_02;

/// The atlas column of a tile, or `None` when the name is unknown.
#[must_use]
pub fn index_of(name: &str) -> Option<usize> {
    NAMES.iter().position(|candidate| *candidate == name)
}

/// The atlas image: every tile side by side, `16 * SIZE` wide and `SIZE` tall.
#[must_use]
pub fn atlas() -> Image {
    let mut sheet = Image::new(SIZE * NAMES.len() as u32, SIZE, SHADOW);
    for (column, name) in NAMES.iter().enumerate() {
        if let Some(tile) = tile(name) {
            sheet.blit(&tile, (column as u32 * SIZE) as i32, 0);
        }
    }
    sheet
}

/// One terrain tile by name.
#[must_use]
pub fn tile(name: &str) -> Option<Image> {
    let image = match name {
        "grass" => grass(GRASS_MID, GRASS_DARK, GRASS_LIGHT, draw::seed_of("grass")),
        "grass_dark" => grass(
            GRASS_DARK,
            LEAF_DARK,
            GRASS_MID,
            draw::seed_of("grass_dark"),
        ),
        "grass_flowers" => grass_flowers(),
        "dirt" => dirt(DIRT_MID, DIRT_DARK, DIRT_LIGHT, draw::seed_of("dirt")),
        "dirt_dark" => dirt(DIRT_DARK, SHADOW, DIRT_MID, draw::seed_of("dirt_dark")),
        "sand" => sand(),
        "rock" => rock(STONE_MID, STONE_LIGHT, STONE_DARK, draw::seed_of("rock")),
        "rock_dark" => rock(STONE_DARK, STONE_MID, SHADOW, draw::seed_of("rock_dark")),
        "cliff" => cliff(),
        "water_shallow" => water(false),
        "water_deep" => water(true),
        "road" => road(),
        "road_edge" => road_edge(),
        "plaza" => plaza(),
        "bridge" => bridge(),
        "snow" => snow(),
        _ => return None,
    };
    Some(image)
}

// --- ground ----------------------------------------------------------------

/// A grass field: a dithered shadow tone with scattered two-pixel blades.
fn grass(base: Color8, shade: Color8, blade_tip: Color8, seed: u64) -> Image {
    let mut img = Image::new(SIZE, SIZE, base);
    draw::dither(&mut img, draw::TILE, shade, seed);

    // Six blades, each a dark stalk with a lit tip, placed by hash.
    for i in 0..6 {
        let x = 1 + (draw::hash01(i, 0, seed) * 13.0) as i32;
        let y = 3 + (draw::hash01(i, 1, seed) * 11.0) as i32;
        draw::blade(&mut img, x, y, 2, shade, blade_tip);
        draw::put(&mut img, x + 1, y, base);
    }
    draw::bevel(&mut img, blade_tip, shade);
    img
}

/// Grass with three small flowers: four petals around a lit centre.
fn grass_flowers() -> Image {
    let seed = draw::seed_of("grass_flowers");
    let mut img = grass(GRASS_MID, GRASS_DARK, GRASS_LIGHT, seed);
    let spots = [(3, 4), (10, 7), (6, 12)];
    for (i, (x, y)) in spots.iter().enumerate() {
        let petal = if i == 1 { FOAM } else { ROOF_RED };
        let light = if i == 1 { PLASTER_MID } else { ROOF_LIGHT };
        draw::put(&mut img, *x, y - 1, petal);
        draw::put(&mut img, *x - 1, *y, petal);
        draw::put(&mut img, *x + 1, *y, light);
        draw::put(&mut img, *x, y + 1, petal);
        draw::put(&mut img, *x + 1, y - 1, light);
        draw::put(&mut img, *x, *y, SAND_LIGHT);
        draw::put(&mut img, *x + 1, y + 1, GRASS_DARK);
    }
    img
}

/// Bare earth: a dithered base with a few lit pebbles and their shadows.
fn dirt(base: Color8, shade: Color8, pebble: Color8, seed: u64) -> Image {
    let mut img = Image::new(SIZE, SIZE, base);
    draw::dither(&mut img, draw::TILE, shade, seed);
    for i in 0..5 {
        let x = 2 + (draw::hash01(i, 7, seed) * 11.0) as i32;
        let y = 2 + (draw::hash01(i, 9, seed) * 11.0) as i32;
        draw::put(&mut img, x, y, pebble);
        draw::put(&mut img, x + 1, y, pebble);
        draw::put(&mut img, x, y + 1, shade);
        draw::put(&mut img, x + 1, y + 1, shade);
    }
    draw::bevel(&mut img, pebble, shade);
    img
}

/// Wind-blown sand: long dunes with a lit crest and a few dark specks.
fn sand() -> Image {
    let seed = draw::seed_of("sand");
    let mut img = Image::new(SIZE, SIZE, SAND_LIGHT);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dune = tileable_value_2d(x as f32 * 0.5, y as f32 * 0.5, 8, seed);
            let ripple = tileable_value_2d(x as f32 * 0.25, y as f32 * 0.5, 4, seed ^ 0x33);
            if dune > 0.55 {
                img.set(x, y, PLASTER_DARK);
            }
            if ripple > 0.72 {
                img.set(x, y, PLASTER_DARK);
            }
            if dune < -0.45 {
                img.set(x, y, FOAM);
            }
        }
    }
    draw::scatter(&mut img, draw::TILE, PLASTER_DARK, 3, seed ^ 0x51);
    draw::bevel(&mut img, FOAM, PLASTER_DARK);
    img
}

/// Cracked stone: two wandering fissures, lit facets and a lit top edge.
fn rock(base: Color8, facet: Color8, crack: Color8, seed: u64) -> Image {
    let mut img = Image::new(SIZE, SIZE, base);
    draw::dither(&mut img, draw::TILE, crack, seed);

    // Two fissures: a vertical wander and a short diagonal branch.
    let mut x = 4;
    for y in 1..14 {
        draw::put(&mut img, x, y, crack);
        draw::put(&mut img, x + 1, y, base);
        if draw::hash01(y, 3, seed) < 0.35 && x < 11 {
            x += 1;
        }
    }
    let mut bx = 10;
    for y in 5..11 {
        draw::put(&mut img, bx, y, crack);
        if draw::hash01(y, 11, seed) < 0.5 && bx > 8 {
            bx -= 1;
        }
    }

    // Lit facets on the upper-left half, a scatter of grain inside the stone.
    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = draw::hash01(x as i32, y as i32, seed ^ 0xA5) < 0.16;
            let upper_left = x + y < 12;
            if inside && upper_left {
                img.set(x, y, facet);
            }
        }
    }
    draw::bevel(&mut img, facet, crack);
    img
}

/// A cliff face: a lit ledge on top, strata and fissures below, shadow at the
/// foot.
fn cliff() -> Image {
    let seed = draw::seed_of("cliff");
    let mut img = Image::new(SIZE, SIZE, STONE_MID);

    // The ledge: the top three rows are the sunlit top of the rock.
    draw::fill(&mut img, Area::new(0, 0, 15, 2), STONE_LIGHT);
    draw::hline(&mut img, 0, 15, 3, STONE_MID);

    // Strata: every fifth row is a darker bedding plane.
    for y in (2..SIZE).step_by(5) {
        draw::hline(&mut img, 0, 15, y as i32, STONE_DARK);
    }
    // Vertical fissures, offset per band so they do not line up.
    for band in 0..3 {
        let x = 3 + band * 5;
        for y in (4 + band * 4)..SIZE {
            draw::put(&mut img, x as i32, y as i32, STONE_DARK);
        }
    }
    draw::scatter(&mut img, Area::new(0, 4, 15, 12), STONE_LIGHT, 10, seed);
    // The foot of the cliff: darker, but never a flat black bar.
    draw::fill(&mut img, Area::new(0, 13, 15, 15), STONE_DARK);
    draw::scatter(&mut img, Area::new(0, 13, 15, 15), SHADOW, 30, seed ^ 0x2C);
    draw::hline(&mut img, 0, 15, 15, SHADOW);
    draw::hline(&mut img, 0, 15, 12, STONE_DARK);
    draw::bevel(&mut img, STONE_LIGHT, SHADOW);
    img
}

/// Snow: tileable drifts over a near-white base, with a cold blue shadow.
fn snow() -> Image {
    let seed = draw::seed_of("snow");
    let mut img = Image::new(SIZE, SIZE, FOAM);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let drift = tileable_value_2d(x as f32 * 0.5, y as f32 * 0.5, 8, seed);
            if drift > 0.45 {
                img.set(x, y, PLASTER_MID);
            }
            if drift < -0.50 {
                img.set(x, y, STONE_LIGHT);
            }
        }
    }
    draw::scatter(&mut img, draw::TILE, PLASTER_MID, 4, seed ^ 0x33);
    draw::bevel(&mut img, PLASTER_MID, STONE_LIGHT);
    img
}

// --- water -----------------------------------------------------------------

/// One pixel of the water field, in world pixel coordinates.
///
/// The function is periodic with period [`SIZE`] in both axes, which is exactly
/// the condition for a tile drawn from it to wrap seamlessly. Both octaves are
/// **under-sampled on purpose**: halving the sampling rate while halving the
/// lattice period keeps the wrap at 16 while stretching the features, and it is
/// the long wavelength that reads as a swell rather than as grain.
#[must_use]
pub fn water_pixel(x: i32, y: i32, deep: bool) -> Color8 {
    let swell = tileable_value_2d(x as f32 * 0.5, y as f32 * 0.5, 8, WATER_SEED);
    // A quarter-speed sample across against a half-speed one down: the ripples
    // come out four times wider than they are tall, so the water has a current.
    let ripple = tileable_value_2d(x as f32 * 0.25, y as f32 * 0.5, 4, WATER_SEED ^ 0x9E37_79B9);

    if deep {
        let mut color = WATER_DEEP;
        if swell > 0.05 {
            color = WATER_MID;
        }
        // A shaded trough under each crest is what gives the ripple its depth.
        if ripple > 0.30 {
            color = WATER_MID;
        }
        if ripple > 0.55 {
            color = WATER_SHALLOW;
        }
        let wrapped_x = x.rem_euclid(SIZE as i32);
        let wrapped_y = y.rem_euclid(SIZE as i32);
        if ripple > 0.78 && draw::hash01(wrapped_x, wrapped_y, WATER_SEED ^ 0x51ED) < 0.35 {
            color = FOAM;
        }
        color
    } else {
        let mut color = WATER_SHALLOW;
        if swell < -0.05 {
            color = WATER_MID;
        }
        if swell < -0.45 {
            color = WATER_DEEP;
        }
        if ripple > 0.25 {
            color = WATER_MID;
        }
        if ripple > 0.52 {
            color = FOAM;
        }
        color
    }
}

/// A water tile built from [`water_pixel`].
fn water(deep: bool) -> Image {
    Image::from_fn(SIZE, SIZE, |x, y| water_pixel(x as i32, y as i32, deep))
}

// --- road ------------------------------------------------------------------

/// One pixel of the road field, in world pixel coordinates.
///
/// Cobbles are 4x4 cells with a one-pixel mortar joint along each cell's top and
/// left edge. Because `SIZE` is a whole number of cells, a joint lands exactly
/// on the tile seam, so four road tiles meeting at a corner produce **one**
/// joint rather than a double-width cross: the last column and row of a tile are
/// cobble body, never joint. The per-cobble tone is hashed from the cell index
/// modulo the tile, which keeps the whole field periodic.
#[must_use]
pub fn road_pixel(x: i32, y: i32) -> Color8 {
    let local_x = x.rem_euclid(SIZE as i32);
    let local_y = y.rem_euclid(SIZE as i32);
    let cell_x = local_x / 4;
    let cell_y = local_y / 4;
    let joint_x = local_x % 4;
    let joint_y = local_y % 4;

    if joint_x == 0 || joint_y == 0 {
        return ROAD_DARK;
    }

    // Per-cobble tone, then the fixed shading of the stone itself. The shading
    // uses the mid tone rather than the joint colour on purpose, so the seam
    // edge of a tile cannot be mistaken for a second joint.
    let tone = draw::hash01(cell_x, cell_y, ROAD_SEED);
    let mut color = if tone > 0.66 {
        STONE_MID
    } else if tone < 0.22 {
        ROAD_DARK
    } else {
        ROAD_MID
    };
    if joint_x == 1 && joint_y <= 2 {
        color = STONE_MID;
    }
    if joint_x == 3 || joint_y == 3 {
        color = ROAD_MID;
    }
    // Worn grit.
    if draw::hash01(local_x, local_y, ROAD_SEED ^ 0x1234) < 0.08 {
        color = STONE_LIGHT;
    }
    color
}

/// A road tile built from [`road_pixel`].
fn road() -> Image {
    Image::from_fn(SIZE, SIZE, |x, y| road_pixel(x as i32, y as i32))
}

/// A road tile with a grass verge and a kerb along the top edge.
///
/// This is the boundary tile: it is meant to be placed where the paving ends,
/// so only its horizontal joint rhythm has to line up with its neighbours.
fn road_edge() -> Image {
    let mut img = Image::new(SIZE, SIZE, GRASS_MID);
    // Verge and kerb.
    draw::dither(
        &mut img,
        Area::new(0, 0, 15, 3),
        GRASS_DARK,
        draw::seed_of("road_edge"),
    );
    draw::hline(&mut img, 0, 15, 4, STONE_LIGHT);
    draw::hline(&mut img, 0, 15, 5, STONE_MID);
    for x in 0..SIZE {
        img.set(x, 4, STONE_LIGHT);
        if draw::chance(x as i32, 5, ROAD_SEED, 40) {
            img.set(x, 5, STONE_DARK);
        }
    }
    // The paving itself, sampled from the same field as a plain road tile so
    // the cobbles continue across the boundary.
    for y in 6..SIZE {
        for x in 0..SIZE {
            img.set(x, y, road_pixel(x as i32, y as i32));
        }
    }
    // This tile carries its own lighting (a lit verge on top), and its paving
    // rows are left exactly as the field drew them so the cobbles continue
    // into the neighbouring road tile without a seam.
    draw::hline(&mut img, 0, 15, 0, GRASS_LIGHT);
    img
}

/// Paved plaza: 8x8 slabs, so the slab grid also wraps.
fn plaza() -> Image {
    let mut img = Image::new(SIZE, SIZE, STONE_MID);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let slab_x = x % 8;
            let slab_y = y % 8;
            let color = if slab_x == 0 || slab_y == 0 {
                STONE_DARK
            } else if slab_x <= 1 || slab_y <= 1 {
                STONE_LIGHT
            } else {
                STONE_MID
            };
            img.set(x, y, color);
        }
    }
    draw::scatter(&mut img, draw::TILE, STONE_LIGHT, 7, PLAZA_SEED);
    draw::scatter(&mut img, draw::TILE, STONE_DARK, 5, PLAZA_SEED ^ 0x77);
    draw::bevel(&mut img, STONE_LIGHT, STONE_DARK);
    img
}

/// A plank bridge deck: four boards with lit top edges, dark gaps and nails.
fn bridge() -> Image {
    let mut img = Image::new(SIZE, SIZE, DIRT_MID);
    for y in 0..SIZE {
        let local_y = y % 4;
        for x in 0..SIZE {
            let color = match local_y {
                3 => DIRT_DARK,
                0 => DIRT_LIGHT,
                _ => DIRT_MID,
            };
            img.set(x, y, color);
            // Grain: worm-eaten streaks along the board.
            if local_y == 1 && draw::hash01(x as i32, y as i32, BRIDGE_SEED) < 0.18 {
                img.set(x, y, DIRT_DARK);
            }
            if local_y == 2 && draw::hash01(x as i32, y as i32, BRIDGE_SEED ^ 0x99) < 0.12 {
                img.set(x, y, DIRT_LIGHT);
            }
        }
    }
    // Nails at the ends of every board.
    for y in 0..SIZE {
        if y % 4 == 1 {
            img.set(2, y, STONE_LIGHT);
            img.set(13, y, STONE_LIGHT);
        }
    }
    draw::bevel(&mut img, DIRT_LIGHT, SHADOW);
    img
}
