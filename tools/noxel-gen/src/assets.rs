//! The asset set: every file the generator owns, built in memory.
//!
//! [`build`] is the single source of truth. `generate` writes what it returns,
//! `verify` compares it against the disk, `list` prints it — so the three
//! subcommands can never disagree about what the asset tree should contain.
//!
//! Everything is a pure function of the seed: no timestamps, no iteration over
//! a hash map, no clock and no system randomness anywhere in the pipeline.

use std::path::Path;

use noxel_asset::format::AssetManifest;
use noxel_asset::json::JsonValue;
use noxel_asset::png;

use crate::error::{Error, Result};
use crate::prefabs::NAMES as PREFAB_NAMES;
use crate::{buildings, characters, palette, prefabs, props, sprites, terrain, tilesets, world};

/// The manifest's file name, relative to the asset root.
pub const MANIFEST: &str = "manifest.json";

/// One generated file: where it goes, what it is and its exact bytes.
#[derive(Clone, Debug)]
pub struct AssetFile {
    /// Path relative to the asset root, always with `/` separators.
    pub path: String,
    /// What kind of asset it is: `palette`, `texture`, `tileset`, `prefab`,
    /// `sprite`, `world`, `doc` or `manifest`.
    pub kind: &'static str,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// What `generate` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files written (or rewritten).
    pub written: usize,
    /// Files left alone because their bytes already matched.
    pub skipped: usize,
    /// Total bytes of the generated set.
    pub bytes: usize,
}

impl Summary {
    /// The one-line report `generate` prints.
    #[must_use]
    pub fn line(&self, out: &Path) -> String {
        format!(
            "noxel-gen: {} written, {} skipped, {} bytes -> {}",
            self.written,
            self.skipped,
            self.bytes,
            out.display()
        )
    }
}

/// What `verify` checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifySummary {
    /// Files compared.
    pub files: usize,
    /// Total bytes compared.
    pub bytes: usize,
}

/// Builds every asset in memory, in write order.
pub fn build(seed: u64) -> Result<Vec<AssetFile>> {
    let mut files = Vec::new();

    // 1. The palette everything else is drawn from.
    files.push(AssetFile {
        path: "config/palette.json".to_string(),
        kind: "palette",
        bytes: json_bytes(&palette::file().to_json()),
    });

    // 2. The textures. Every pixel is checked against the palette as it is
    //    encoded: the pixel-art guarantee is enforced here, not only in tests.
    files.push(png_file("textures/terrain.png", &terrain::atlas())?);
    files.push(png_file("textures/props.png", &props::atlas())?);
    files.push(png_file("textures/characters.png", &characters::sheet())?);
    files.push(png_file("textures/buildings.png", &buildings::atlas())?);

    // 3. The tile sets.
    files.push(AssetFile {
        path: "tilesets/terrain.json".to_string(),
        kind: "tileset",
        bytes: json_bytes(&tilesets::terrain_set().to_json()),
    });
    files.push(AssetFile {
        path: "tilesets/buildings.json".to_string(),
        kind: "tileset",
        bytes: json_bytes(&tilesets::buildings_set().to_json()),
    });

    // 4. The prefabs.
    for prefab in prefabs::all()? {
        files.push(AssetFile {
            path: format!("prefabs/{}.json", prefab.name),
            kind: "prefab",
            bytes: json_bytes(&prefabs::to_json(&prefab)),
        });
    }

    // 5. The character sprite sheet description.
    files.push(AssetFile {
        path: "sprites/characters.json".to_string(),
        kind: "sprite",
        bytes: json_bytes(&sprites::characters_json()),
    });

    // 6. The demo world and its schema. The world file is parsed back and
    //    validated before it is accepted, so a broken configuration cannot be
    //    written just because the generator produced it.
    let demo = json_bytes(&world::demo(seed));
    let parsed = noxel_asset::json::parse(&demo)
        .map_err(|error| Error::invalid("world/demo.json", error.to_string()))?;
    world::validate(&parsed).map_err(|message| Error::invalid("world/demo.json", message))?;
    files.push(AssetFile {
        path: "world/demo.json".to_string(),
        kind: "world",
        bytes: demo,
    });
    files.push(AssetFile {
        path: "world/README.md".to_string(),
        kind: "doc",
        bytes: world::readme().into_bytes(),
    });

    // 7. The directory guide.
    files.push(AssetFile {
        path: "README.md".to_string(),
        kind: "doc",
        bytes: readme().into_bytes(),
    });

    // 8. The manifest, which lists everything above *and itself*, so that the
    //    file list in the asset tree and the file list in the manifest are the
    //    same set by construction.
    let mut entries: Vec<(String, &'static str)> = files
        .iter()
        .map(|file| (file.path.clone(), file.kind))
        .collect();
    entries.push((MANIFEST.to_string(), "manifest"));
    entries.sort();
    files.push(AssetFile {
        path: MANIFEST.to_string(),
        kind: "manifest",
        bytes: json_bytes(&manifest(&entries)),
    });

    Ok(files)
}

/// Writes the asset set to `out`.
///
/// A file whose bytes already match is left untouched, so a rebuild does not
/// touch timestamps and does not invalidate a hot-reloading editor. `force`
/// rewrites everything.
pub fn generate(out: &Path, seed: u64, force: bool) -> Result<Summary> {
    let files = build(seed)?;
    let mut summary = Summary::default();
    for file in &files {
        summary.bytes += file.bytes.len();
        let path = out.join(&file.path);
        if !force && std::fs::read(&path).is_ok_and(|existing| existing == file.bytes) {
            summary.skipped += 1;
            continue;
        }
        Error::write(&path, &file.path, &file.bytes)?;
        summary.written += 1;
    }
    Ok(summary)
}

/// Regenerates in memory and compares every file against the disk.
///
/// Prints one line per difference and returns [`Error::Mismatch`] when anything
/// differs, so `verify` is usable as a CI gate.
pub fn verify(out: &Path, seed: u64) -> Result<VerifySummary> {
    let files = build(seed)?;
    let mut summary = VerifySummary::default();
    let mut mismatches = 0usize;
    for file in &files {
        summary.files += 1;
        summary.bytes += file.bytes.len();
        let path = out.join(&file.path);
        match std::fs::read(&path) {
            Ok(existing) if existing == file.bytes => {}
            Ok(existing) => {
                println!(
                    "  differs: {} ({} bytes on disk, {} generated)",
                    file.path,
                    existing.len(),
                    file.bytes.len()
                );
                mismatches += 1;
            }
            Err(_) => {
                println!("  missing: {}", file.path);
                mismatches += 1;
            }
        }
    }
    if mismatches > 0 {
        return Err(Error::Mismatch { count: mismatches });
    }
    Ok(summary)
}

/// The seed recorded in `<out>/world/demo.json`, or `None` when there is not a
/// readable world file to take it from.
///
/// `verify` has no `--seed` flag: a world generated with a custom seed must
/// still verify against itself, so the seed is recovered from the world it
/// describes and only falls back to the default when there is nothing to read.
#[must_use]
pub fn seed_from_disk(out: &Path) -> Option<u64> {
    let bytes = std::fs::read(out.join("world/demo.json")).ok()?;
    let json = noxel_asset::json::parse(&bytes).ok()?;
    let seed = json.get("seed")?.as_f64()?;
    if seed.is_finite() && seed >= 0.0 && seed <= u64::MAX as f64 {
        Some(seed as u64)
    } else {
        None
    }
}

/// The `list` report: every file, its kind and its size.
#[must_use]
pub fn listing(seed: u64) -> Result<String> {
    let files = build(seed)?;
    let total: usize = files.iter().map(|file| file.bytes.len()).sum();
    let mut text = format!("noxel-gen: {} files, {} bytes\n", files.len(), total);
    for file in &files {
        text.push_str(&format!(
            "  {:>8}  {:<30}  {}\n",
            file.bytes.len(),
            file.path,
            file.kind
        ));
    }
    Ok(text)
}

/// Encodes an image as a PNG asset, rejecting any pixel outside the palette.
fn png_file(path: &str, image: &noxel_asset::image::Image) -> Result<AssetFile> {
    for pixel in &image.pixels {
        // A fully transparent pixel is a hole, not a colour.
        if pixel.a != 0 && !palette::contains(*pixel) {
            return Err(Error::invalid(
                path,
                format!(
                    "pixel {pixel:?} is outside the {} palette colours",
                    palette::ENTRIES.len()
                ),
            ));
        }
    }
    Ok(AssetFile {
        path: path.to_string(),
        kind: "texture",
        bytes: png::encode(image),
    })
}

/// Renders JSON the way every text asset in the tree is written: two-space
/// indentation and a trailing newline.
fn json_bytes(value: &JsonValue) -> Vec<u8> {
    let mut text = value.to_string_pretty();
    text.push('\n');
    text.into_bytes()
}

/// Builds the manifest: the engine's [`AssetManifest`] lists plus the complete
/// file index.
fn manifest(entries: &[(String, &'static str)]) -> JsonValue {
    let collect = |kind: &str| -> Vec<String> {
        entries
            .iter()
            .filter(|(_, entry_kind)| *entry_kind == kind)
            .map(|(path, _)| path.clone())
            .collect()
    };
    let manifest = AssetManifest {
        name: "town-demo".to_string(),
        version: "1".to_string(),
        tile_sets: collect("tileset"),
        prefabs: collect("prefab"),
        palettes: collect("palette"),
        sprites: collect("sprite"),
    };

    let files = JsonValue::Array(
        entries
            .iter()
            .map(|(path, kind)| {
                JsonValue::object([
                    ("path", JsonValue::from(path.as_str())),
                    ("kind", JsonValue::from(*kind)),
                ])
            })
            .collect(),
    );

    match manifest.to_json() {
        JsonValue::Object(mut fields) => {
            fields.push(("files".to_string(), files));
            JsonValue::Object(fields)
        }
        other => other,
    }
}

/// The text of `assets/README.md`.
#[must_use]
pub fn readme() -> String {
    let mut text = String::new();
    text.push_str(
        "# Noxel demo assets\n\n\
         **Generated by `noxel-gen`. Do not hand-edit anything in this directory** —\n\
         every file here is overwritten on the next run, and `noxel-gen verify` fails\n\
         the build when a file no longer matches the generator.\n\n\
         ```sh\n\
         cargo run -p noxel-gen -- generate --force   # rewrite the whole tree\n\
         cargo run -p noxel-gen -- verify             # check it against the generator\n\
         cargo run -p noxel-gen -- preview            # contact sheet of the textures\n\
         cargo run -p noxel-gen -- list               # what is generated, and how big\n\
         ```\n\n\
         ## Units\n\n\
         One tile is 16x16 pixels, one world unit and one metre. A character cell is\n\
         16x24 pixels and stands 1.0 x 1.5 world units. All art is authored against the\n\
         24-colour palette in `config/palette.json`; index == colour id, and no pixel\n\
         outside that list is ever written.\n\n\
         ## config/\n\n\
         Data files. `palette.json` is the fixed 24-colour palette, in index order:\n\
         colour id 0 is `shadow`, id 4 is `grass_dark`, and so on. Every texture in\n\
         this tree is drawn only from these colours, and every colour is used by at\n\
         least one material ramp.\n\n\
         ## textures/\n\n\
         PNG atlases, all nearest-neighbour pixel art with hard alpha (no partial\n\
         coverage anywhere). `terrain.png` is 16 ground tiles of 16x16 in a single row,\n\
         `buildings.png` is 12 wall/roof/floor tiles of 16x16 in a single row,\n\
         `characters.png` is a 5x4 grid of 16x24 cells (four walk frames per direction\n\
         plus an idle cell, rows in the order down/left/right/up), and `props.png` is a\n\
         single 24-pixel-tall row of prop cells: three 24x24 tree-canopy frames followed\n\
         by 11 cells of 16x16.\n\n\
         ## tilesets/\n\n\
         Tile definitions: id, name, `uv` region, gameplay flags and stack height. Tile\n\
         ids are unique across both files (terrain 0-15, buildings 16-27), so a prefab\n\
         voxel can name any tile with one number. `uv` regions are in texture pixels and\n\
         never overlap.\n\n\
         ## prefabs/\n\n\
         Building blocks for the world generator: a `voxels` list (grid position, tile\n\
         id and the tile name for readability), `props`, `spawns` (a door, a shopkeeper,\n\
         a patrol point) and `occluders`. Sizes are in tiles: `house_small` is 5x4 tiles\n\
         and three tiles tall. Every prefab is structurally validated by the generator:\n\
         no voxel outside its bounds, no two voxels in the same cell, and every building\n\
         has a door on its perimeter with a walkable floor behind it.\n\n\
         ## sprites/\n\n\
         Sprite sheet descriptions: the texture to read, the default frame, the pivot,\n\
         the display size in world units and the frame list. `characters.json` also\n\
         carries an `animations` index that groups the frames into `walk_down`,\n\
         `idle_up` and the rest, with each animation's loop flag and duration.\n\n\
         ## world/\n\n\
         The demo world configuration the `town-demo` example reads: seed, size, biome\n\
         hints, towns, roads and named points of interest. See `world/README.md` for the\n\
         schema. Units are tiles, and one tile is one metre.\n\n",
    );

    text.push_str("## Tile ids\n\n| Id | Name | Set | Walkable | Blocks sight | Water | Road |\n");
    text.push_str("|---|---|---|---|---|---|---|\n");
    for tile in tilesets::all() {
        let flags = match tile.set {
            "terrain" => tilesets::terrain_flags(tile.name),
            _ => tilesets::building_flags(tile.name),
        };
        text.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | {} |\n",
            tile.id,
            tile.name,
            tile.set,
            yes_no(flags.walkable),
            yes_no(flags.blocks_sight),
            yes_no(flags.water),
            yes_no(flags.road),
        ));
    }

    text.push_str("\n## Prop cells\n\n");
    text.push_str("| Cell | x | y | w | h |\n|---|---|---|---|---|\n");
    for cell in &props::CELLS {
        text.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            cell.name, cell.x, cell.y, cell.w, cell.h
        ));
    }

    text.push_str("\n## Prefabs\n\n");
    text.push_str("| Prefab | Size (x, y, z) | Tags |\n|---|---|---|\n");
    if let Ok(all) = prefabs::all() {
        for prefab in &all {
            text.push_str(&format!(
                "| `{}` | {}x{}x{} | {} |\n",
                prefab.name,
                prefab.size[0],
                prefab.size[1],
                prefab.size[2],
                prefab.tags.join(", ")
            ));
        }
    }

    text.push_str(&format!(
        "\n## Files\n\n{} generated files. The manifest at `manifest.json` lists every one\n\
         of them with its kind, including the manifest itself; the prefab order above is\n\
         the order in `manifest.json`.\n",
        PREFAB_NAMES.len()
    ));

    text
}

/// `yes` / `no` for the README tables.
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
