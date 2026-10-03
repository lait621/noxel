//! # Noxel World
//!
//! Procedural generation, chunk streaming, roads, towns and the queries the
//! rest of the engine asks about the world. **Zero dependencies beyond
//! `noxel-core` and `noxel-asset`, zero `unsafe`.**
//!
//! ## The one rule
//!
//! A chunk is a **pure function of `(config.seed, chunk_pos)`**. Nothing here
//! keeps a shared mutable random number generator, so chunk `(900, -400)`
//! generates identically whether its neighbours were visited or not, on this
//! machine or another. Every stage draws from its own addressable
//! [`noxel_core::rng::RngStream`], and every world query —
//! [`WorldGenerator::sample_height`], [`WorldGenerator::biome_at`],
//! [`WorldGenerator::is_road_at`] — answers from the same fields the chunks are
//! built from, at any coordinate, without generating anything.
//!
//! ## Module map
//!
//! | Module | Purpose |
//! |---|---|
//! | [`biome`] | [`BiomeId`], [`Biome`], [`BiomeTable`]: the fixed biome table |
//! | [`chunk`] | [`Chunk`]: tiles, heights, slopes, colliders, props, buildings |
//! | `gen` (module `r#gen`) | [`WorldConfig`], [`WorldGenerator`], [`GenStats`]: the generator |
//! | [`road`] | [`RoadSegment`], [`RoadNetwork`]: the macro lattice |
//! | [`town`] | [`TownPlan`], [`TownStyle`], [`BuildingPlot`], [`BuildingInstance`] |
//! | [`stream`] | [`WorldStreamer`], [`StreamStats`]: the LRU chunk cache |
//!
//! ## Example
//!
//! ```
//! use std::sync::Arc;
//! use noxel_asset::format::TileSet;
//! use noxel_core::math::{ChunkPos, Vec3};
//! use noxel_world::{WorldConfig, WorldGenerator, WorldStreamer};
//!
//! // A world with no tile definitions at all still generates: every tile
//! // resolves to the documented fallback instead of panicking.
//! let set = Arc::new(TileSet {
//!     name: "ground".into(),
//!     texture: "ground.png".into(),
//!     tile_size: 16,
//!     tiles: Vec::new(),
//! });
//! let generator = WorldGenerator::new(WorldConfig::new(7), set, Vec::new());
//!
//! let height = generator.sample_height(120.0, -80.0);
//! assert!(height.is_finite());
//!
//! let mut streamer = WorldStreamer::new(generator);
//! streamer.update(Vec3::ZERO);
//! assert_eq!(streamer.loaded_chunks(), 13 * 13);
//! assert!(streamer.chunk(ChunkPos::ZERO).is_some());
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod biome;
pub mod chunk;
/// The generator module.
///
/// The module is declared with a raw identifier because `gen` became a
/// reserved keyword in Rust 2024; `noxel_world::r#gen` is the path, and the
/// crate root re-exports everything in it.
pub mod r#gen;
pub mod road;
pub mod stream;
pub mod town;

pub use biome::{Biome, BiomeId, BiomeTable};
pub use chunk::{Chunk, ChunkPos as WorldChunkPos, PropInstance, PropKind};
pub use r#gen::{GenStats, WorldConfig, WorldGenerator};
pub use road::{RoadNetwork, RoadSegment};
pub use stream::{StreamStats, WorldStreamer};
pub use town::{BuildingInstance, BuildingPlot, Facing, TownPlan, TownStyle};

use noxel_asset::format::TileSet;

/// Writes a chunk as text so a human can diff generated worlds.
///
/// The format is line-oriented and byte-stable: the same chunk always produces
/// the same string, so a golden-world test can compare two runs (or two
/// machines) with a plain string equality. It is used by `tools/noxel-gen` and
/// by the golden-world tests.
///
/// Layout:
///
/// ```text
/// chunk <x> <y> biome=<name> town=<0|1> tiles=<n> roads=<r> buildings=<b> props=<p>
/// legend <char>=<id>:<name> ...
/// tiles
/// <n rows of n characters, row-major, y * n + x>
/// heights
/// <n rows of n decimetre integers>
/// colliders <count> occluders <count>
/// ```
///
/// Tiles are rendered as one character per tile from a per-chunk legend, so the
/// body stays narrow enough to read in a terminal. Heights are printed in
/// decimetres: a diff of two worlds then shows metre changes as character
/// changes rather than as float noise.
#[must_use]
pub fn chunk_to_string(chunk: &Chunk, set: &TileSet) -> String {
    use core::fmt::Write as _;
    use std::collections::BTreeMap;

    let side = chunk.tiles();
    let mut out = String::with_capacity(side as usize * (side as usize * 8 + 40) + 256);

    // Legend: tile id -> printable character, assigned in ascending id order so
    // it is stable regardless of where a tile appears in the chunk.
    let mut ids: Vec<u32> = chunk.tiles.clone();
    ids.sort_unstable();
    ids.dedup();
    let alphabet: Vec<char> =
        ".oO#*+xX~=:-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
            .chars()
            .collect();
    let mut legend: BTreeMap<u32, char> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        legend.insert(*id, alphabet[i % alphabet.len()]);
    }

    let road_count = chunk.roads.iter().filter(|road| road.is_main).count();
    let _ = writeln!(
        out,
        "chunk {} {} biome={} town={} tiles={} roads={} buildings={} props={}",
        chunk.pos.x,
        chunk.pos.y,
        chunk.biome.name(),
        u8::from(chunk.has_town),
        side,
        road_count,
        chunk.buildings.len(),
        chunk.props.len(),
    );

    out.push_str("legend");
    for (id, ch) in &legend {
        let name = set.tile(*id).map_or("?", |tile| tile.name.as_str());
        let _ = write!(out, " {ch}={id}:{name}");
    }
    out.push('\n');

    out.push_str("tiles\n");
    if side == 0 {
        out.push('\n');
    } else {
        for y in 0..side {
            for x in 0..side {
                out.push(*legend.get(&chunk.tile(x, y)).unwrap_or(&'?'));
            }
            out.push('\n');
        }
    }

    out.push_str("heights\n");
    if side == 0 {
        out.push('\n');
    } else {
        for y in 0..side {
            for x in 0..side {
                let h = chunk.height(x, y);
                let decimetres = if h.is_finite() {
                    (h * 10.0).round() as i64
                } else {
                    0
                };
                let _ = write!(out, "{decimetres:>5}");
            }
            out.push('\n');
        }
    }

    let occluders = chunk.occluder_boxes().count();
    let _ = writeln!(
        out,
        "colliders {} occluders {}",
        chunk.colliders.len(),
        occluders
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_asset::format::{TileDef, TileFlags};
    use noxel_core::math::ChunkPos;
    use std::sync::Arc;

    fn set() -> Arc<TileSet> {
        Arc::new(TileSet {
            name: "ground".into(),
            texture: "ground.png".into(),
            tile_size: 16,
            tiles: vec![
                TileDef {
                    id: 0,
                    name: "grass".into(),
                    texture: String::new(),
                    uv: [0, 0, 16, 16],
                    flags: TileFlags::default(),
                    height: 0.0,
                    layer: 0,
                },
                TileDef {
                    id: 1,
                    name: "road".into(),
                    texture: String::new(),
                    uv: [16, 0, 16, 16],
                    flags: TileFlags {
                        road: true,
                        ..TileFlags::default()
                    },
                    height: 0.0,
                    layer: 0,
                },
            ],
        })
    }

    fn generator(seed: u64) -> WorldGenerator {
        WorldGenerator::new(WorldConfig::new(seed), set(), Vec::new())
    }

    #[test]
    fn chunk_to_string_is_stable() {
        let g = generator(3);
        let chunk = g.generate_chunk(ChunkPos::new(1, -1));
        let a = chunk_to_string(&chunk, g.tile_set());
        let b = chunk_to_string(&chunk, g.tile_set());
        assert_eq!(a, b);
        let other = g.generate_chunk(ChunkPos::new(1, -1));
        assert_eq!(a, chunk_to_string(&other, g.tile_set()));
    }

    #[test]
    fn chunk_to_string_has_the_documented_sections() {
        let g = generator(4);
        let chunk = g.generate_chunk(ChunkPos::new(0, 0));
        let text = chunk_to_string(&chunk, g.tile_set());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("chunk 0 0 biome="));
        assert!(lines[1].starts_with("legend "));
        assert_eq!(lines[2], "tiles");
        assert_eq!(lines[3].chars().count(), 32);
        assert_eq!(lines[3 + 32], "heights");
        assert_eq!(lines[3 + 33].split_whitespace().count(), 32);
        let last = lines.last().unwrap();
        assert!(last.starts_with("colliders "), "{last}");
        // Header, legend, "tiles", 32 rows, "heights", 32 rows, footer.
        assert_eq!(lines.len(), 1 + 1 + 1 + 32 + 1 + 32 + 1);
    }

    #[test]
    fn chunk_to_string_marks_roads_and_towns() {
        let g = generator(5);
        let spacing = g.config().town_spacing_chunks;
        let chunk = g.generate_chunk(ChunkPos::new(spacing, spacing));
        let text = chunk_to_string(&chunk, g.tile_set());
        assert!(text.contains("town=1"));
        assert!(text.contains("road"));
    }

    #[test]
    fn chunk_to_string_handles_an_empty_chunk() {
        let empty = Chunk::empty(ChunkPos::new(2, 2), 0, 1.0, BiomeId::LAKE);
        let text = chunk_to_string(&empty, &set());
        assert!(text.contains("tiles=0"));
        assert!(text.starts_with("chunk 2 2 biome=lake"));
    }

    #[test]
    fn chunk_to_string_does_not_panic_on_unknown_tiles() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), 2, 1.0, BiomeId::PLAINS);
        chunk.tiles = vec![999, 0, 1, 999];
        let text = chunk_to_string(&chunk, &set());
        assert!(text.contains("999:?"));
        assert_eq!(text.lines().count(), 1 + 1 + 1 + 2 + 1 + 2 + 1);
    }

    #[test]
    fn different_seeds_dump_differently() {
        let a = generator(1).generate_chunk(ChunkPos::ZERO);
        let b = generator(2).generate_chunk(ChunkPos::ZERO);
        let set = set();
        assert_ne!(chunk_to_string(&a, &set), chunk_to_string(&b, &set));
    }

    #[test]
    fn two_generation_orders_agree() {
        // The headline determinism test: the same chunk, reached by two
        // different routes through the world, is byte-identical.
        let g = generator(99);
        let target = ChunkPos::new(900, -400);
        let first = chunk_to_string(&g.generate_chunk(target), g.tile_set());
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-5, 7),
            target,
            ChunkPos::new(12, 12),
            target,
        ] {
            let _ = g.generate_chunk(pos);
        }
        let second = chunk_to_string(&g.generate_chunk(target), g.tile_set());
        assert_eq!(first, second);
    }
}
