//! # Noxel Assets
//!
//! The asset pipeline of the Noxel engine: text formats, a complete pure-Rust
//! PNG codec, image manipulation, texture sampling, atlas packing, typed data
//! formats and a hot-reloading asset database. **Zero dependencies, zero
//! `unsafe`.**
//!
//! ## Module map
//!
//! | Module | Purpose |
//! |---|---|
//! | [`json`] | insertion-ordered JSON parser and writer with line/column errors |
//! | [`png`] | PNG decode/encode with a from-scratch inflate and deflate |
//! | [`image`] | [`image::Image`]: the RGBA pixel buffer and sprite authoring |
//! | [`texture`] | [`texture::Texture`]: nearest/bilinear/pixel-art sampling, wrap modes, mips |
//! | [`atlas`] | deterministic shelf packing into a sprite sheet |
//! | [`format`] | the engine's data formats: tile sets, prefabs, palettes, sprites, manifests |
//! | [`db`] | [`db::AssetDb`]: resolution, caching, hot reload, atomic writes |
//!
//! ## The pipeline in one example
//!
//! ```
//! use noxel_asset::atlas::Atlas;
//! use noxel_asset::format::TileSet;
//! use noxel_asset::image::Image;
//! use noxel_asset::json::parse_str;
//! use noxel_core::math::Color8;
//!
//! // 1. Load a tile set definition.
//! let set = TileSet::from_json(&parse_str(
//!     r#"{"name":"ground","texture":"ground.png","tile_size":16,
//!         "tiles":[{"id":0,"name":"grass","uv":[0,0,16,16]}]}"#,
//! )
//! .unwrap())
//! .unwrap();
//! assert_eq!(set.tile(0).unwrap().name, "grass");
//!
//! // 2. Pack loose images into one texture.
//! let entries = vec![
//!     ("grass".to_string(), Image::new(16, 16, Color8::GREEN)),
//!     ("road".to_string(), Image::new(16, 16, Color8::GREY)),
//! ];
//! let atlas = Atlas::build(entries, 1, 128).unwrap();
//!
//! // 3. Sample a region the way the renderer will: at its centre.
//! let region = atlas.region("grass").unwrap();
//! let mid = (region.uv_min + region.uv_max) * 0.5;
//! assert_eq!(atlas.texture().sample_pixel_art(mid.x, mid.y), Color8::GREEN);
//! ```
//!
//! ## Determinism
//!
//! Everything a tool writes is byte-stable: JSON keeps the author's key order,
//! atlas packing sorts by height then name, and PNG encoding is a fixed
//! algorithm. Re-running the generator on unchanged input produces unchanged
//! files, which keeps asset diffs reviewable.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod atlas;
pub mod db;
pub mod format;
pub mod image;
pub mod json;
pub mod png;
pub mod texture;

/// The asset crate version string, taken from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Re-export of the most commonly used items.
///
/// `use noxel_asset::prelude::*;` pulls in the pixel, image and algebra types a
/// caller needs to work with assets, without dragging in the whole codec.
pub mod prelude {
    pub use crate::atlas::{Atlas, AtlasError, SpriteRegion};
    pub use crate::db::{AssetDb, AssetError, AssetStats};
    pub use crate::format::{
        AssetManifest, FormatError, PaletteEntryFile, PaletteFile, Prefab, PrefabProp, PrefabSpawn,
        PrefabVoxel, SpriteFile, SpriteFrame, TileDef, TileFlags, TileSet,
    };
    pub use crate::image::{Image, TILE_16, TILE_32, TILE_64};
    pub use crate::json::{JsonError, JsonValue, parse, parse_str};
    pub use crate::png::{PngError, decode, encode};
    pub use crate::texture::{Texture, WrapMode};
    pub use noxel_core::math::{Color8, Palette, Rect, Vec2, Vec3};
}
