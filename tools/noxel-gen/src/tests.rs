//! The generator's test suite.
//!
//! Four things are pinned down here, in this order: the command line and the
//! small pure helpers; the art contract (palette-only pixels, water and roads
//! that tile, sprites that read); the structural contract (tile sets, prefabs,
//! the world schema); and the tool contract — determinism, idempotence and
//! `verify`, which is what makes the whole tree safe to regenerate.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use noxel_asset::format::{PaletteFile, Prefab, SpriteFile, TileSet};
use noxel_asset::image::Image;
use noxel_asset::json::{self, JsonValue};
use noxel_asset::png;
use noxel_core::math::Color8;

use crate::characters::Pose;
use crate::{
    DEFAULT_SEED, assets, buildings, characters, cli, draw, palette, prefabs, preview, props,
    sprites, terrain, tilesets, world,
};

// --- helpers ---------------------------------------------------------------

/// A scratch directory that removes itself when the test ends.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a uniquely named directory under the system temp directory.
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noxel-gen-{}-{}-{}",
            std::process::id(),
            unique,
            tag
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create the test directory");
        Self { path }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Every regular file under `root`, as `/`-separated paths relative to it.
fn walk(root: &Path) -> Vec<String> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort();
    files
}

/// The generated file with `path`, or a panic naming the missing one.
fn file<'a>(files: &'a [assets::AssetFile], path: &str) -> &'a assets::AssetFile {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("no generated file `{path}`"))
}

/// Parses a generated JSON file.
fn json_of(file: &assets::AssetFile) -> JsonValue {
    json::parse(&file.bytes).unwrap_or_else(|error| panic!("{}: {error}", file.path))
}

/// The pixels of a generated texture, indexed by name.
fn texture(file: &assets::AssetFile) -> Image {
    png::decode(&file.bytes).unwrap_or_else(|error| panic!("{}: {error}", file.path))
}

/// Counts the pixels of `color` inside an inclusive rectangle.
fn count_color(image: &Image, x0: u32, y0: u32, x1: u32, y1: u32, color: Color8) -> usize {
    let mut count = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if image.get(x, y) == Some(color) {
                count += 1;
            }
        }
    }
    count
}

/// The rows of `image` that contain at least one opaque pixel.
fn opaque_rows(image: &Image) -> Vec<u32> {
    (0..image.height())
        .filter(|y| (0..image.width()).any(|x| draw::opaque(image, x, *y)))
        .collect()
}

/// The columns of `image` that contain at least one opaque pixel.
fn opaque_columns(image: &Image) -> Vec<u32> {
    (0..image.width())
        .filter(|x| (0..image.height()).any(|y| draw::opaque(image, *x, y)))
        .collect()
}

/// The command line from a list of arguments.
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_string()).collect()
}

// --- command line ----------------------------------------------------------

#[test]
fn cli_defaults_to_the_demo_asset_root() {
    let command = cli::parse(&args(&["generate"])).expect("generate parses");
    match command {
        cli::Command::Generate { out, seed, force } => {
            assert!(
                out.ends_with("examples/town-demo/assets"),
                "{}",
                out.display()
            );
            assert_eq!(seed, DEFAULT_SEED);
            assert!(!force);
        }
        other => panic!("expected generate, got {other:?}"),
    }
}

#[test]
fn cli_reads_every_generate_flag() {
    let command = cli::parse(&args(&[
        "generate",
        "--out",
        "/tmp/noxel",
        "--seed=7",
        "--force",
    ]))
    .expect("generate parses");
    assert_eq!(
        command,
        cli::Command::Generate {
            out: PathBuf::from("/tmp/noxel"),
            seed: 7,
            force: true,
        }
    );
}

#[test]
fn cli_reads_the_other_subcommands() {
    assert_eq!(
        cli::parse(&args(&["verify", "--out=/tmp/x"])).expect("verify parses"),
        cli::Command::Verify {
            out: PathBuf::from("/tmp/x")
        }
    );
    assert_eq!(
        cli::parse(&args(&["preview", "--scale", "6"])).expect("preview parses"),
        cli::Command::Preview {
            out: cli::default_out_dir().expect("workspace root"),
            scale: 6,
        }
    );
    assert_eq!(
        cli::parse(&args(&["preview"])).expect("preview parses"),
        cli::Command::Preview {
            out: cli::default_out_dir().expect("workspace root"),
            scale: cli::DEFAULT_PREVIEW_SCALE,
        }
    );
    assert_eq!(
        cli::parse(&args(&["list"])).expect("list parses"),
        cli::Command::List
    );
}

#[test]
fn cli_help_is_recognised() {
    assert_eq!(
        cli::parse(&args(&["--help"])).expect("help"),
        cli::Command::Help
    );
    assert_eq!(
        cli::parse(&args(&["-h"])).expect("help"),
        cli::Command::Help
    );
    assert_eq!(
        cli::parse(&args(&["help"])).expect("help"),
        cli::Command::Help
    );
    assert!(cli::USAGE.contains("noxel-gen generate"));
}

#[test]
fn cli_rejects_an_empty_command_line() {
    let error = cli::parse(&args(&[])).expect_err("no subcommand is an error");
    assert!(error.to_string().contains("no subcommand"), "{error}");
}

#[test]
fn cli_rejects_unknown_options_and_subcommands() {
    assert!(cli::parse(&args(&["generate", "--nope"])).is_err());
    assert!(cli::parse(&args(&["frobnicate"])).is_err());
    assert!(cli::parse(&args(&["generate", "--seed"])).is_err());
    assert!(cli::parse(&args(&["generate", "--force=yes"])).is_err());
}

#[test]
fn cli_rejects_malformed_values() {
    assert!(cli::parse(&args(&["generate", "--seed", "twelve"])).is_err());
    assert!(cli::parse(&args(&["generate", "--seed", "-1"])).is_err());
    assert!(cli::parse(&args(&["preview", "--scale", "0"])).is_err());
    assert!(cli::parse(&args(&["preview", "--scale", "999"])).is_err());
}

#[test]
fn cli_rejects_flags_that_belong_to_another_subcommand() {
    assert!(cli::parse(&args(&["generate", "--scale", "2"])).is_err());
    assert!(cli::parse(&args(&["verify", "--force"])).is_err());
    assert!(cli::parse(&args(&["verify", "--scale", "2"])).is_err());
    assert!(cli::parse(&args(&["preview", "--force"])).is_err());
    assert!(cli::parse(&args(&["list", "--out", "/tmp/x", "--force"])).is_err());
}

#[test]
fn the_workspace_root_is_the_noxel_checkout() {
    let root = cli::workspace_root().expect("the workspace root is found");
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("the manifest is readable");
    assert!(manifest.contains("[workspace]"));
    assert!(root.join("tools/noxel-gen").is_dir());
}

// --- palette ---------------------------------------------------------------

#[test]
fn the_palette_has_24_named_colours() {
    assert_eq!(palette::ENTRIES.len(), 24);
    for entry in &palette::ENTRIES {
        assert!(!entry.name.is_empty());
        assert!(!entry.note.is_empty());
        assert_eq!(entry.color.a, 255, "`{}` must be opaque", entry.name);
    }
}

#[test]
fn palette_names_are_unique() {
    for (index, entry) in palette::ENTRIES.iter().enumerate() {
        for other in &palette::ENTRIES[index + 1..] {
            assert_ne!(entry.name, other.name, "duplicate palette name");
        }
    }
}

#[test]
fn palette_colours_are_unique() {
    for (index, entry) in palette::ENTRIES.iter().enumerate() {
        assert_eq!(palette::index_of(entry.color), Some(index));
    }
}

#[test]
fn palette_lookup_round_trips() {
    for (index, entry) in palette::ENTRIES.iter().enumerate() {
        assert!(palette::contains(entry.color));
        assert_eq!(palette::index_of(entry.color), Some(index));
    }
    assert!(!palette::contains(Color8::rgb(1, 2, 3)));
    assert_eq!(palette::index_of(Color8::TRANSPARENT), None);
}

#[test]
fn the_palette_file_round_trips() {
    let file = palette::file();
    assert_eq!(file.entries.len(), 24);
    assert_eq!(file.name, "noxel-dawn");
    let written = file.to_json();
    assert_eq!(PaletteFile::from_json(&written).expect("parses"), file);
    let text = written.to_string_pretty();
    assert_eq!(
        PaletteFile::from_json(&json::parse_str(&text).expect("parses")).expect("parses"),
        file
    );
    let engine = file.to_palette();
    assert_eq!(engine.len(), 24);
    assert_eq!(engine.find("grass_mid"), Some(palette::GRASS_MID));
    assert_eq!(engine.find("water_deep"), Some(palette::WATER_DEEP));
    assert_eq!(engine.find("roof_red"), Some(palette::ROOF_RED));
    assert_eq!(engine.find("road_dark"), Some(palette::ROAD_DARK));
}

// --- drawing primitives ----------------------------------------------------

#[test]
fn drawing_primitives_clip_instead_of_panicking() {
    let mut image = Image::transparent(4, 4);
    draw::put(&mut image, -5, -5, palette::SHADOW);
    draw::put(&mut image, 99, 99, palette::SHADOW);
    draw::hline(&mut image, -10, 2, 0, palette::GRASS_MID);
    draw::vline(&mut image, 0, -10, 1, palette::GRASS_MID);
    draw::fill(&mut image, draw::Area::new(-3, -3, 1, 1), palette::DIRT_MID);
    draw::frame(
        &mut image,
        draw::Area::new(-2, -2, 5, 5),
        palette::STONE_MID,
    );
    draw::scatter(
        &mut image,
        draw::Area::new(-4, -4, 8, 8),
        palette::FOAM,
        50,
        1,
    );
    draw::bevel(&mut image, palette::FOAM, palette::SHADOW);
    draw::clear(&mut image, 100, 100);
    assert_eq!(image.pixel_count(), 16);
    // The bevel ran last and owns the rim; nothing escaped the 4x4 buffer.
    assert_eq!(image.get(0, 0), Some(palette::FOAM));
    assert_eq!(image.get(3, 3), Some(palette::SHADOW));
}

#[test]
fn outline_only_paints_transparent_neighbours() {
    let mut sprite = Image::transparent(7, 7);
    draw::fill(&mut sprite, draw::Area::new(2, 2, 4, 4), palette::ROOF_RED);
    let before = sprite.clone();
    draw::outline(&mut sprite, palette::SHADOW);
    // The body is untouched...
    for y in 2..=4 {
        for x in 2..=4 {
            assert_eq!(sprite.get(x, y), before.get(x, y));
        }
    }
    // ... and the ring around it is now the outline colour.
    assert_eq!(sprite.get(1, 1), Some(palette::SHADOW));
    assert_eq!(sprite.get(5, 5), Some(palette::SHADOW));
    assert_eq!(
        sprite.get(0, 0),
        Some(Color8::TRANSPARENT),
        "far corners stay clear"
    );
    assert_eq!(
        sprite.get(6, 0),
        Some(Color8::TRANSPARENT),
        "far corners stay clear"
    );
}

#[test]
fn bevel_puts_light_on_top_and_shadow_at_the_bottom() {
    let mut tile = Image::new(8, 8, palette::GRASS_MID);
    draw::bevel(&mut tile, palette::GRASS_LIGHT, palette::GRASS_DARK);
    // The convention is "lit from above, shadowed below", dithered so the rim
    // does not read as a flat bar.
    let top = count_color(&tile, 0, 0, 7, 0, palette::GRASS_LIGHT);
    let bottom = count_color(&tile, 0, 7, 7, 7, palette::GRASS_DARK);
    assert!(top >= 4, "the top edge catches the light (got {top}/8)");
    assert!(
        bottom >= 4,
        "the bottom edge falls into shadow (got {bottom}/8)"
    );
    assert_eq!(count_color(&tile, 1, 1, 6, 6, palette::GRASS_LIGHT), 0);
    assert_eq!(count_color(&tile, 1, 1, 6, 6, palette::GRASS_DARK), 0);
}

#[test]
fn the_hash_is_deterministic_and_stays_in_range() {
    for x in -40..40 {
        for y in -40..40 {
            let value = draw::hash01(x, y, 99);
            assert!((0.0..1.0).contains(&value));
            assert_eq!(value, draw::hash01(x, y, 99));
        }
    }
    assert_ne!(draw::hash01(1, 2, 3), draw::hash01(2, 1, 3));
}

// --- terrain ---------------------------------------------------------------

#[test]
fn the_terrain_atlas_holds_every_tile_side_by_side() {
    let atlas = terrain::atlas();
    assert_eq!(atlas.width(), terrain::SIZE * 16);
    assert_eq!(atlas.height(), terrain::SIZE);
    for (column, name) in terrain::NAMES.iter().enumerate() {
        let tile = terrain::tile(name).unwrap_or_else(|| panic!("{name} is missing"));
        for y in 0..terrain::SIZE {
            for x in 0..terrain::SIZE {
                assert_eq!(
                    atlas.get(column as u32 * terrain::SIZE + x, y),
                    tile.get(x, y),
                    "`{name}` does not sit at column {column}"
                );
            }
        }
    }
    assert!(terrain::tile("nope").is_none());
}

#[test]
fn terrain_tile_names_are_unique() {
    for (index, name) in terrain::NAMES.iter().enumerate() {
        assert_eq!(terrain::index_of(name), Some(index));
    }
    assert_eq!(terrain::NAMES.len(), 16);
}

#[test]
fn ground_tiles_are_opaque_and_textured() {
    for name in terrain::NAMES {
        let tile = terrain::tile(name).expect("every named tile exists");
        assert!(draw::is_opaque(&tile), "`{name}` has holes");
        assert!(
            draw::distinct_colors(&tile) >= 3,
            "`{name}` is a flat block ({} colours)",
            draw::distinct_colors(&tile)
        );
    }
}

#[test]
fn water_wraps_seamlessly_in_both_axes() {
    for deep in [false, true] {
        for y in 0..terrain::SIZE as i32 {
            for x in 0..terrain::SIZE as i32 {
                let base = terrain::water_pixel(x, y, deep);
                assert_eq!(terrain::water_pixel(x + 16, y, deep), base, "x seam");
                assert_eq!(terrain::water_pixel(x, y + 16, deep), base, "y seam");
                assert_eq!(terrain::water_pixel(x - 16, y, deep), base, "x seam (left)");
                assert_eq!(terrain::water_pixel(x, y - 16, deep), base, "y seam (up)");
            }
        }
    }
}

#[test]
fn a_water_tile_is_exactly_its_field() {
    for (name, deep) in [("water_shallow", false), ("water_deep", true)] {
        let tile = terrain::tile(name).expect("the water tile exists");
        for y in 0..terrain::SIZE as i32 {
            for x in 0..terrain::SIZE as i32 {
                assert_eq!(
                    tile.get(x as u32, y as u32),
                    Some(terrain::water_pixel(x, y, deep))
                );
            }
            // The pixel just past the right edge continues the tile's first
            // column, which is what a neighbouring tile draws there.
            assert_eq!(
                terrain::water_pixel(16, y, deep),
                tile.get(0, y as u32).expect("in bounds"),
                "column 16 does not continue column 0"
            );
        }
    }
}

#[test]
fn deep_and_shallow_water_are_different_art() {
    let shallow = terrain::tile("water_shallow").expect("tile");
    let deep = terrain::tile("water_deep").expect("tile");
    assert!(shallow != deep, "deep and shallow water are the same art");
    let shallow_pixels = count_color(&shallow, 0, 0, 15, 15, palette::WATER_SHALLOW);
    let deep_pixels = count_color(&deep, 0, 0, 15, 15, palette::WATER_DEEP);
    assert!(shallow_pixels > 20, "shallow water needs its own tone");
    assert!(deep_pixels > 20, "deep water needs its own tone");
    assert!(
        count_color(&deep, 0, 0, 15, 15, palette::WATER_SHALLOW) < shallow_pixels / 4,
        "deep water must not be as bright as the shallows"
    );
}

#[test]
fn the_road_wraps_and_joins_in_all_four_directions() {
    for y in 0..terrain::SIZE as i32 {
        for x in 0..terrain::SIZE as i32 {
            let base = terrain::road_pixel(x, y);
            assert_eq!(terrain::road_pixel(x + 16, y), base, "east join");
            assert_eq!(terrain::road_pixel(x - 16, y), base, "west join");
            assert_eq!(terrain::road_pixel(x, y + 16), base, "south join");
            assert_eq!(terrain::road_pixel(x, y - 16), base, "north join");
        }
    }
    // The joints sit exactly on the cobble grid...
    for i in 0..terrain::SIZE as i32 {
        assert_eq!(
            terrain::road_pixel(0, i),
            palette::ROAD_DARK,
            "no joint at x = 0"
        );
        assert_eq!(
            terrain::road_pixel(i, 0),
            palette::ROAD_DARK,
            "no joint at y = 0"
        );
        for step in [4, 8, 12] {
            assert_eq!(terrain::road_pixel(step, i), palette::ROAD_DARK);
            assert_eq!(terrain::road_pixel(i, step), palette::ROAD_DARK);
        }
        // ... and the last row and column of a tile are cobble, so two tiles
        // meeting produce one joint rather than a double-width cross. (A pixel
        // on a joint row or column is a joint in the other axis by design, so
        // those are skipped.)
        if i % 4 != 0 {
            assert_ne!(
                terrain::road_pixel(15, i),
                palette::ROAD_DARK,
                "double joint at x = 15"
            );
            assert_ne!(
                terrain::road_pixel(i, 15),
                palette::ROAD_DARK,
                "double joint at y = 15"
            );
        }
    }
}

#[test]
fn the_road_edge_has_a_verge_and_a_kerb() {
    let tile = terrain::tile("road_edge").expect("tile");
    // The top rows are grass, the bottom rows are paving: the boundary between
    // them is what makes this an edge tile.
    for x in 0..terrain::SIZE {
        let top = tile.get(x, 0).expect("in bounds");
        assert!(
            [
                palette::GRASS_MID,
                palette::GRASS_DARK,
                palette::GRASS_LIGHT
            ]
            .contains(&top),
            "row 0 should be grass"
        );
        let bottom = tile.get(x, 15).expect("in bounds");
        assert!(
            [
                palette::ROAD_MID,
                palette::ROAD_DARK,
                palette::STONE_MID,
                palette::STONE_LIGHT
            ]
            .contains(&bottom),
            "row 15 should be paving"
        );
    }
    assert_eq!(
        tile.get(0, 4),
        Some(palette::STONE_LIGHT),
        "the kerb is lit"
    );
}

#[test]
fn the_road_edge_paving_lines_up_with_the_road() {
    let tile = terrain::tile("road_edge").expect("tile");
    for y in 6..terrain::SIZE {
        for x in 0..terrain::SIZE {
            assert_eq!(
                tile.get(x, y),
                Some(terrain::road_pixel(x as i32, y as i32)),
                "the kerb tile must use the same cobble field"
            );
        }
    }
}

// --- props -----------------------------------------------------------------

#[test]
fn the_prop_sheet_lays_out_every_cell() {
    let sheet = props::atlas();
    assert_eq!(sheet.height(), props::HEIGHT);
    let expected_width = props::CELLS
        .iter()
        .map(|cell| cell.x + cell.w)
        .max()
        .expect("cells");
    assert_eq!(sheet.width(), expected_width);
    assert_eq!(props::CELLS.len(), 14);
    for cell in &props::CELLS {
        assert!(cell.h <= props::HEIGHT, "`{}` is too tall", cell.name);
        let image = props::region(cell.name).unwrap_or_else(|| panic!("{}", cell.name));
        assert_eq!((image.width(), image.height()), (cell.w, cell.h));
        for y in 0..cell.h {
            for x in 0..cell.w {
                assert_eq!(
                    sheet.get(cell.x + x, cell.y + y),
                    image.get(x, y),
                    "`{}` is misplaced on the sheet",
                    cell.name
                );
            }
        }
    }
    assert!(props::region("nothing").is_none());
}

#[test]
fn prop_cells_are_inside_the_sheet_and_do_not_overlap() {
    let sheets = props::CELLS;
    for (index, cell) in sheets.iter().enumerate() {
        assert!(cell.x + cell.w <= props::atlas().width());
        assert!(cell.y + cell.h <= props::HEIGHT);
        for other in &sheets[index + 1..] {
            let disjoint = cell.x + cell.w <= other.x
                || other.x + other.w <= cell.x
                || cell.y + cell.h <= other.y
                || other.y + other.h <= cell.y;
            assert!(disjoint, "`{}` overlaps `{}`", cell.name, other.name);
        }
    }
}

#[test]
fn props_are_bottom_aligned_and_outlined() {
    for cell in &props::CELLS {
        if cell.name.starts_with("tree_canopy") {
            continue;
        }
        assert_eq!(
            cell.y + cell.h,
            props::HEIGHT,
            "`{}` must stand on the sheet's ground line",
            cell.name
        );
    }
    for name in ["barrel", "crate", "bush", "rock_small"] {
        let image = props::region(name).expect("the prop exists");
        assert!(!draw::is_opaque(&image), "`{name}` fills its whole cell");
        // An outline means the sprite's edge is the darkest palette colour.
        let rows = opaque_rows(&image);
        assert!(!rows.is_empty(), "`{name}` is empty");
        let columns = opaque_columns(&image);
        assert!(!columns.is_empty(), "`{name}` is empty");
        assert!(
            image.pixels.contains(&palette::SHADOW),
            "`{name}` has no outline"
        );
    }
}

#[test]
fn the_tree_canopy_sways_between_its_frames() {
    let frames: Vec<Image> = (0..3)
        .map(|frame| props::region(&format!("tree_canopy_{frame}")).expect("a canopy frame"))
        .collect();
    assert!(
        frames[0] != frames[1],
        "canopy frames 0 and 1 are identical"
    );
    assert!(
        frames[1] != frames[2],
        "canopy frames 1 and 2 are identical"
    );
    // The sway is a one-pixel shift, so each frame covers the same area.
    for frame in &frames {
        assert_eq!(frame.width(), props::CANOPY);
        assert_eq!(frame.height(), props::CANOPY);
        assert!(draw::distinct_colors(frame) >= 3);
    }
}

// --- characters ------------------------------------------------------------

#[test]
fn the_character_sheet_is_a_five_by_four_grid() {
    let sheet = characters::sheet();
    assert_eq!(sheet.width(), characters::COLS * characters::CELL_W);
    assert_eq!(sheet.height(), characters::ROWS * characters::CELL_H);
    assert_eq!(characters::DIRECTIONS.len(), 4);
    for (row, direction) in characters::DIRECTIONS.iter().enumerate() {
        for column in 0..characters::COLS as usize {
            let uv = characters::cell_uv(direction, column).expect("a cell");
            assert_eq!(uv[0], column as u32 * characters::CELL_W);
            assert_eq!(uv[1], row as u32 * characters::CELL_H);
            let pose = if column == characters::IDLE_COLUMN {
                Pose::Idle
            } else {
                Pose::Walk(column)
            };
            let cell = characters::character(direction, pose).expect("a character cell");
            for y in 0..characters::CELL_H {
                for x in 0..characters::CELL_W {
                    assert_eq!(
                        sheet.get(uv[0] + x, uv[1] + y),
                        cell.get(x, y),
                        "`{direction}` column {column} is misplaced"
                    );
                }
            }
        }
    }
    assert!(characters::cell_uv("sideways", 0).is_none());
    assert!(characters::cell_uv("down", 99).is_none());
    assert!(characters::character("sideways", Pose::Idle).is_none());
}

#[test]
fn every_character_cell_has_a_readable_figure() {
    for direction in characters::DIRECTIONS {
        for column in 0..characters::COLS as usize {
            let pose = if column == characters::IDLE_COLUMN {
                Pose::Idle
            } else {
                Pose::Walk(column)
            };
            let cell = characters::character(direction, pose).expect("a cell");
            assert!(
                draw::distinct_colors(&cell) >= 5,
                "`{direction}` cell {column} is too plain"
            );
            let rows = opaque_rows(&cell);
            assert!(rows.len() >= 20, "`{direction}` cell {column} is too short");
            // The outline may reach the very edge of the cell, but the figure
            // must leave transparent pixels around itself.
            assert!(
                !draw::is_opaque(&cell),
                "`{direction}` cell {column} is clipped to its cell"
            );
            let empty_columns = (0..characters::CELL_W)
                .filter(|x| !(0..characters::CELL_H).any(|y| draw::opaque(&cell, *x, y)))
                .count();
            assert!(
                empty_columns >= 2,
                "`{direction}` cell {column} has no horizontal margin"
            );
        }
    }
}

#[test]
fn the_walk_cycle_alternates_its_legs() {
    // On the shin row, the transparent run between the two legs is the stride:
    // wide on the contact frames, narrow while the legs pass each other.
    let gap = |frame: usize| -> usize {
        let cell = characters::character("down", Pose::Walk(frame)).expect("a frame");
        let row = 19;
        let legs: Vec<u32> = (0..characters::CELL_W)
            .filter(|x| draw::opaque(&cell, *x, row))
            .collect();
        let first = *legs.first().expect("a leg");
        let last = *legs.last().expect("a leg");
        (first..=last)
            .filter(|x| !draw::opaque(&cell, *x, row))
            .count()
    };
    assert!(
        gap(0) > gap(1),
        "the legs are further apart on the contact frame ({} vs {})",
        gap(0),
        gap(1)
    );
    assert!(
        gap(2) > gap(3),
        "the second contact frame mirrors the first"
    );
    let stance = characters::character("down", Pose::Walk(0)).expect("frame 0");
    let passing = characters::character("down", Pose::Walk(1)).expect("frame 1");
    assert!(
        stance != passing,
        "the contact and passing frames are identical"
    );
}

#[test]
fn the_body_bobs_by_one_pixel_on_the_passing_frames() {
    let contact = characters::character("down", Pose::Walk(0)).expect("frame 0");
    let passing = characters::character("down", Pose::Walk(1)).expect("frame 1");
    let contact_top = *opaque_rows(&contact).first().expect("rows");
    let passing_top = *opaque_rows(&passing).first().expect("rows");
    assert_eq!(
        contact_top,
        passing_top + 1,
        "the body rises exactly one pixel while the legs pass"
    );
}

#[test]
fn all_four_walk_frames_differ() {
    let frames: Vec<Image> = (0..characters::WALK_FRAMES)
        .map(|frame| characters::character("left", Pose::Walk(frame)).expect("a frame"))
        .collect();
    for (index, frame) in frames.iter().enumerate() {
        for other in &frames[index + 1..] {
            assert!(frame != other, "two walk frames are identical");
        }
    }
}

#[test]
fn the_four_directions_are_distinguishable() {
    let down = characters::character("down", Pose::Idle).expect("down");
    let up = characters::character("up", Pose::Idle).expect("up");
    let left = characters::character("left", Pose::Idle).expect("left");
    let right = characters::character("right", Pose::Idle).expect("right");

    // The face: two pixels of eye looking down, none looking away.
    let face = |image: &Image| count_color(image, 5, 6, 10, 9, palette::SHADOW);
    assert_eq!(face(&down), 2, "the down pose needs two eyes");
    assert_eq!(face(&up), 0, "the up pose must not show a face");
    assert_eq!(face(&left), 1, "a profile shows one eye");

    // Right is left mirrored, which is what keeps the pair symmetrical.
    assert!(
        left.flip_x() == right,
        "the right pose is not the left one mirrored"
    );
    assert!(down != up, "the down and up poses are identical");
    assert!(down != left, "the down and left poses are identical");
}

#[test]
fn the_idle_pose_is_not_a_walk_frame() {
    for direction in characters::DIRECTIONS {
        let idle = characters::character(direction, Pose::Idle).expect("idle");
        for frame in 0..characters::WALK_FRAMES {
            let walk = characters::character(direction, Pose::Walk(frame)).expect("walk");
            assert!(
                idle != walk,
                "`{direction}` idle matches walk frame {frame}"
            );
        }
    }
}

// --- buildings -------------------------------------------------------------

#[test]
fn the_buildings_atlas_holds_every_tile() {
    let atlas = buildings::atlas();
    assert_eq!(atlas.width(), buildings::SIZE * 12);
    assert_eq!(atlas.height(), buildings::SIZE);
    assert_eq!(buildings::NAMES.len(), 12);
    for (column, name) in buildings::NAMES.iter().enumerate() {
        let tile = buildings::tile(name).unwrap_or_else(|| panic!("{name} is missing"));
        assert_eq!(buildings::index_of(name), Some(column));
        for y in 0..buildings::SIZE {
            for x in 0..buildings::SIZE {
                assert_eq!(
                    atlas.get(column as u32 * buildings::SIZE + x, y),
                    tile.get(x, y)
                );
            }
        }
    }
    assert!(buildings::tile("nope").is_none());
}

#[test]
fn building_tiles_are_opaque_and_textured() {
    for name in buildings::NAMES {
        let tile = buildings::tile(name).expect("every named tile exists");
        assert!(draw::is_opaque(&tile), "`{name}` has holes");
        assert!(
            draw::distinct_colors(&tile) >= 3,
            "`{name}` is a flat block"
        );
    }
}

#[test]
fn the_window_is_glazed_and_the_door_is_floored() {
    let window = buildings::tile("wall_window").expect("window");
    assert!(count_color(&window, 0, 0, 15, 15, palette::WATER_SHALLOW) > 15);
    assert!(count_color(&window, 0, 0, 15, 15, palette::FOAM) > 10);
    assert!(count_color(&window, 0, 0, 15, 15, palette::WATER_MID) > 10);
    let door = buildings::tile("wall_door").expect("door");
    assert!(count_color(&door, 0, 0, 15, 15, palette::DIRT_MID) > 40);
    assert!(
        count_color(&door, 0, 0, 15, 15, palette::SAND_LIGHT) >= 2,
        "a handle"
    );
}

// --- tilesets --------------------------------------------------------------

#[test]
fn the_terrain_tileset_describes_every_tile() {
    let set = tilesets::terrain_set();
    assert_eq!(set.texture, "textures/terrain.png");
    assert_eq!(set.tile_size, terrain::SIZE);
    assert_eq!(set.len(), 16);
    for (index, name) in terrain::NAMES.iter().enumerate() {
        let tile = set
            .tile_by_name(name)
            .unwrap_or_else(|| panic!("{name} is missing"));
        assert_eq!(tile.id, index as u32);
        assert_eq!(tile.uv, [index as u32 * 16, 0, 16, 16]);
        assert!(tile.texture.is_empty(), "tiles use the set's texture");
        assert_eq!(set.texture_of(tile), "textures/terrain.png");
    }
}

#[test]
fn the_buildings_tileset_describes_every_tile() {
    let set = tilesets::buildings_set();
    assert_eq!(set.texture, "textures/buildings.png");
    assert_eq!(set.len(), 12);
    for (index, name) in buildings::NAMES.iter().enumerate() {
        let tile = set
            .tile_by_name(name)
            .unwrap_or_else(|| panic!("{name} is missing"));
        assert_eq!(tile.id, tilesets::BUILDINGS_BASE_ID + index as u32);
        assert_eq!(tile.uv, [index as u32 * 16, 0, 16, 16]);
    }
}

#[test]
fn tileset_uv_regions_are_inside_their_texture_and_do_not_overlap() {
    let cases = [
        (
            tilesets::terrain_set(),
            terrain::atlas().width(),
            terrain::atlas().height(),
        ),
        (
            tilesets::buildings_set(),
            buildings::atlas().width(),
            buildings::atlas().height(),
        ),
    ];
    for (set, width, height) in cases {
        for (index, tile) in set.tiles.iter().enumerate() {
            let [x, y, w, h] = tile.uv;
            assert!(
                x + w <= width && y + h <= height,
                "`{}` is outside its texture",
                tile.name
            );
            for other in &set.tiles[index + 1..] {
                let [ox, oy, ow, oh] = other.uv;
                let disjoint = x + w <= ox || ox + ow <= x || y + h <= oy || oy + oh <= y;
                assert!(disjoint, "`{}` overlaps `{}`", tile.name, other.name);
            }
        }
    }
}

#[test]
fn the_terrain_flags_follow_the_gameplay_contract() {
    let set = tilesets::terrain_set();
    let flags = |name: &str| set.tile_by_name(name).expect("tile").flags;
    for name in [
        "grass",
        "grass_dark",
        "grass_flowers",
        "dirt",
        "dirt_dark",
        "sand",
        "snow",
    ] {
        assert!(flags(name).walkable, "`{name}` must be walkable");
        assert!(flags(name).buildable, "`{name}` must be buildable");
        assert!(!flags(name).blocks_sight);
    }
    for name in ["rock", "rock_dark", "cliff"] {
        assert!(!flags(name).walkable);
        assert!(flags(name).blocks_sight);
        assert!(flags(name).occluder);
    }
    for name in ["water_shallow", "water_deep"] {
        assert!(flags(name).water, "`{name}` must be water");
        assert!(!flags(name).walkable);
        assert!(!flags(name).road);
    }
    for name in ["road", "road_edge", "plaza", "bridge"] {
        assert!(flags(name).road, "`{name}` must be a road");
        assert!(flags(name).walkable);
    }
    assert_eq!(set.tile_by_name("cliff").expect("cliff").height, 2.0);
    assert_eq!(set.tile_by_name("bridge").expect("bridge").layer, 1);
}

#[test]
fn the_building_flags_follow_the_gameplay_contract() {
    let set = tilesets::buildings_set();
    let flags = |name: &str| set.tile_by_name(name).expect("tile").flags;
    for name in [
        "wall_plaster",
        "wall_wood",
        "wall_stone",
        "wall_window",
        "roof_red",
        "roof_slate",
        "roof_edge",
        "chimney",
    ] {
        assert!(flags(name).blocks_sight, "`{name}` must block sight");
        assert!(!flags(name).walkable, "`{name}` must not be walkable");
    }
    assert!(!flags("wall_door").blocks_sight, "a doorway is see-through");
    assert!(flags("wall_door").walkable);
    for name in ["floor_wood", "floor_stone"] {
        assert!(flags(name).walkable, "`{name}` must be walkable");
        assert!(!flags(name).blocks_sight);
    }
    assert!(!flags("counter").walkable);
    assert!(!flags("counter").blocks_sight);
}

#[test]
fn tile_ids_are_unique_across_both_sets() {
    let tiles = tilesets::all();
    assert_eq!(tiles.len(), 28);
    for (index, tile) in tiles.iter().enumerate() {
        assert_eq!(tilesets::find(tile.name), Some(*tile));
        assert_eq!(tilesets::find_id(tile.id), Some(*tile));
        assert_eq!(tilesets::name_of(tile.id), Some(tile.name));
        assert!(tilesets::flags_of(tile.id).is_some());
        for other in &tiles[index + 1..] {
            assert_ne!(tile.id, other.id, "duplicate tile id");
            assert_ne!(tile.name, other.name, "duplicate tile name");
        }
    }
    assert_eq!(tilesets::find("nothing"), None);
    assert_eq!(tilesets::find_id(999), None);
    assert_eq!(tilesets::flags_of(999), None);
    assert_eq!(tilesets::TERRAIN_BASE_ID, 0);
    assert_eq!(tilesets::BUILDINGS_BASE_ID, 16);
}

#[test]
fn both_tilesets_round_trip_through_json() {
    for set in [tilesets::terrain_set(), tilesets::buildings_set()] {
        let written = set.to_json();
        assert_eq!(TileSet::from_json(&written).expect("parses"), set);
        let text = written.to_string_pretty();
        assert_eq!(
            TileSet::from_json(&json::parse_str(&text).expect("parses")).expect("parses"),
            set
        );
    }
}

#[test]
fn every_tileset_texture_is_generated() {
    let files = assets::build(DEFAULT_SEED).expect("the assets build");
    for set in [tilesets::terrain_set(), tilesets::buildings_set()] {
        let texture = file(&files, &set.texture);
        assert_eq!(texture.kind, "texture");
        assert!(png::decode(&texture.bytes).is_ok());
    }
}

// --- sprites ---------------------------------------------------------------

#[test]
fn the_sprite_file_round_trips() {
    let sprite = sprites::characters_file();
    assert_eq!(sprite.name, "characters");
    assert_eq!(sprite.texture, "textures/characters.png");
    assert_eq!(sprite.frames.len(), 20);
    assert_eq!(sprite.pivot, [0.5, 1.0]);
    assert_eq!(sprite.size, sprites::SIZE);

    let json = sprites::characters_json();
    let parsed = SpriteFile::from_json(&json).expect("parses");
    assert_eq!(
        parsed, sprite,
        "the extra `animations` key must not break parsing"
    );
}

#[test]
fn sprite_frames_cover_the_sheet() {
    let sprite = sprites::characters_file();
    let (width, height) = (characters::sheet().width(), characters::sheet().height());
    for (index, frame) in sprite.frames.iter().enumerate() {
        let [x, y, w, h] = frame.uv;
        assert!(
            x + w <= width && y + h <= height,
            "`{}` is outside",
            frame.name
        );
        assert_eq!((w, h), (characters::CELL_W, characters::CELL_H));
        assert!(frame.duration > 0.0);
        for other in &sprite.frames[index + 1..] {
            let [ox, oy, ow, oh] = other.uv;
            let disjoint = x + w <= ox || ox + ow <= x || y + h <= oy || oy + oh <= y;
            assert!(disjoint, "`{}` overlaps `{}`", frame.name, other.name);
        }
    }
}

#[test]
fn every_animation_is_complete() {
    let json = sprites::characters_json();
    let sprite = sprites::characters_file();
    let animations = json
        .get("animations")
        .and_then(JsonValue::as_array)
        .expect("the file lists its animations");
    assert_eq!(animations.len(), 8);
    let mut names: Vec<&str> = Vec::new();
    for animation in animations {
        let name = animation.get_str("name").expect("an animation name");
        names.push(name);
        let frames = animation
            .get("frames")
            .and_then(JsonValue::as_array)
            .expect("an animation frame list");
        assert!(!frames.is_empty());
        for frame in frames {
            let frame = frame.as_str().expect("a frame name");
            assert!(
                sprite.frame(frame).is_some(),
                "`{name}` names missing frame `{frame}`"
            );
            assert!(sprite.frame(frame).expect("frame").duration > 0.0);
        }
        assert!(animation.get_bool("loop", false));
        assert!(animation.get_f32("duration", 0.0) > 0.0);
    }
    for direction in characters::DIRECTIONS {
        assert!(names.contains(&format!("walk_{direction}").as_str()));
        assert!(names.contains(&format!("idle_{direction}").as_str()));
    }
    assert!(sprites::characters_json().get("frames").is_some());
}

// --- prefabs ---------------------------------------------------------------

/// Builds a prefab and runs the shared structural validation over it.
fn valid(name: &str) -> Prefab {
    let prefab = prefabs::build(name).unwrap_or_else(|error| panic!("{name}: {error}"));
    prefabs::validate(&prefab).unwrap_or_else(|message| panic!("{name}: {message}"));
    prefab
}

#[test]
fn the_prefab_list_is_complete_and_unique() {
    assert_eq!(prefabs::NAMES.len(), 12);
    for (index, name) in prefabs::NAMES.iter().enumerate() {
        for other in &prefabs::NAMES[index + 1..] {
            assert_ne!(name, other);
        }
    }
    assert_eq!(prefabs::all().expect("all prefabs build").len(), 12);
    assert!(prefabs::build("nope").is_err());
}

#[test]
fn prefab_house_small_is_valid() {
    let prefab = valid("house_small");
    assert_eq!(prefab.size, [5, 3, 4]);
    assert!(prefab.has_tag("building"));
    assert_eq!(
        prefab.voxel(2, 1, 0),
        tilesets::find("wall_door").map(|tile| tile.id)
    );
    assert!(prefab.spawn("door").is_some());
    assert!(!prefab.props.is_empty());
    assert!(!prefab.occluders.is_empty());
}

#[test]
fn prefab_house_large_is_valid() {
    let prefab = valid("house_large");
    assert_eq!(prefab.size, [7, 4, 5]);
    let chimney = tilesets::find("chimney").expect("chimney").id;
    assert!(prefab.voxels.iter().any(|voxel| voxel.tile == chimney));
    // Two storeys: walls at y = 1 and y = 2, a roof at y = 3.
    assert!(prefab.voxel(0, 2, 2).is_some());
    assert_eq!(prefab.spawns.len(), 3);
}

#[test]
fn prefab_shop_is_valid() {
    let prefab = valid("shop");
    assert_eq!(prefab.size, [6, 3, 5]);
    let counter = tilesets::find("counter").expect("counter").id;
    assert_eq!(
        prefab
            .voxels
            .iter()
            .filter(|voxel| voxel.tile == counter)
            .count(),
        4
    );
    assert!(prefab.spawn("shopkeeper").is_some());
    assert!(prefab.props.iter().any(|prop| prop.kind == "sign"));
}

#[test]
fn prefab_inn_is_valid() {
    let prefab = valid("inn");
    assert_eq!(prefab.size, [8, 4, 6]);
    assert!(prefab.has_tag("commercial"));
    assert!(prefab.spawn("guard").is_some());
    assert!(prefab.spawn("innkeeper").is_some());
}

#[test]
fn prefab_barn_is_valid() {
    let prefab = valid("barn");
    assert_eq!(prefab.size, [7, 4, 5]);
    let door = tilesets::find("wall_door").expect("door").id;
    assert_eq!(
        prefab
            .voxels
            .iter()
            .filter(|voxel| voxel.tile == door)
            .count(),
        2,
        "a barn has a double door"
    );
    assert!(prefab.has_tag("farm"));
}

#[test]
fn prefab_well_is_valid() {
    let prefab = valid("well");
    assert_eq!(prefab.size, [3, 1, 3]);
    let water = tilesets::find("water_deep").expect("water").id;
    assert_eq!(prefab.voxel(1, 0, 1), Some(water));
    assert!(prefab.spawn("water").is_some());
    assert_eq!(prefab.props[0].kind, "well");
}

#[test]
fn prefab_fence_segment_is_valid() {
    let prefab = valid("fence_segment");
    assert_eq!(prefab.size, [4, 1, 1]);
    assert_eq!(prefab.props.len(), 4);
    assert!(prefab.props.iter().all(|prop| prop.kind == "fence_h"));
    assert_eq!(prefab.voxels.len(), 4);
}

#[test]
fn prefab_tree_pine_is_valid() {
    let prefab = valid("tree_pine");
    assert_eq!(prefab.size, [1, 1, 1]);
    assert!(prefab.has_tag("tree"));
    assert_eq!(prefab.props.len(), 2);
    assert!(prefab.props.iter().any(|prop| prop.kind == "tree_canopy_0"));
    assert!(prefab.spawn("chop").is_some());
}

#[test]
fn prefab_tree_oak_is_valid() {
    let prefab = valid("tree_oak");
    assert_eq!(prefab.props.len(), 2);
    let canopy = prefab
        .props
        .iter()
        .find(|prop| prop.kind == "tree_canopy_2")
        .expect("an oak canopy");
    assert!(canopy.scale > 1.0, "the oak is the bigger tree");
}

#[test]
fn prefab_rock_cluster_is_valid() {
    let prefab = valid("rock_cluster");
    assert_eq!(prefab.size, [3, 1, 3]);
    assert_eq!(prefab.props.len(), 3);
    assert!(prefab.spawn("harvest").is_some());
}

#[test]
fn prefab_lamp_is_valid() {
    let prefab = valid("lamp");
    assert_eq!(prefab.props[0].kind, "lamp_post");
    assert_eq!(
        prefab.voxels[0].tile,
        tilesets::find("plaza").expect("plaza").id
    );
}

#[test]
fn prefab_market_stall_is_valid() {
    let prefab = valid("market_stall");
    assert_eq!(prefab.size, [4, 2, 3]);
    assert!(prefab.spawn("vendor").is_some());
    assert!(prefab.spawn("customer").is_some());
    let counter = tilesets::find("counter").expect("counter").id;
    assert_eq!(
        prefab
            .voxels
            .iter()
            .filter(|voxel| voxel.tile == counter)
            .count(),
        4
    );
}

#[test]
fn prefab_json_round_trips_with_tile_names() {
    for prefab in prefabs::all().expect("all prefabs build") {
        let json = prefabs::to_json(&prefab);
        assert_eq!(
            Prefab::from_json(&json).expect("parses"),
            prefab,
            "`{}` does not round-trip",
            prefab.name
        );
        let text = json.to_string_pretty();
        assert_eq!(
            Prefab::from_json(&json::parse_str(&text).expect("parses")).expect("parses"),
            prefab
        );
    }
}

#[test]
fn every_voxel_carries_its_tile_name() {
    for prefab in prefabs::all().expect("all prefabs build") {
        let json = prefabs::to_json(&prefab);
        let voxels = json
            .get("voxels")
            .and_then(JsonValue::as_array)
            .expect("voxels");
        assert_eq!(voxels.len(), prefab.voxels.len());
        for (entry, voxel) in voxels.iter().zip(&prefab.voxels) {
            let name = entry.get_str("name").expect("a tile name");
            assert_eq!(tilesets::find(name).map(|tile| tile.id), Some(voxel.tile));
            assert_eq!(entry.get_u32("tile", 0), voxel.tile);
        }
    }
}

#[test]
fn prefab_voxels_are_unique_and_sorted() {
    for prefab in prefabs::all().expect("all prefabs build") {
        let mut seen: Vec<(u8, u8, u8)> = Vec::new();
        for voxel in &prefab.voxels {
            assert!(
                !seen.contains(&(voxel.x, voxel.y, voxel.z)),
                "`{}` repeats a cell",
                prefab.name
            );
            seen.push((voxel.x, voxel.y, voxel.z));
        }
        let sorted = prefab
            .voxels
            .windows(2)
            .all(|pair| (pair[0].y, pair[0].z, pair[0].x) <= (pair[1].y, pair[1].z, pair[1].x));
        assert!(sorted, "`{}` is not in a stable order", prefab.name);
    }
}

#[test]
fn the_validator_rejects_broken_prefabs() {
    let mut prefab = valid("house_small");
    // A voxel outside the declared bounds.
    let mut outside = prefab.clone();
    outside.voxels.push(noxel_asset::format::PrefabVoxel {
        x: 9,
        y: 0,
        z: 0,
        tile: tilesets::find("floor_wood").expect("tile").id,
    });
    assert!(prefabs::validate(&outside).is_err());

    // A duplicated cell.
    let mut duplicate = prefab.clone();
    if let Some(voxel) = duplicate.voxels.first().copied() {
        duplicate.voxels.push(voxel);
    }
    assert!(prefabs::validate(&duplicate).is_err());

    // A blocked interior: replace the floor with rock.
    let rock = tilesets::find("rock").expect("rock").id;
    for voxel in &mut prefab.voxels {
        if voxel.x == 1 && voxel.y == 0 && voxel.z == 1 {
            voxel.tile = rock;
        }
    }
    assert!(prefabs::validate(&prefab).is_err());
}

// --- world -----------------------------------------------------------------

#[test]
fn the_world_file_follows_its_schema() {
    let demo = world::demo(DEFAULT_SEED);
    world::validate(&demo).expect("the world validates");
    assert_eq!(demo.get_str("name"), Some("town-demo"));
    assert_eq!(demo.get_u32("schema", 0), 1);
    assert_eq!(demo.get_f32("tile_size", 0.0), 1.0);
    assert_eq!(
        demo.get("seed").and_then(JsonValue::as_f64),
        Some(DEFAULT_SEED as f64)
    );
    assert!(demo.get("bounds").is_some());
    assert!(demo.get("player_start").is_some());
}

#[test]
fn the_world_seed_follows_the_generator_seed() {
    for seed in [1u64, 42, DEFAULT_SEED] {
        let demo = world::demo(seed);
        assert_eq!(
            demo.get("seed").and_then(JsonValue::as_f64),
            Some(seed as f64)
        );
        world::validate(&demo).expect("valid");
    }
    // A different seed lays the town out differently.
    assert_ne!(world::demo(1), world::demo(2));
}

#[test]
fn the_world_names_are_unique_and_the_tiles_exist() {
    let demo = world::demo(DEFAULT_SEED);
    let pois = demo
        .get("pois")
        .and_then(JsonValue::as_array)
        .expect("pois");
    let mut names: Vec<&str> = Vec::new();
    for poi in pois {
        let name = poi.get_str("name").expect("a name");
        assert!(!names.contains(&name), "duplicate point of interest");
        names.push(name);
    }
    let tiles = world::known_tiles();
    for biome in demo
        .get("biomes")
        .and_then(JsonValue::as_array)
        .expect("biomes")
    {
        for key in ["ground", "accent"] {
            let tile = biome.get_str(key).expect("a tile name");
            assert!(tiles.contains(&tile), "unknown tile `{tile}`");
        }
    }
}

#[test]
fn the_world_round_trips_through_json() {
    let demo = world::demo(DEFAULT_SEED);
    let text = demo.to_string_pretty();
    let parsed = json::parse_str(&text).expect("parses");
    assert_eq!(parsed, demo);
    world::validate(&parsed).expect("valid");
}

#[test]
fn the_world_notes_are_not_too_short() {
    let demo = world::demo(DEFAULT_SEED);
    assert!(
        !demo
            .get("towns")
            .and_then(JsonValue::as_array)
            .expect("towns")
            .is_empty()
    );
    assert!(
        !demo
            .get("roads")
            .and_then(JsonValue::as_array)
            .expect("roads")
            .is_empty()
    );
    assert!(
        demo.get("biomes")
            .and_then(JsonValue::as_array)
            .expect("biomes")
            .len()
            >= 3
    );
    assert!(
        demo.get("pois")
            .and_then(JsonValue::as_array)
            .expect("pois")
            .len()
            >= 4
    );
}

#[test]
fn the_world_readme_documents_the_schema() {
    let readme = world::readme();
    for key in [
        "seed",
        "biomes",
        "towns",
        "roads",
        "pois",
        "tile_size",
        "player_start",
    ] {
        assert!(
            readme.contains(key),
            "the world README does not mention `{key}`"
        );
    }
    assert!(readme.contains("noxel-gen"));
    assert!(readme.contains("Units"));
}

// --- the asset set ---------------------------------------------------------

#[test]
fn the_asset_set_is_complete() {
    let files = assets::build(DEFAULT_SEED).expect("the assets build");
    assert_eq!(files.len(), 24);
    for (index, file) in files.iter().enumerate() {
        assert!(!file.bytes.is_empty(), "`{}` is empty", file.path);
        for other in &files[index + 1..] {
            assert_ne!(file.path, other.path, "duplicate path");
        }
    }
    assert_eq!(file(&files, "manifest.json").kind, "manifest");
    assert_eq!(file(&files, "config/palette.json").kind, "palette");
    assert_eq!(file(&files, "textures/terrain.png").kind, "texture");
    assert_eq!(file(&files, "tilesets/terrain.json").kind, "tileset");
    assert_eq!(file(&files, "prefabs/house_small.json").kind, "prefab");
    assert_eq!(file(&files, "sprites/characters.json").kind, "sprite");
    assert_eq!(file(&files, "world/demo.json").kind, "world");
    assert_eq!(file(&files, "README.md").kind, "doc");
    assert_eq!(file(&files, "world/README.md").kind, "doc");
}

#[test]
fn building_the_asset_set_is_deterministic() {
    let first = assets::build(DEFAULT_SEED).expect("build");
    let second = assets::build(DEFAULT_SEED).expect("build");
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.path, b.path);
        assert_eq!(a.bytes, b.bytes, "`{}` is not deterministic", a.path);
    }
}

#[test]
fn generating_twice_produces_identical_bytes() {
    let seed = 0x1234_5678;
    let one = TempDir::new("determinism-a");
    let two = TempDir::new("determinism-b");
    let first = assets::generate(one.path(), seed, false).expect("first run");
    let second = assets::generate(two.path(), seed, false).expect("second run");
    assert_eq!(first.written, second.written);
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(walk(one.path()), walk(two.path()));
    for path in walk(one.path()) {
        let a = fs::read(one.path().join(&path)).expect("read");
        let b = fs::read(two.path().join(&path)).expect("read");
        assert_eq!(a, b, "`{path}` differs between two runs");
    }
}

#[test]
fn every_texture_decodes_back_to_the_image_it_was_made_from() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    let expected = [
        ("textures/terrain.png", terrain::atlas()),
        ("textures/props.png", props::atlas()),
        ("textures/characters.png", characters::sheet()),
        ("textures/buildings.png", buildings::atlas()),
    ];
    for (path, image) in expected {
        let generated = file(&files, path);
        assert_eq!(
            texture(generated),
            image,
            "`{path}` does not survive the PNG round trip"
        );
    }
}

#[test]
fn every_texture_pixel_is_a_palette_colour() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    for path in [
        "textures/terrain.png",
        "textures/props.png",
        "textures/characters.png",
        "textures/buildings.png",
    ] {
        let image = texture(file(&files, path));
        for pixel in &image.pixels {
            assert!(
                pixel.a == 0 || palette::contains(*pixel),
                "`{path}` contains {pixel:?}, which is not in the palette"
            );
        }
    }
}

#[test]
fn every_json_asset_reparses_to_the_type_it_claims() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    let palette_file = PaletteFile::from_json(&json_of(file(&files, "config/palette.json")))
        .expect("the palette parses");
    assert_eq!(palette_file.entries.len(), 24);
    for (name, set) in [
        ("tilesets/terrain.json", tilesets::terrain_set()),
        ("tilesets/buildings.json", tilesets::buildings_set()),
    ] {
        assert_eq!(
            TileSet::from_json(&json_of(file(&files, name))).expect("parses"),
            set
        );
    }
    for prefab in prefabs::all().expect("build") {
        let path = format!("prefabs/{}.json", prefab.name);
        assert_eq!(
            Prefab::from_json(&json_of(file(&files, &path))).expect("parses"),
            prefab
        );
    }
    assert_eq!(
        SpriteFile::from_json(&json_of(file(&files, "sprites/characters.json"))).expect("parses"),
        sprites::characters_file()
    );
    world::validate(&json_of(file(&files, "world/demo.json"))).expect("the world validates");
    assert!(json_of(file(&files, "manifest.json")).has("files"));
}

#[test]
fn the_manifest_lists_exactly_the_files_that_exist() {
    let dir = TempDir::new("manifest");
    assets::generate(dir.path(), DEFAULT_SEED, false).expect("generate");
    let on_disk = walk(dir.path());
    let built = assets::build(DEFAULT_SEED).expect("build");
    let manifest = json_of(file(&built, "manifest.json"));
    let listed: Vec<String> = manifest
        .get("files")
        .and_then(JsonValue::as_array)
        .expect("the manifest lists files")
        .iter()
        .map(|entry| entry.get_str("path").expect("a path").to_string())
        .collect();
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(sorted, on_disk, "the manifest and the tree disagree");
    assert!(listed.contains(&"manifest.json".to_string()));
}

#[test]
fn the_manifest_lists_every_file_of_every_kind() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    let manifest = json_of(file(&files, "manifest.json"));
    let typed = [
        ("tile_sets", "tileset"),
        ("prefabs", "prefab"),
        ("palettes", "palette"),
        ("sprites", "sprite"),
    ];
    for (key, kind) in typed {
        let listed = manifest
            .get(key)
            .and_then(JsonValue::as_array)
            .unwrap_or_else(|| panic!("`{key}` is missing"));
        // The typed lists keep generation order, which for tile sets means the
        // terrain set comes first: a consumer that loads a single tile set (as
        // `noxel-app` does today) must get the ground, not the walls.
        let expected: Vec<&str> = files
            .iter()
            .filter(|file| file.kind == kind)
            .map(|file| file.path.as_str())
            .collect();
        let actual: Vec<&str> = listed.iter().filter_map(JsonValue::as_str).collect();
        assert_eq!(actual, expected, "`{key}` does not match the {kind} files");
    }
    // The engine can load the manifest as an AssetManifest.
    let parsed = noxel_asset::format::AssetManifest::from_json(&manifest).expect("parses");
    assert_eq!(parsed.name, "town-demo");
    assert_eq!(parsed.tile_sets.len(), 2);
    assert_eq!(
        parsed.tile_sets.first().map(String::as_str),
        Some("tilesets/terrain.json"),
        "the ground tile set must be the first the manifest lists"
    );
    assert_eq!(parsed.prefabs.len(), 12);
    assert_eq!(parsed.palettes.len(), 1);
    assert_eq!(parsed.sprites.len(), 1);
    assert_eq!(parsed.len(), 16);
}

#[test]
fn the_readme_forbids_hand_editing() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    let readme = String::from_utf8(file(&files, "README.md").bytes.clone()).expect("utf-8");
    assert!(readme.contains("Do not hand-edit"));
    assert!(readme.contains("cargo run -p noxel-gen -- generate --force"));
    for folder in [
        "config/",
        "textures/",
        "tilesets/",
        "prefabs/",
        "sprites/",
        "world/",
    ] {
        assert!(
            readme.contains(folder),
            "the README does not describe `{folder}`"
        );
    }
    for name in terrain::NAMES.iter().chain(buildings::NAMES.iter()) {
        assert!(readme.contains(name), "the README omits tile `{name}`");
    }
    for cell in &props::CELLS {
        assert!(
            readme.contains(cell.name),
            "the README omits prop `{}`",
            cell.name
        );
    }
    assert!(readme.contains("metre"));
}

#[test]
fn the_readme_reports_the_real_file_count() {
    let files = assets::build(DEFAULT_SEED).expect("build");
    let readme = String::from_utf8(file(&files, "README.md").bytes.clone()).expect("utf-8");
    assert!(
        readme.contains(&format!("{} generated files", files.len())),
        "the README miscounts the asset set"
    );
    assert!(
        readme.contains("preview.png"),
        "the README explains preview.png"
    );
}

// --- generate, verify, list ------------------------------------------------

#[test]
fn generate_writes_every_file_and_then_skips_them() {
    let dir = TempDir::new("generate");
    let first = assets::generate(dir.path(), DEFAULT_SEED, false).expect("first");
    let files = assets::build(DEFAULT_SEED).expect("build");
    assert_eq!(first.written, files.len());
    assert_eq!(first.skipped, 0);
    assert_eq!(
        first.bytes,
        files.iter().map(|file| file.bytes.len()).sum::<usize>()
    );
    assert!(first.line(dir.path()).contains("written"));

    let second = assets::generate(dir.path(), DEFAULT_SEED, false).expect("second");
    assert_eq!(second.written, 0, "an unchanged tree must not be rewritten");
    assert_eq!(second.skipped, files.len());

    let forced = assets::generate(dir.path(), DEFAULT_SEED, true).expect("forced");
    assert_eq!(forced.written, files.len());
    assert_eq!(forced.skipped, 0);
}

#[test]
fn verify_passes_on_a_fresh_generation() {
    let dir = TempDir::new("verify-ok");
    assets::generate(dir.path(), DEFAULT_SEED, false).expect("generate");
    let summary = assets::verify(dir.path(), DEFAULT_SEED).expect("verify");
    assert_eq!(summary.files, 24);
    assert!(summary.bytes > 0);
}

#[test]
fn verify_fails_when_a_file_is_tampered_with() {
    let dir = TempDir::new("verify-tampered");
    assets::generate(dir.path(), DEFAULT_SEED, false).expect("generate");
    let target = dir.path().join("textures/terrain.png");
    let mut bytes = fs::read(&target).expect("read");
    bytes.push(0);
    fs::write(&target, &bytes).expect("write");
    let error = assets::verify(dir.path(), DEFAULT_SEED).expect_err("verify must fail");
    assert!(matches!(error, crate::error::Error::Mismatch { count: 1 }));
    assert!(error.to_string().contains("differ"));
}

#[test]
fn verify_fails_when_a_file_is_missing() {
    let dir = TempDir::new("verify-missing");
    assets::generate(dir.path(), DEFAULT_SEED, false).expect("generate");
    fs::remove_file(dir.path().join("prefabs/inn.json")).expect("remove");
    let error = assets::verify(dir.path(), DEFAULT_SEED).expect_err("verify must fail");
    assert!(matches!(error, crate::error::Error::Mismatch { count: 1 }));
}

#[test]
fn verify_fails_on_an_empty_directory() {
    let dir = TempDir::new("verify-empty");
    assert!(assets::verify(dir.path(), DEFAULT_SEED).is_err());
}

#[test]
fn verify_recovers_the_seed_from_the_world_file() {
    let dir = TempDir::new("verify-seed");
    assets::generate(dir.path(), 4242, false).expect("generate");
    assert_eq!(assets::seed_from_disk(dir.path()), Some(4242));
    assets::verify(dir.path(), 4242).expect("verify with the world's own seed");
    // Verifying with the wrong seed is a real difference, not a silent pass.
    assert!(assets::verify(dir.path(), DEFAULT_SEED).is_err());
    assert_eq!(assets::seed_from_disk(&dir.path().join("nope")), None);
}

#[test]
fn a_custom_seed_still_verifies_against_itself() {
    let dir = TempDir::new("verify-custom-seed");
    assets::generate(dir.path(), 9, false).expect("generate");
    let seed = assets::seed_from_disk(dir.path()).expect("a recorded seed");
    assets::verify(dir.path(), seed).expect("verify");
}

#[test]
fn the_listing_names_every_file() {
    let listing = assets::listing(DEFAULT_SEED).expect("listing");
    let files = assets::build(DEFAULT_SEED).expect("build");
    assert!(listing.starts_with(&format!("noxel-gen: {} files", files.len())));
    for file in &files {
        assert!(
            listing.contains(&file.path),
            "the listing omits `{}`",
            file.path
        );
        assert!(listing.contains(file.kind));
    }
    assert_eq!(listing, assets::listing(DEFAULT_SEED).expect("listing"));
}

#[test]
fn the_asset_paths_are_relative_and_use_forward_slashes() {
    for file in assets::build(DEFAULT_SEED).expect("build") {
        assert!(!file.path.starts_with('/'));
        assert!(!file.path.contains('\\'));
        assert!(!file.path.contains(".."));
        assert!(Path::new(&file.path).extension().is_some());
    }
}

// --- preview ---------------------------------------------------------------

#[test]
fn the_contact_sheet_is_the_scaled_union_of_the_textures() {
    let (width, height) = preview::sheet_size(4);
    assert_eq!(width, terrain::atlas().width() * 4);
    let expected_height = (terrain::atlas().height()
        + props::atlas().height()
        + characters::sheet().height()
        + buildings::atlas().height())
        * 4;
    assert_eq!(height, expected_height);
    assert_eq!(preview::sheet_size(1), (256, 152));
    assert_eq!(preview::sheet_size(8), (2048, 1216));
}

#[test]
fn preview_writes_a_png_of_exactly_the_scaled_size() {
    for scale in [1u32, 3, 5] {
        let dir = TempDir::new("preview");
        assets::generate(dir.path(), DEFAULT_SEED, false).expect("generate");
        let size = preview::write(dir.path(), scale).expect("preview");
        assert_eq!(size, preview::sheet_size(scale));
        let bytes = fs::read(dir.path().join("preview.png")).expect("read");
        let image = png::decode(&bytes).expect("decode");
        assert_eq!((image.width(), image.height()), size);
        for pixel in &image.pixels {
            assert!(pixel.a == 0 || palette::contains(*pixel));
        }
    }
}

#[test]
fn preview_needs_the_textures_it_sheets() {
    let dir = TempDir::new("preview-empty");
    assert!(preview::write(dir.path(), 2).is_err());
}

// --- the CLI end to end ----------------------------------------------------

#[test]
fn the_subcommands_run_end_to_end() {
    let dir = TempDir::new("end-to-end");
    let out = dir.path().to_string_lossy().to_string();
    crate::run(&args(&["generate", "--out", &out, "--seed", "11"])).expect("generate");
    let summary = assets::seed_from_disk(dir.path()).expect("a seed");
    assert_eq!(summary, 11);
    crate::run(&args(&["verify", "--out", &out])).expect("verify");
    crate::run(&args(&["preview", "--out", &out, "--scale", "2"])).expect("preview");
    crate::run(&args(&["list"])).expect("list");
    crate::run(&args(&["--help"])).expect("help");
    assert!(dir.path().join("preview.png").is_file());

    // A tampered file makes `verify` fail, which is the whole point of it.
    fs::write(dir.path().join("README.md"), b"hand edited").expect("tamper");
    assert!(crate::run(&args(&["verify", "--out", &out])).is_err());
}
