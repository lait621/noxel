//! Finding and loading the game's assets.
//!
//! The game must run in three places, in this order of preference:
//!
//! 1. From a shipped bundle, where `assets/` sits beside the executable.
//! 2. From the source tree, where it is `games/noxel-valley/assets/`.
//! 3. Nowhere. A missing asset tree is not an error: the game falls back to flat
//!    colours and the built-in UI theme, which is what lets `cargo test` build a
//!    working game in a millisecond with no files on disk. That property comes
//!    straight from the engine (`docs/02-getting-started.md`), and it is worth
//!    keeping: a test that needs a PNG is a test that fails on a fresh checkout.

use std::path::{Path, PathBuf};

use noxel_asset::atlas::{Atlas, SpriteRegion};
use noxel_asset::image::Image;
use noxel_asset::json::parse_str;
use noxel_asset::texture::Texture;
use noxel_ui::FontSet;

/// The atlases the game draws from.
#[derive(Clone, Debug, Default)]
pub struct Atlases {
    /// Ground tiles.
    pub terrain: Option<Atlas>,
    /// Crop growth stages.
    pub crops: Option<Atlas>,
    /// Trees, rocks, buildings, fixtures.
    pub props: Option<Atlas>,
    /// Character walk cycles.
    pub characters: Option<Atlas>,
    /// Nine-slice frames and icons.
    pub ui: Option<Atlas>,
}

impl Atlases {
    /// Looks a region up across every atlas, in a fixed order.
    ///
    /// One lookup for the caller, so a UI icon and a crop icon are fetched the
    /// same way even though they live in different files.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<(&Atlas, &SpriteRegion)> {
        for atlas in [
            &self.ui,
            &self.props,
            &self.crops,
            &self.terrain,
            &self.characters,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(region) = atlas.region(name) {
                return Some((atlas, region));
            }
        }
        None
    }
}

/// Everything the game loads from disk.
#[derive(Clone, Debug)]
pub struct Assets {
    /// The atlases.
    pub atlases: Atlases,
    /// The UI font, if the bake was found.
    pub font: Option<FontSet>,
    /// The directory the assets came from, for the log.
    pub root: Option<PathBuf>,
    /// The UI texture as a raw image, for the painter's nine-slice blits.
    pub ui_image: Option<Image>,
}

impl Assets {
    /// Loads everything that exists, and nothing that does not.
    #[must_use]
    pub fn load(root: Option<PathBuf>) -> Self {
        let Some(root) = root else {
            return Self::empty();
        };
        let farm = root.join("farm");
        Self {
            atlases: Atlases {
                terrain: load_atlas(&farm, "terrain"),
                crops: load_atlas(&farm, "crops"),
                props: load_atlas(&farm, "props"),
                characters: load_atlas(&farm, "characters"),
                ui: load_atlas(&farm, "ui"),
            },
            font: FontSet::load(
                root.join("fonts/ui_font.png"),
                root.join("fonts/ui_font.json"),
            )
            .ok(),
            root: Some(root),
            ui_image: load_atlas(&farm, "ui").map(|atlas| atlas.texture().image().clone()),
        }
    }

    /// No assets at all: every lookup misses and the game draws flat colours.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            atlases: Atlases::default(),
            font: None,
            root: None,
            ui_image: None,
        }
    }

    /// Whether the art was found. The UI reports this, so a player looking at a
    /// flat-coloured farm knows why.
    #[must_use]
    pub fn has_art(&self) -> bool {
        self.atlases.terrain.is_some() && self.atlases.props.is_some()
    }

    /// Whether the font was found.
    #[must_use]
    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// The sprite region for a character frame.
    #[must_use]
    pub fn character(&self, name: &str) -> Option<&SpriteRegion> {
        self.atlases.characters.as_ref()?.region(name)
    }

    /// The UI texture, or a transparent placeholder.
    #[must_use]
    pub fn ui_texture(&self) -> &Image {
        static EMPTY: std::sync::OnceLock<Image> = std::sync::OnceLock::new();
        self.ui_image
            .as_ref()
            .unwrap_or_else(|| EMPTY.get_or_init(|| Image::transparent(1, 1)))
    }
}

fn load_atlas(directory: &Path, name: &str) -> Option<Atlas> {
    let image = Image::load_png(directory.join(format!("{name}.png"))).ok()?;
    let text = std::fs::read_to_string(directory.join(format!("{name}.json"))).ok()?;
    let json = parse_str(&text).ok()?;
    Atlas::from_json(Texture::from_image(image), &json).ok()
}

/// Finds the asset directory.
///
/// Beside the executable first, because that is what makes a shipped bundle
/// relocatable — copy the folder anywhere and it still finds its art. The source
/// tree is the fallback, and only exists in a development build.
#[must_use]
pub fn find_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(directory) = exe.parent() {
            candidates.push(directory.join("assets"));
            // A macOS `.app` puts the binary in `Contents/MacOS` and the
            // resources in `Contents/Resources`.
            candidates.push(directory.join("../Resources/assets"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("assets"));
        candidates.push(cwd.join("games/noxel-valley/assets"));
    }
    // The compile-time path is the last resort: it is what makes `cargo run`
    // from the workspace root work without anyone setting a variable.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("assets"));

    candidates
        .into_iter()
        .find(|candidate| candidate.join("farm").is_dir() || candidate.join("fonts").is_dir())
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_asset_set_reports_itself_honestly() {
        // The fallback path matters: every test in this crate runs against it,
        // and a game that panicked without art would be untestable.
        let assets = Assets::empty();
        assert!(!assets.has_art());
        assert!(!assets.has_font());
        assert!(assets.atlases.find("grass").is_none());
        assert!(assets.character("player_down_0").is_none());
    }

    #[test]
    fn the_ui_texture_falls_back_to_something_drawable() {
        // `Painter::frame` needs an image even when no atlas was loaded; the
        // one-pixel placeholder keeps the flat theme working.
        let assets = Assets::empty();
        assert_eq!(assets.ui_texture().width(), 1);
        assert_eq!(assets.ui_texture().height(), 1);
    }

    #[test]
    fn loading_a_missing_directory_yields_no_assets_rather_than_panicking() {
        let assets = Assets::load(Some(PathBuf::from("/nonexistent/noxel-valley-assets")));
        assert!(!assets.has_art());
        assert!(
            assets.root.is_some(),
            "the root is remembered even when it is empty"
        );
    }
}
