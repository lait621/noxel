//! The contact sheet: every generated texture, scaled up, in one PNG.
//!
//! The sheet is the tool's visual smoke test. It reads the textures back from
//! disk (so it shows what is actually being shipped, not what the generator
//! believes it wrote), scales each one with nearest-neighbour — which never
//! invents a colour — and stacks them top to bottom.
//!
//! The layout is fixed and deliberately trivial: the sheet is as wide as the
//! widest scaled texture and as tall as all of them together, with no padding
//! and no separators, so its size is a pure function of the texture sizes.

use std::path::Path;

use noxel_asset::image::Image;
use noxel_asset::png;

use crate::error::{Error, Result};
use crate::{buildings, characters, props, terrain};

/// The textures on the sheet, top to bottom, relative to the asset root.
pub const TEXTURES: [&str; 4] = [
    "textures/terrain.png",
    "textures/props.png",
    "textures/characters.png",
    "textures/buildings.png",
];

/// The size of the sheet `scale` times up, computed from the in-memory art.
#[must_use]
pub fn sheet_size(scale: u32) -> (u32, u32) {
    let sources = [
        terrain::atlas(),
        props::atlas(),
        characters::sheet(),
        buildings::atlas(),
    ];
    let width = sources
        .iter()
        .map(|image| image.width().saturating_mul(scale))
        .max()
        .unwrap_or(0);
    let height = sources
        .iter()
        .map(|image| image.height().saturating_mul(scale))
        .fold(0u32, |total, value| total.saturating_add(value));
    (width, height)
}

/// Builds the contact sheet from the textures under `out`.
pub fn contact_sheet(out: &Path, scale: u32) -> Result<Image> {
    let mut scaled = Vec::with_capacity(TEXTURES.len());
    for name in TEXTURES {
        let path = out.join(name);
        let bytes = Error::read(&path, "a texture")?;
        let image = png::decode(&bytes).map_err(|error| {
            Error::invalid(format!("decoding {}", path.display()), error.to_string())
        })?;
        scaled.push(image.scale_nearest(
            image.width().saturating_mul(scale),
            image.height().saturating_mul(scale),
        ));
    }

    let width = scaled.iter().map(Image::width).max().unwrap_or(0);
    let height = scaled
        .iter()
        .map(Image::height)
        .fold(0u32, |total, value| total.saturating_add(value));
    let mut sheet = Image::transparent(width, height);
    let mut y = 0u32;
    for image in &scaled {
        sheet.stamp(image, 0, y as i32);
        y = y.saturating_add(image.height());
    }
    Ok(sheet)
}

/// Renders the sheet and writes it to `<out>/preview.png`, returning its size.
///
/// The rendered size is checked against [`sheet_size`], which is computed from
/// the in-memory art: a mismatch means the textures on disk are not the ones
/// the generator draws, which is worth reporting rather than shipping.
pub fn write(out: &Path, scale: u32) -> Result<(u32, u32)> {
    let sheet = contact_sheet(out, scale)?;
    let expected = sheet_size(scale);
    if (sheet.width(), sheet.height()) != expected {
        return Err(Error::invalid(
            "preview",
            format!(
                "the contact sheet is {}x{} but the generated textures are {expected:?} at scale {scale}",
                sheet.width(),
                sheet.height()
            ),
        ));
    }
    let path = out.join("preview.png");
    Error::write(&path, "the contact sheet", &png::encode(&sheet))?;
    Ok((sheet.width(), sheet.height()))
}
