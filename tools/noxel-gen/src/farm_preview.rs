//! The farm contact sheet: every farm sprite, at `--scale`, on one dark canvas.
//!
//! This exists because the farm's art is judged by eye, and 141 sprites spread
//! over five PNGs is not something a human can look at. The sheet packs every
//! sprite from every sheet into rows — the same greedy row packing a text
//! layout would use — and scales the finished canvas up with nearest-neighbour,
//! which is the only resampling that never invents a colour.
//!
//! Two details are deliberate:
//!
//! * **The background is dark slate, not black.** A near-black backdrop would
//!   swallow the [`SHADOW`] outline that every sprite in this set is finished
//!   with, and the outline is exactly the thing there is to check.
//! * **Rows are bottom-aligned.** A 64x64 farmhouse and a 16x16 rock in the
//!   same row then stand on one ground line, which is how they will sit in the
//!   game.
//!
//! [`SHADOW`]: crate::palette::SHADOW

use noxel_asset::image::Image;

use crate::farm::{self, BACKDROP, BACKDROP_LINE};

/// Clear space between two sprites, in unscaled pixels.
const GUTTER: u32 = 3;
/// Clear space between two sheets, in unscaled pixels.
const SECTION_GAP: u32 = 10;
/// Margin around the whole canvas, in unscaled pixels.
const MARGIN: u32 = 10;
/// The widest a row may grow before it wraps, in unscaled pixels.
const ROW_WIDTH: u32 = 340;

/// One sprite's place in a packed section.
struct Placement {
    /// Index into the section's sprite list.
    index: usize,
    /// Left edge, relative to the section.
    x: u32,
    /// Top edge, relative to the section.
    y: u32,
}

/// One sheet, packed into rows.
struct Section {
    placements: Vec<Placement>,
    width: u32,
    height: u32,
}

/// The contact sheet, composed at 1x and then scaled.
///
/// The result is the same size for the same art whatever the platform: the only
/// arithmetic is integer, and the one resample is nearest-neighbour.
#[must_use]
pub fn sheet(scale: u32) -> Image {
    let scale = scale.max(1);
    let sections: Vec<Vec<(String, Image)>> = farm::sheets()
        .into_iter()
        .map(|sheet| sheet.sprites)
        .collect();
    let packed: Vec<Section> = sections.iter().map(|sprites| pack(sprites)).collect();

    let width = MARGIN * 2
        + packed
            .iter()
            .map(|section| section.width)
            .max()
            .unwrap_or(0);
    let height = MARGIN * 2
        + packed.iter().map(|section| section.height).sum::<u32>()
        + SECTION_GAP * (packed.len().saturating_sub(1) as u32);

    let mut canvas = Image::new(width, height, BACKDROP);
    let mut y = MARGIN;
    for (index, section) in packed.iter().enumerate() {
        if index > 0 {
            // A one-pixel rule between sheets, so a reader can tell where one
            // atlas ends and the next begins.
            let rule = y - SECTION_GAP / 2;
            for x in 0..width {
                canvas.set(x, rule, BACKDROP_LINE);
            }
        }
        for placement in &section.placements {
            let (_, image) = &sections[index][placement.index];
            canvas.blit(
                image,
                (MARGIN + placement.x) as i32,
                (y + placement.y) as i32,
            );
        }
        y += section.height + SECTION_GAP;
    }
    canvas.scale_nearest(width * scale, height * scale)
}

/// Packs one sheet's sprites into rows of at most [`ROW_WIDTH`] pixels.
///
/// Rows are filled greedily in authoring order — which is the contract's order,
/// so the sheet reads in the same sequence as the region lists — and every row
/// is as tall as its tallest sprite, with the sprites standing on its bottom
/// edge.
fn pack(sprites: &[(String, Image)]) -> Section {
    let mut rows: Vec<(usize, usize)> = Vec::new(); // (first index, count)
    let mut row_width = 0;
    for (index, (_, image)) in sprites.iter().enumerate() {
        let cell = image.width + GUTTER;
        let wraps = row_width > 0 && row_width + image.width > ROW_WIDTH;
        if rows.is_empty() || wraps {
            rows.push((index, 1));
            row_width = cell;
        } else {
            let last = rows.len() - 1;
            rows[last].1 += 1;
            row_width += cell;
        }
    }

    let mut placements = Vec::with_capacity(sprites.len());
    let mut width = 0;
    let mut y = 0;
    for (first, count) in rows {
        let row = &sprites[first..first + count];
        let row_height = row.iter().map(|(_, image)| image.height).max().unwrap_or(0);
        let mut x = 0;
        for (offset, (_, image)) in row.iter().enumerate() {
            placements.push(Placement {
                index: first + offset,
                x,
                // Bottom-aligned in the row: one ground line for the section.
                y: y + row_height - image.height,
            });
            x += image.width + GUTTER;
        }
        width = width.max(x.saturating_sub(GUTTER));
        y += row_height + GUTTER;
    }
    Section {
        placements,
        width,
        height: y.saturating_sub(GUTTER),
    }
}
