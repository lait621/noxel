//! The fixed 24-colour palette.
//!
//! Every pixel the generator writes comes from [`ENTRIES`]; the palette file in
//! `assets/config/palette.json` is just this list serialised, so the art and the
//! data can never drift apart.
//!
//! The list is deliberately organised as **ramps** rather than as a bag of
//! colours: each material (grass, dirt, stone, water, roof, plaster) owns three
//! neighbouring values — a shadow, a base and a highlight — and every drawing
//! routine stays inside one ramp plus [`SHADOW`]. That is what makes the tiles
//! read as one coherent set instead of as procedural noise.

use noxel_asset::format::{PaletteEntryFile, PaletteFile};
use noxel_core::math::Color8;

/// One palette ramp entry: name, colour and the role it plays.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Stable name, e.g. `"grass_mid"`.
    pub name: &'static str,
    /// The colour.
    pub color: Color8,
    /// What the colour is for, written into the palette file.
    pub note: &'static str,
}

/// Opaque colour shorthand.
const fn c(r: u8, g: u8, b: u8) -> Color8 {
    Color8::new(r, g, b, 255)
}

// --- the ramps, as named constants the drawing code uses -------------------

/// Outline, deepest shade, window and door frames.
pub const SHADOW: Color8 = c(0x17, 0x15, 0x1F);
/// Stone, rock and slate roof shadow.
pub const STONE_DARK: Color8 = c(0x55, 0x5C, 0x66);
/// Stone, cobble, rock and slate roof base.
pub const STONE_MID: Color8 = c(0x86, 0x8D, 0x98);
/// Stone, plaza and slate roof highlight.
pub const STONE_LIGHT: Color8 = c(0xBC, 0xC3, 0xCD);
/// Grass shadow, bush shadow.
pub const GRASS_DARK: Color8 = c(0x2E, 0x6A, 0x32);
/// Grass base.
pub const GRASS_MID: Color8 = c(0x4B, 0x94, 0x40);
/// Grass highlight, grass blades.
pub const GRASS_LIGHT: Color8 = c(0x7C, 0xBE, 0x55);
/// Tree canopy shadow.
pub const LEAF_DARK: Color8 = c(0x23, 0x4F, 0x2B);
/// Tree canopy base.
pub const LEAF_MID: Color8 = c(0x3C, 0x7C, 0x3E);
/// Dirt, wood, plank and trunk shadow.
pub const DIRT_DARK: Color8 = c(0x57, 0x40, 0x2A);
/// Dirt, wood, plank and trunk base.
pub const DIRT_MID: Color8 = c(0x8A, 0x66, 0x40);
/// Dirt and plank highlight; skin shadow.
pub const DIRT_LIGHT: Color8 = c(0xB5, 0x8C, 0x5E);
/// Road joints and cobble shadow.
pub const ROAD_DARK: Color8 = c(0x4B, 0x46, 0x3C);
/// Road cobble base.
pub const ROAD_MID: Color8 = c(0x6E, 0x67, 0x59);
/// Sand base; skin; lamp glow.
pub const SAND_LIGHT: Color8 = c(0xE3, 0xCE, 0x8C);
/// Sand shadow, plaster shadow, floor-stone shading.
pub const PLASTER_DARK: Color8 = c(0xBC, 0xA9, 0x8A);
/// Deep water, night reflection.
pub const WATER_DEEP: Color8 = c(0x1C, 0x4A, 0x69);
/// Mid water, water shadow under a crest.
pub const WATER_MID: Color8 = c(0x2E, 0x70, 0x93);
/// Shallow water, window glass.
pub const WATER_SHALLOW: Color8 = c(0x57, 0xA5, 0xBD);
/// Water sparkle, snow base, plaster highlight.
pub const FOAM: Color8 = c(0xCF, 0xE7, 0xEA);
/// Roof tiles, tunic base, flower petals.
pub const ROOF_RED: Color8 = c(0xA8, 0x44, 0x3A);
/// Roof shadow, brick, tunic shadow.
pub const ROOF_DARK: Color8 = c(0x74, 0x2C, 0x27);
/// Roof highlight, tunic highlight.
pub const ROOF_LIGHT: Color8 = c(0xD4, 0x71, 0x5A);
/// Plaster base, snow highlight, counter top.
pub const PLASTER_MID: Color8 = c(0xE7, 0xD9, 0xBA);

/// The palette, in index order: index == colour id.
pub const ENTRIES: [Entry; 24] = [
    Entry {
        name: "shadow",
        color: SHADOW,
        note: "outlines, deepest shade, window and door frames",
    },
    Entry {
        name: "stone_dark",
        color: STONE_DARK,
        note: "stone, rock and slate-roof shadow",
    },
    Entry {
        name: "stone_mid",
        color: STONE_MID,
        note: "stone, cobble, rock and slate-roof base",
    },
    Entry {
        name: "stone_light",
        color: STONE_LIGHT,
        note: "stone, plaza and slate-roof highlight",
    },
    Entry {
        name: "grass_dark",
        color: GRASS_DARK,
        note: "grass and bush shadow",
    },
    Entry {
        name: "grass_mid",
        color: GRASS_MID,
        note: "grass base",
    },
    Entry {
        name: "grass_light",
        color: GRASS_LIGHT,
        note: "grass highlight and blades",
    },
    Entry {
        name: "leaf_dark",
        color: LEAF_DARK,
        note: "tree canopy shadow",
    },
    Entry {
        name: "leaf_mid",
        color: LEAF_MID,
        note: "tree canopy base",
    },
    Entry {
        name: "dirt_dark",
        color: DIRT_DARK,
        note: "dirt, wood, plank and trunk shadow",
    },
    Entry {
        name: "dirt_mid",
        color: DIRT_MID,
        note: "dirt, wood, plank and trunk base",
    },
    Entry {
        name: "dirt_light",
        color: DIRT_LIGHT,
        note: "dirt and plank highlight, skin shadow",
    },
    Entry {
        name: "road_dark",
        color: ROAD_DARK,
        note: "road joints and cobble shadow",
    },
    Entry {
        name: "road_mid",
        color: ROAD_MID,
        note: "road cobble base",
    },
    Entry {
        name: "sand_light",
        color: SAND_LIGHT,
        note: "sand base, skin, lamp glow",
    },
    Entry {
        name: "plaster_dark",
        color: PLASTER_DARK,
        note: "sand shadow, plaster shadow, floor-stone shading",
    },
    Entry {
        name: "water_deep",
        color: WATER_DEEP,
        note: "deep water and night reflections",
    },
    Entry {
        name: "water_mid",
        color: WATER_MID,
        note: "mid-depth water and the shadow under a crest",
    },
    Entry {
        name: "water_shallow",
        color: WATER_SHALLOW,
        note: "shallow water and window glass",
    },
    Entry {
        name: "foam",
        color: FOAM,
        note: "water sparkle, snow base, plaster highlight",
    },
    Entry {
        name: "roof_red",
        color: ROOF_RED,
        note: "roof tiles, tunic base, flower petals",
    },
    Entry {
        name: "roof_dark",
        color: ROOF_DARK,
        note: "roof shadow, brick, tunic shadow",
    },
    Entry {
        name: "roof_light",
        color: ROOF_LIGHT,
        note: "roof highlight and tunic highlight",
    },
    Entry {
        name: "plaster_mid",
        color: PLASTER_MID,
        note: "plaster base, snow highlight, counter top",
    },
];

/// The palette file that is written to `config/palette.json`.
pub fn file() -> PaletteFile {
    PaletteFile {
        name: "noxel-dawn".to_string(),
        entries: ENTRIES
            .iter()
            .map(|entry| PaletteEntryFile {
                name: entry.name.to_string(),
                color: entry.color,
                description: Some(entry.note.to_string()),
            })
            .collect(),
    }
}

/// True when `color` is one of the 24 palette colours.
///
/// This is the pixel-art guarantee: every generated pixel satisfies it.
#[must_use]
pub fn contains(color: Color8) -> bool {
    index_of(color).is_some()
}

/// The palette index of `color`, if it is present.
#[must_use]
pub fn index_of(color: Color8) -> Option<usize> {
    ENTRIES.iter().position(|entry| entry.color == color)
}
