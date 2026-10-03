//! The prop atlas: trees, rocks, fences and street furniture.
//!
//! Props are sprites, not ground, so they are drawn on a transparent cell and
//! finished with a one-pixel dark outline. The outline is what keeps a barrel
//! readable when it stands on a busy cobble tile; it is also why every cell
//! keeps a one-pixel margin.
//!
//! The canopy is the only multi-frame prop: three frames of the same tree,
//! swaying one pixel left and right, laid out side by side.

use noxel_asset::image::Image;
use noxel_core::math::Color8;

use crate::draw::{self, Area};
use crate::palette::*;

/// Width and height of an ordinary prop cell.
pub const CELL: u32 = 16;
/// Width and height of a canopy cell.
pub const CANOPY: u32 = 24;
/// Height of the whole sheet.
pub const HEIGHT: u32 = 24;

/// One named region of the sheet.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    /// Region name, e.g. `"tree_trunk"`.
    pub name: &'static str,
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

/// Every prop cell, in sheet order. The layout is fixed: it is the contract the
/// demo's prop renderer is written against, and it is documented in
/// `assets/README.md`.
///
/// A prop stands on the ground, so its cell is **bottom-aligned**: a 16x16 prop
/// sits in the lower 16 rows of the 24-pixel sheet and a 24x24 canopy fills it.
/// A renderer places a prop by its bottom-centre.
pub const CELLS: [Cell; 14] = [
    Cell {
        name: "tree_canopy_0",
        x: 0,
        y: 0,
        w: CANOPY,
        h: CANOPY,
    },
    Cell {
        name: "tree_canopy_1",
        x: 24,
        y: 0,
        w: CANOPY,
        h: CANOPY,
    },
    Cell {
        name: "tree_canopy_2",
        x: 48,
        y: 0,
        w: CANOPY,
        h: CANOPY,
    },
    Cell {
        name: "tree_trunk",
        x: 72,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "bush",
        x: 88,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "rock_small",
        x: 104,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "rock_large",
        x: 120,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "flower",
        x: 136,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "fence_h",
        x: 152,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "fence_v",
        x: 168,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "well",
        x: 184,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "lamp_post",
        x: 200,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "barrel",
        x: 216,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
    Cell {
        name: "crate",
        x: 232,
        y: HEIGHT - CELL,
        w: CELL,
        h: CELL,
    },
];

/// The prop sheet: every cell side by side, 248x24.
#[must_use]
pub fn atlas() -> Image {
    let mut sheet = Image::transparent(CELLS.last().map_or(0, |cell| cell.x + cell.w), HEIGHT);
    for cell in &CELLS {
        if let Some(image) = region(cell.name) {
            sheet.blit(&image, cell.x as i32, cell.y as i32);
        }
    }
    sheet
}

/// One prop as its own image, or `None` for an unknown name.
#[must_use]
pub fn region(name: &str) -> Option<Image> {
    let image = match name {
        "tree_canopy_0" => canopy(0),
        "tree_canopy_1" => canopy(1),
        "tree_canopy_2" => canopy(2),
        "tree_trunk" => trunk(),
        "bush" => bush(),
        "rock_small" => boulder(8, 6, STONE_MID, STONE_LIGHT, STONE_DARK, 4, 9),
        "rock_large" => boulder(14, 12, STONE_DARK, STONE_MID, SHADOW, 1, 4),
        "flower" => flower(),
        "fence_h" => fence(),
        "fence_v" => fence().rotate_90_cw(),
        "well" => well(),
        "lamp_post" => lamp_post(),
        "barrel" => barrel(),
        "crate" => crate_box(),
        _ => return None,
    };
    Some(image)
}

/// A round tree crown built from three overlapping clumps.
///
/// `frame` is 0, 1 or 2: the clumps shift one pixel left or right, which is the
/// whole sway animation.
fn canopy(frame: u32) -> Image {
    let seed = draw::seed_of("canopy");
    let mut img = Image::transparent(CANOPY, CANOPY);
    let sway = frame.min(2) as i32 - 1;
    let clumps = [(8, 13, 6), (15, 13, 6), (11, 8, 7)];

    for y in 0..CANOPY {
        for x in 0..CANOPY {
            let inside = clumps.iter().any(|(cx, cy, r)| {
                let dx = x as i32 - (cx + sway);
                let dy = y as i32 - cy;
                dx * dx + dy * dy <= r * r
            });
            if !inside {
                continue;
            }
            let mut color = LEAF_MID;
            // Sun from the top-left, depth at the bottom-right.
            if (x as i32 - (7 + sway)) + (y as i32 - 6) < 3 {
                color = GRASS_LIGHT;
            }
            if draw::hash01(x as i32, y as i32, seed) < 0.20 {
                color = LEAF_DARK;
            }
            if (x as i32) + (y as i32) > 30 {
                color = LEAF_DARK;
            }
            if draw::hash01(x as i32, y as i32, seed ^ 0x44) < 0.10 && color == LEAF_MID {
                color = GRASS_LIGHT;
            }
            img.set(x, y, color);
        }
    }
    draw::outline(&mut img, SHADOW);
    img
}

/// A trunk with roots, so a canopy has something to stand on.
fn trunk() -> Image {
    let seed = draw::seed_of("trunk");
    let mut img = Image::transparent(CELL, CELL);
    // The bole.
    draw::fill(&mut img, Area::new(6, 1, 9, 13), DIRT_MID);
    draw::vline(&mut img, 6, 1, 13, DIRT_LIGHT);
    draw::vline(&mut img, 9, 1, 13, DIRT_DARK);
    for y in 2..13 {
        if draw::hash01(7, y, seed) < 0.45 {
            draw::put(&mut img, 7, y, DIRT_DARK);
        }
        if draw::hash01(8, y, seed ^ 0x11) < 0.35 {
            draw::put(&mut img, 8, y, DIRT_LIGHT);
        }
    }
    // Roots flaring at the base.
    draw::fill(&mut img, Area::new(4, 13, 11, 14), DIRT_MID);
    draw::hline(&mut img, 3, 12, 15, DIRT_DARK);
    draw::put(&mut img, 4, 13, DIRT_LIGHT);
    draw::put(&mut img, 11, 14, DIRT_DARK);
    draw::outline(&mut img, SHADOW);
    img
}

/// A low leafy bush.
fn bush() -> Image {
    let seed = draw::seed_of("bush");
    let mut img = Image::transparent(CELL, CELL);
    let clumps = [(5, 10, 4), (10, 10, 4), (7, 7, 4)];
    for y in 0..CELL {
        for x in 0..CELL {
            let inside = clumps.iter().any(|(cx, cy, r)| {
                let dx = x as i32 - cx;
                let dy = y as i32 - cy;
                dx * dx + dy * dy <= r * r
            });
            if !inside {
                continue;
            }
            let mut color = LEAF_MID;
            if (x as i32 - 4) + (y as i32 - 5) < 2 {
                color = GRASS_LIGHT;
            }
            if draw::hash01(x as i32, y as i32, seed) < 0.22 {
                color = LEAF_DARK;
            }
            if (x as i32) + (y as i32) > 19 {
                color = LEAF_DARK;
            }
            img.set(x, y, color);
        }
    }
    draw::outline(&mut img, SHADOW);
    img
}

/// A faceted boulder in the bottom-left of its cell.
fn boulder(
    width: u32,
    height: u32,
    base: Color8,
    facet: Color8,
    shade: Color8,
    origin_x: u32,
    origin_y: u32,
) -> Image {
    let seed = draw::seed_of("boulder");
    let mut img = Image::transparent(CELL, CELL);
    let (w, h) = (width as i32, height as i32);
    for dy in 0..h {
        for dx in 0..w {
            // A rounded silhouette: inset the top and bottom rows by the
            // ellipse equation so the rock does not read as a box.
            let nx = (dx as f32 + 0.5) / (w as f32 / 2.0) - 1.0;
            let ny = (dy as f32 + 0.5) / (h as f32 / 2.0) - 1.0;
            if nx * nx + ny * ny > 1.05 {
                continue;
            }
            let x = origin_x as i32 + dx;
            let y = origin_y as i32 + dy;
            let mut color = base;
            if ny < -0.35 {
                color = facet;
            }
            if ny > 0.35 {
                color = shade;
            }
            if draw::hash01(x, y, seed) < 0.14 {
                color = facet;
            }
            draw::put(&mut img, x, y, color);
        }
    }
    // A crack across the face.
    let mut crack_x = origin_x as i32 + 1;
    for y in (origin_y as i32 + 1)..(origin_y as i32 + h - 1) {
        draw::put(&mut img, crack_x, y, shade);
        if draw::hash01(crack_x, y, seed ^ 0x5A) < 0.4 {
            crack_x += 1;
        }
    }
    draw::outline(&mut img, SHADOW);
    img
}

/// A roadside flower on a stem.
fn flower() -> Image {
    let mut img = Image::transparent(CELL, CELL);
    // Stem and leaves.
    draw::vline(&mut img, 7, 8, 14, GRASS_MID);
    draw::put(&mut img, 5, 11, GRASS_LIGHT);
    draw::put(&mut img, 6, 11, GRASS_MID);
    draw::put(&mut img, 8, 9, GRASS_MID);
    draw::put(&mut img, 9, 9, GRASS_LIGHT);
    // Four petals around a lit centre.
    draw::put(&mut img, 7, 4, ROOF_RED);
    draw::put(&mut img, 6, 5, ROOF_RED);
    draw::put(&mut img, 8, 5, ROOF_LIGHT);
    draw::put(&mut img, 7, 6, ROOF_RED);
    draw::put(&mut img, 9, 4, ROOF_LIGHT);
    draw::put(&mut img, 5, 4, ROOF_LIGHT);
    draw::put(&mut img, 7, 5, SAND_LIGHT);
    draw::outline(&mut img, SHADOW);
    img
}

/// A horizontal fence: two posts and two rails.
fn fence() -> Image {
    let seed = draw::seed_of("fence");
    let mut img = Image::transparent(CELL, CELL);
    // Rails first, so the posts overlap them.
    for y in [6, 10] {
        draw::hline(&mut img, 0, 15, y, DIRT_MID);
        draw::hline(&mut img, 0, 15, y + 1, DIRT_DARK);
        for x in 0..CELL {
            if draw::chance(x as i32, y, seed, 25) {
                img.set(x, y as u32, DIRT_LIGHT);
            }
        }
    }
    // Posts.
    for x in [2, 12] {
        draw::fill(&mut img, Area::new(x, 3, x + 1, 14), DIRT_MID);
        draw::vline(&mut img, x, 3, 14, DIRT_LIGHT);
        draw::vline(&mut img, x + 1, 3, 14, DIRT_DARK);
        draw::hline(&mut img, x, x + 1, 3, DIRT_LIGHT);
        draw::hline(&mut img, x, x + 1, 14, DIRT_DARK);
    }
    draw::outline(&mut img, SHADOW);
    img
}

/// A stone well with a little shingled roof.
fn well() -> Image {
    let mut img = Image::transparent(CELL, CELL);
    // Roof.
    draw::fill(&mut img, Area::new(2, 1, 13, 2), ROOF_RED);
    draw::hline(&mut img, 3, 12, 3, ROOF_DARK);
    draw::hline(&mut img, 2, 13, 1, ROOF_LIGHT);
    // Posts.
    draw::vline(&mut img, 4, 3, 7, DIRT_MID);
    draw::vline(&mut img, 11, 3, 7, DIRT_DARK);
    // Stone ring.
    draw::fill(&mut img, Area::new(2, 8, 13, 14), STONE_MID);
    draw::hline(&mut img, 2, 13, 8, STONE_LIGHT);
    draw::hline(&mut img, 2, 13, 14, STONE_DARK);
    draw::vline(&mut img, 2, 8, 14, STONE_LIGHT);
    draw::vline(&mut img, 13, 8, 14, STONE_DARK);
    // Water in the shaft.
    draw::fill(&mut img, Area::new(5, 10, 10, 12), WATER_DEEP);
    draw::hline(&mut img, 5, 10, 10, WATER_SHALLOW);
    draw::put(&mut img, 7, 11, FOAM);
    // Bucket on a rope.
    draw::put(&mut img, 7, 5, DIRT_DARK);
    draw::put(&mut img, 7, 6, DIRT_MID);
    draw::fill(&mut img, Area::new(6, 6, 8, 7), DIRT_MID);
    draw::hline(&mut img, 6, 8, 7, DIRT_DARK);
    draw::outline(&mut img, SHADOW);
    img
}

/// A lamp post with a lit head.
fn lamp_post() -> Image {
    let mut img = Image::transparent(CELL, CELL);
    // Head: a dark frame around warm glass.
    draw::fill(&mut img, Area::new(5, 1, 10, 4), SHADOW);
    draw::fill(&mut img, Area::new(6, 2, 9, 3), SAND_LIGHT);
    draw::hline(&mut img, 6, 9, 2, FOAM);
    draw::hline(&mut img, 5, 10, 5, STONE_DARK);
    // Pole.
    draw::vline(&mut img, 7, 5, 14, STONE_MID);
    draw::vline(&mut img, 8, 5, 14, STONE_DARK);
    draw::put(&mut img, 7, 6, STONE_LIGHT);
    // Base.
    draw::fill(&mut img, Area::new(5, 14, 10, 15), STONE_MID);
    draw::hline(&mut img, 5, 10, 14, STONE_LIGHT);
    draw::hline(&mut img, 5, 10, 15, SHADOW);
    draw::outline(&mut img, SHADOW);
    img
}

/// A wooden barrel with iron bands.
fn barrel() -> Image {
    let seed = draw::seed_of("barrel");
    let mut img = Image::transparent(CELL, CELL);
    draw::fill(&mut img, Area::new(4, 2, 11, 14), DIRT_MID);
    // Staves.
    for x in [5, 7, 9] {
        draw::vline(&mut img, x, 2, 14, DIRT_DARK);
    }
    draw::vline(&mut img, 4, 2, 14, DIRT_LIGHT);
    draw::vline(&mut img, 11, 2, 14, DIRT_DARK);
    // Bands.
    for y in [4, 11] {
        draw::hline(&mut img, 4, 11, y, STONE_MID);
        draw::hline(&mut img, 4, 11, y + 1, STONE_DARK);
    }
    // Lid.
    draw::hline(&mut img, 4, 11, 2, DIRT_LIGHT);
    draw::hline(&mut img, 4, 11, 1, DIRT_DARK);
    for x in 0..CELL {
        if draw::chance(x as i32, 6, seed, 20) {
            img.set(x, 6, DIRT_LIGHT);
        }
    }
    draw::outline(&mut img, SHADOW);
    img
}

/// A slatted wooden crate.
fn crate_box() -> Image {
    let mut img = Image::transparent(CELL, CELL);
    draw::fill(&mut img, Area::new(2, 3, 13, 14), DIRT_MID);
    // Frame.
    draw::frame(&mut img, Area::new(2, 3, 13, 14), DIRT_DARK);
    draw::hline(&mut img, 2, 13, 4, DIRT_LIGHT);
    draw::vline(&mut img, 3, 4, 13, DIRT_LIGHT);
    // Cross braces.
    for step in 0..9 {
        draw::put(&mut img, 4 + step, 12 - step, DIRT_LIGHT);
        draw::put(&mut img, 5 + step, 12 - step, DIRT_DARK);
        draw::put(&mut img, 10 - step, 12 - step, DIRT_LIGHT);
    }
    // Nails.
    for (x, y) in [(3, 4), (12, 4), (3, 13), (12, 13)] {
        draw::put(&mut img, x, y, STONE_LIGHT);
    }
    draw::outline(&mut img, SHADOW);
    img
}
