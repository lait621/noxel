//! The buildings atlas: twelve 16x16 wall, roof and floor tiles.
//!
//! These are placed as voxels by the prefabs, so every one of them is a
//! full-bleed tile: no transparency, and a pattern that continues across the
//! tile boundary wherever the material repeats (stone courses, roof shingles,
//! floor planks).

use noxel_asset::image::Image;
use noxel_core::math::Color8;

use crate::draw::{self, Area};
use crate::palette::*;

/// Edge length of one building tile, in pixels.
pub const SIZE: u32 = 16;

/// The tiles, in atlas order. Index == atlas column.
pub const NAMES: [&str; 12] = [
    "wall_plaster",
    "wall_wood",
    "wall_stone",
    "wall_window",
    "wall_door",
    "roof_red",
    "roof_slate",
    "roof_edge",
    "chimney",
    "floor_wood",
    "floor_stone",
    "counter",
];

/// The atlas column of a tile, or `None` when the name is unknown.
#[must_use]
pub fn index_of(name: &str) -> Option<usize> {
    NAMES.iter().position(|candidate| *candidate == name)
}

/// The atlas image: every tile side by side, `12 * SIZE` wide and `SIZE` tall.
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

/// One building tile by name.
#[must_use]
pub fn tile(name: &str) -> Option<Image> {
    let image = match name {
        "wall_plaster" => wall_plaster(),
        "wall_wood" => wall_wood(),
        "wall_stone" => wall_stone(),
        "wall_window" => wall_window(),
        "wall_door" => wall_door(),
        "roof_red" => roof(ROOF_RED, ROOF_LIGHT, ROOF_DARK, draw::seed_of("roof_red")),
        "roof_slate" => roof(
            STONE_MID,
            STONE_LIGHT,
            STONE_DARK,
            draw::seed_of("roof_slate"),
        ),
        "roof_edge" => roof_edge(),
        "chimney" => chimney(),
        "floor_wood" => floor_wood(),
        "floor_stone" => floor_stone(),
        "counter" => counter(),
        _ => return None,
    };
    Some(image)
}

/// A plastered wall: warm render, a hairline crack and a shadowed foot.
fn wall_plaster() -> Image {
    let seed = draw::seed_of("wall_plaster");
    let mut img = Image::new(SIZE, SIZE, PLASTER_MID);
    draw::dither(&mut img, draw::TILE, PLASTER_DARK, seed);
    // Two hairline cracks wandering down the render.
    let mut crack_x = 4;
    for y in 2..13 {
        draw::put(&mut img, crack_x, y, PLASTER_DARK);
        if draw::hash01(y, 1, seed) < 0.35 && crack_x < 9 {
            crack_x += 1;
        }
    }
    draw::put(&mut img, 11, 5, PLASTER_DARK);
    draw::put(&mut img, 11, 6, PLASTER_DARK);
    draw::put(&mut img, 11, 7, FOAM);
    draw::scatter(&mut img, draw::TILE, FOAM, 6, seed ^ 0x21);
    draw::bevel(&mut img, FOAM, PLASTER_DARK);
    img
}

/// A plank wall: four boards with lit top edges and dark shadow gaps.
fn wall_wood() -> Image {
    let seed = draw::seed_of("wall_wood");
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
            if local_y == 1 && draw::hash01(x as i32, y as i32, seed) < 0.20 {
                img.set(x, y, DIRT_DARK);
            }
            if local_y == 2 && draw::hash01(x as i32, y as i32, seed ^ 0x55) < 0.12 {
                img.set(x, y, DIRT_LIGHT);
            }
        }
    }
    // A nail at each end of every board.
    for y in 0..SIZE {
        if y % 4 == 1 {
            img.set(1, y, STONE_MID);
            img.set(14, y, STONE_MID);
        }
    }
    draw::bevel(&mut img, DIRT_LIGHT, SHADOW);
    img
}

/// A stone wall: staggered courses of lit bricks with dark mortar.
fn wall_stone() -> Image {
    let seed = draw::seed_of("wall_stone");
    let mut img = Image::new(SIZE, SIZE, STONE_MID);
    for y in 0..SIZE {
        let row = y / 4;
        let offset = if row % 2 == 0 { 0 } else { 4 };
        for x in 0..SIZE {
            let brick_x = (x + offset) % 8;
            let brick_y = y % 4;
            let color = if brick_y == 0 || brick_x == 0 {
                STONE_DARK
            } else if brick_y == 1 {
                STONE_LIGHT
            } else {
                STONE_MID
            };
            img.set(x, y, color);
            if draw::hash01(x as i32, y as i32, seed) < 0.10 {
                img.set(x, y, STONE_DARK);
            }
        }
    }
    draw::bevel(&mut img, STONE_LIGHT, STONE_DARK);
    img
}

/// A plastered wall with a glazed window: dark frame, muntins and a glint.
fn wall_window() -> Image {
    let seed = draw::seed_of("wall_window");
    let mut img = wall_plaster();
    draw::fill(&mut img, Area::new(2, 2, 13, 13), SHADOW);
    // Three bands of glass: the sky it reflects, the sheet itself, and the
    // dark room behind it. Reading top to bottom is what makes it a window
    // rather than a blue square.
    draw::fill(&mut img, Area::new(3, 3, 12, 12), WATER_SHALLOW);
    draw::fill(&mut img, Area::new(3, 3, 12, 4), FOAM);
    draw::fill(&mut img, Area::new(3, 9, 12, 12), WATER_MID);
    draw::scatter(&mut img, Area::new(3, 5, 12, 8), FOAM, 12, seed ^ 0x66);
    // Muntins and sill.
    draw::vline(&mut img, 7, 3, 12, SHADOW);
    draw::vline(&mut img, 8, 3, 12, SHADOW);
    draw::hline(&mut img, 2, 13, 7, SHADOW);
    draw::hline(&mut img, 2, 13, 14, STONE_LIGHT);
    draw::hline(&mut img, 2, 13, 15, STONE_DARK);
    img
}

/// A plank door in a dark frame, with iron hinges and a lit handle.
fn wall_door() -> Image {
    let seed = draw::seed_of("wall_door");
    let mut img = Image::new(SIZE, SIZE, PLASTER_MID);
    draw::dither(&mut img, draw::TILE, PLASTER_DARK, seed);
    draw::fill(&mut img, Area::new(1, 1, 14, 15), SHADOW);
    draw::fill(&mut img, Area::new(2, 2, 13, 15), DIRT_MID);
    // Vertical boards.
    for x in [3, 7, 11] {
        draw::vline(&mut img, x, 2, 15, DIRT_DARK);
    }
    draw::vline(&mut img, 2, 2, 15, DIRT_LIGHT);
    // Recessed panels.
    draw::frame(&mut img, Area::new(4, 4, 6, 9), DIRT_LIGHT);
    draw::frame(&mut img, Area::new(9, 4, 11, 9), DIRT_LIGHT);
    // Hinges and handle.
    draw::hline(&mut img, 2, 5, 4, STONE_MID);
    draw::hline(&mut img, 2, 5, 11, STONE_MID);
    draw::put(&mut img, 12, 8, SAND_LIGHT);
    draw::put(&mut img, 12, 9, SAND_LIGHT);
    draw::hline(&mut img, 1, 14, 1, STONE_LIGHT);
    img
}

/// A run of shingles in one material ramp.
fn roof(base: Color8, light: Color8, dark: Color8, seed: u64) -> Image {
    let mut img = Image::new(SIZE, SIZE, base);
    for y in 0..SIZE {
        let row = y / 4;
        let local_y = y % 4;
        let offset = (row % 2) * 2;
        for x in 0..SIZE {
            let local_x = (x + offset) % 4;
            let color = match local_y {
                0 => light,
                3 => dark,
                _ => base,
            };
            img.set(x, y, color);
            // A darker seam where two shingles meet, and a little wear.
            if local_x == 0 && local_y != 0 {
                img.set(x, y, dark);
            }
            if local_y == 1 && local_x == 1 && draw::hash01(x as i32, y as i32, seed) < 0.5 {
                img.set(x, y, light);
            }
        }
    }
    draw::bevel(&mut img, light, dark);
    img
}

/// The ridge: a lit cap, its shadow, and the darker course below it.
fn roof_edge() -> Image {
    let seed = draw::seed_of("roof_edge");
    let mut img = Image::new(SIZE, SIZE, ROOF_DARK);
    draw::hline(&mut img, 0, 15, 0, ROOF_LIGHT);
    draw::fill(&mut img, Area::new(0, 1, 15, 2), ROOF_RED);
    draw::hline(&mut img, 0, 15, 3, ROOF_DARK);
    draw::hline(&mut img, 0, 15, 4, SHADOW);
    for y in 5..SIZE {
        for x in 0..SIZE {
            let local_y = y % 4;
            let mut color = ROOF_DARK;
            if local_y == 0 {
                color = ROOF_RED;
            }
            if draw::hash01(x as i32, y as i32, seed) < 0.10 {
                color = SHADOW;
            }
            img.set(x, y, color);
        }
    }
    img
}

/// A brick chimney with a stone cap and a soot smudge.
fn chimney() -> Image {
    let mut img = Image::new(SIZE, SIZE, ROOF_DARK);
    for y in 0..SIZE {
        let row = y / 4;
        let offset = if row % 2 == 0 { 0 } else { 3 };
        for x in 0..SIZE {
            let brick_x = (x + offset) % 6;
            let brick_y = y % 4;
            let color = if brick_y == 3 || brick_x == 0 {
                SHADOW
            } else if brick_y == 0 {
                ROOF_RED
            } else {
                ROOF_DARK
            };
            img.set(x, y, color);
        }
    }
    // Cap.
    draw::fill(&mut img, Area::new(0, 0, 15, 1), STONE_MID);
    draw::hline(&mut img, 0, 15, 0, STONE_LIGHT);
    draw::hline(&mut img, 0, 15, 2, SHADOW);
    // Soot above the flue.
    draw::scatter(
        &mut img,
        Area::new(4, 3, 11, 6),
        SHADOW,
        35,
        draw::seed_of("chimney"),
    );
    draw::bevel(&mut img, STONE_LIGHT, SHADOW);
    img
}

/// An interior plank floor with knots in the boards.
fn floor_wood() -> Image {
    let seed = draw::seed_of("floor_wood");
    let mut img = Image::new(SIZE, SIZE, DIRT_MID);
    for y in 0..SIZE {
        let local_y = y % 5;
        for x in 0..SIZE {
            let color = match local_y {
                4 => DIRT_DARK,
                0 => DIRT_LIGHT,
                _ => DIRT_MID,
            };
            img.set(x, y, color);
            if local_y == 2 && draw::hash01(x as i32, y as i32, seed) < 0.16 {
                img.set(x, y, DIRT_DARK);
            }
        }
    }
    // Board ends and two knots.
    for y in 0..SIZE {
        if y % 5 != 4 {
            img.set(6, y, DIRT_DARK);
        }
    }
    for (x, y) in [(3, 2), (11, 7)] {
        draw::put(&mut img, x, y, DIRT_DARK);
        draw::put(&mut img, x + 1, y, DIRT_DARK);
        draw::put(&mut img, x, y + 1, DIRT_LIGHT);
    }
    draw::bevel(&mut img, DIRT_LIGHT, SHADOW);
    img
}

/// An interior flagstone floor: warm slabs with worn corners.
fn floor_stone() -> Image {
    let seed = draw::seed_of("floor_stone");
    let mut img = Image::new(SIZE, SIZE, PLASTER_DARK);
    for y in 0..SIZE {
        let row = y / 8;
        let local_y = y % 8;
        for x in 0..SIZE {
            let offset = if row % 2 == 0 { 0 } else { 4 };
            let slab_x = (x + offset) % 8;
            let color = if local_y == 0 || slab_x == 0 {
                STONE_DARK
            } else if local_y == 1 || slab_x == 1 {
                PLASTER_MID
            } else {
                PLASTER_DARK
            };
            img.set(x, y, color);
            if draw::hash01(x as i32, y as i32, seed) < 0.14 {
                img.set(x, y, STONE_MID);
            }
        }
    }
    draw::bevel(&mut img, PLASTER_MID, STONE_DARK);
    img
}

/// A shop counter: a pale top over a dark wooden front.
fn counter() -> Image {
    let seed = draw::seed_of("counter");
    let mut img = Image::new(SIZE, SIZE, DIRT_MID);
    draw::fill(&mut img, Area::new(0, 0, 15, 2), PLASTER_MID);
    draw::hline(&mut img, 0, 15, 0, FOAM);
    draw::hline(&mut img, 0, 15, 3, PLASTER_DARK);
    // Front: vertical boards with a moulding.
    for x in 0..SIZE {
        if x % 5 == 0 {
            draw::vline(&mut img, x as i32, 4, 15, DIRT_DARK);
        } else {
            draw::vline(&mut img, x as i32, 4, 15, DIRT_MID);
        }
        if draw::chance(x as i32, 9, seed, 15) {
            img.set(x, 9, DIRT_LIGHT);
        }
    }
    draw::hline(&mut img, 0, 15, 4, DIRT_LIGHT);
    draw::hline(&mut img, 0, 15, 15, SHADOW);
    img
}
