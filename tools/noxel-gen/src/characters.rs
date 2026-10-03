//! The character sheet: four directions, four walk frames and an idle pose.
//!
//! The sheet is a 5x4 grid of 16x24 cells: one row per direction (down, left,
//! right, up), columns 0-3 are the walk cycle and column 4 is the idle pose.
//! `assets/sprites/characters.json` describes the same grid, so the renderer
//! never has to hard-code it.
//!
//! Readability at 1x drives every decision here. The head is eight pixels wide
//! so the face can carry two pixels of eye; the tunic is a saturated red
//! against muted ground; and the whole figure is outlined in the palette's
//! darkest colour so it reads as one shape on a busy tile.

use noxel_asset::image::Image;
use noxel_core::math::Color8;

use crate::draw;
use crate::palette::*;

/// Cell width in pixels.
pub const CELL_W: u32 = 16;
/// Cell height in pixels.
pub const CELL_H: u32 = 24;
/// Walk frames per direction.
pub const WALK_FRAMES: usize = 4;
/// Sheet columns: four walk frames plus the idle cell.
pub const COLS: u32 = WALK_FRAMES as u32 + 1;
/// Sheet rows: one per direction.
pub const ROWS: u32 = 4;
/// The directions, in sheet-row order.
pub const DIRECTIONS: [&str; 4] = ["down", "left", "right", "up"];
/// The idle pose's column.
pub const IDLE_COLUMN: usize = 4;

/// Hair, trousers and boots, so the three figures read as one cast.
const HAIR: Color8 = DIRT_DARK;
const HAIR_LIGHT: Color8 = DIRT_MID;
const TROUSERS: Color8 = STONE_DARK;
const TROUSERS_LIGHT: Color8 = STONE_MID;
const BOOTS: Color8 = SHADOW;

/// Which pose a cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    /// Standing still, feet together.
    Idle,
    /// One of the four walk-cycle frames.
    Walk(usize),
}

/// The whole sheet: `COLS * CELL_W` by `ROWS * CELL_H`.
#[must_use]
pub fn sheet() -> Image {
    let mut image = Image::transparent(COLS * CELL_W, ROWS * CELL_H);
    for (row, direction) in DIRECTIONS.iter().enumerate() {
        for column in 0..COLS as usize {
            let pose = if column == IDLE_COLUMN {
                Pose::Idle
            } else {
                Pose::Walk(column)
            };
            if let Some(cell) = character(direction, pose) {
                image.blit(
                    &cell,
                    (column as u32 * CELL_W) as i32,
                    (row as u32 * CELL_H) as i32,
                );
            }
        }
    }
    image
}

/// The `[x, y, w, h]` region of one cell.
#[must_use]
pub fn cell_uv(direction: &str, column: usize) -> Option<[u32; 4]> {
    let row = DIRECTIONS.iter().position(|name| *name == direction)?;
    if column >= COLS as usize {
        return None;
    }
    Some([column as u32 * CELL_W, row as u32 * CELL_H, CELL_W, CELL_H])
}

/// The canonical frame name for a cell: `walk_down_00`, `idle_up`, ...
#[must_use]
pub fn frame_name(direction: &str, column: usize) -> String {
    if column == IDLE_COLUMN {
        format!("idle_{direction}")
    } else {
        format!("walk_{direction}_{column:02}")
    }
}

/// One character cell, or `None` for an unknown direction.
#[must_use]
pub fn character(direction: &str, pose: Pose) -> Option<Image> {
    let frame = match pose {
        Pose::Idle => IDLE_COLUMN,
        Pose::Walk(index) => index % WALK_FRAMES,
    };
    // A one-pixel bob on the two passing frames: the body rises while the
    // legs are together, which is what separates a walk from a slide.
    let bob = if matches!(pose, Pose::Walk(1 | 3)) {
        -1
    } else {
        0
    };

    let mut image = Image::transparent(CELL_W, CELL_H);
    // Back to front: legs, then the tunic and arms over the hips, then the head.
    draw_legs(&mut image, direction, frame);
    draw_torso(&mut image, direction, bob);
    draw_head(&mut image, direction, bob);
    draw::outline(&mut image, SHADOW);
    // The right-facing sheet is the left-facing one mirrored, which guarantees
    // the pair stays symmetrical.
    Some(if direction == "right" {
        image.flip_x()
    } else {
        image
    })
}

/// Head and face. The four directions are told apart by the face, not by the
/// body: two eyes down, one eye and a nose in profile, no face at all up.
fn draw_head(image: &mut Image, direction: &str, bob: i32) {
    let top = 2 + bob;

    // The skull, in hair.
    if direction == "up" {
        draw::fill(image, 4, top, 11, top + 7, HAIR);
        draw::hline(image, 5, 10, top + 1, HAIR_LIGHT);
        draw::put(image, 5, top + 2, HAIR_LIGHT);
    } else {
        draw::fill(image, 4, top, 11, top + 3, HAIR);
        draw::hline(image, 5, 10, top, HAIR_LIGHT);
    }

    // The face.
    if direction == "up" {
        // Nothing: the back of the head.
    } else {
        draw::fill(image, 5, top + 4, 10, top + 7, SAND_LIGHT);
        draw::vline(image, 5, top + 4, top + 7, SAND_LIGHT);
        draw::vline(image, 10, top + 4, top + 7, DIRT_LIGHT);
        // Side locks framing the face.
        draw::vline(image, 4, top + 3, top + 6, HAIR);
        draw::vline(image, 11, top + 3, top + 6, HAIR);
    }

    match direction {
        "down" => {
            draw::put(image, 6, top + 5, SHADOW);
            draw::put(image, 9, top + 5, SHADOW);
            draw::hline(image, 7, 8, top + 7, DIRT_LIGHT);
        }
        "left" => {
            // Profile: the nose sticks out, one eye, a short mouth.
            draw::fill(image, 4, top + 4, 10, top + 7, SAND_LIGHT);
            draw::vline(image, 11, top + 2, top + 7, HAIR);
            draw::hline(image, 4, 10, top + 4, HAIR);
            draw::put(image, 3, top + 5, SAND_LIGHT);
            draw::put(image, 3, top + 6, DIRT_LIGHT);
            draw::put(image, 5, top + 6, SHADOW);
            draw::hline(image, 4, 5, top + 7, DIRT_LIGHT);
        }
        _ => {}
    }
}

/// Tunic, belt and arms.
fn draw_torso(image: &mut Image, direction: &str, bob: i32) {
    let top = 10 + bob;
    let bottom = 16 + bob;
    let profile = direction == "left" || direction == "right";

    draw::fill(image, 5, top, 10, bottom, ROOF_RED);
    draw::hline(image, 5, 10, top, ROOF_LIGHT);
    draw::vline(image, 10, top, bottom, ROOF_DARK);
    draw::hline(image, 5, 10, top + 3, ROOF_DARK);
    draw::hline(image, 6, 9, top + 4, ROOF_LIGHT);

    // Arms: sleeve over hand, one per side, hanging at the seam.
    let (left_arm, right_arm) = if profile { (6, 9) } else { (3, 11) };
    for x in [left_arm, right_arm] {
        draw::fill(image, x, top + 1, x + 1, top + 4, ROOF_RED);
        draw::put(image, x, top + 5, SAND_LIGHT);
        draw::put(image, x + 1, top + 5, SAND_LIGHT);
    }
    draw::vline(image, left_arm, top + 1, top + 4, ROOF_LIGHT);
    draw::vline(image, right_arm + 1, top + 1, top + 4, ROOF_DARK);

    // Belt.
    draw::hline(image, 5, 10, bottom, SHADOW);
    draw::put(image, 7, bottom, SAND_LIGHT);
    draw::put(image, 8, bottom, SAND_LIGHT);
}

/// Legs and boots. The near leg is drawn after the far one so the walk cycle
/// reads in profile as well as from above.
fn draw_legs(image: &mut Image, direction: &str, frame: usize) {
    let profile = direction == "left" || direction == "right";
    // Per-frame foot placement: contact, passing, opposite contact, passing.
    let (near_dx, far_dx, near_lift, far_lift) = match frame {
        0 => (-1, 1, 0, 0),
        1 => (0, 0, 1, 0),
        2 => (1, -1, 0, 0),
        _ => (0, 0, 0, 1),
    };

    let far_color = SHADOW;
    if profile {
        leg(image, 8 + far_dx, far_color, BOOTS, far_lift);
        leg(image, 6 + near_dx, TROUSERS, BOOTS, near_lift);
    } else {
        leg(image, 5 + far_dx, far_color, BOOTS, far_lift);
        leg(image, 9 + near_dx, TROUSERS, BOOTS, near_lift);
    }
}

/// One leg: a trouser column from the hip to `y = 20` and a boot below it.
fn leg(image: &mut Image, x: i32, trousers: Color8, boot: Color8, lift: i32) {
    let hip = 16;
    let ankle = 20 - lift;
    draw::fill(image, x, hip, x + 1, ankle, trousers);
    draw::vline(image, x, hip, ankle, TROUSERS_LIGHT);
    draw::hline(image, x, x + 1, hip, trousers);
    draw::hline(image, x, x + 1, ankle + 1 - lift, boot);
    draw::hline(image, x - 1, x + 2, ankle + 2 - lift, boot);
    draw::put(image, x + 1, ankle + 2 - lift, SHADOW);
}
