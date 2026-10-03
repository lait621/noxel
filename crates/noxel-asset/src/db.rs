//! The asset database: path resolution, caching and hot reload.
//!
//! [`AssetDb`] maps a *logical* name (`"textures/ground.png"`) onto a real file
//! under a root directory and caches whatever it parses. Every accessor returns
//! an `Arc<T>`, so the renderer and the world generator can hold on to an image
//! while the database keeps handing out the same allocation.
//!
//! ## Hot reload
//!
//! [`AssetDb::poll_changes`] compares each cached file's modification time and
//! length against what it was when the file was read. Changed (or deleted)
//! entries are dropped from the cache and their logical names returned, so the
//! caller can reload exactly what moved:
//!
//! ```no_run
//! # use noxel_asset::db::AssetDb;
//! let mut db = AssetDb::new("assets");
//! for logical in db.poll_changes() {
//!     println!("reloading {logical}");
//! }
//! ```
//!
//! ## Errors
//!
//! Nothing here panics on a missing or malformed file: every accessor returns an
//! [`AssetError`], and [`AssetError::NotFound`] carries both the logical name
//! and the resolved path so the message points at the file that is actually
//! missing.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use noxel_core::math::Palette;

use crate::format::{AssetManifest, FormatError, PaletteFile, Prefab, SpriteFile, TileSet};
use crate::image::Image;
use crate::json::{self, JsonError, JsonValue};
use crate::png::{self, PngError};
use crate::texture::Texture;

/// Anything that can go wrong while loading an asset.
#[derive(Debug)]
pub enum AssetError {
    /// The logical name does not resolve to a readable file.
    NotFound {
        /// The logical name that was requested.
        logical: String,
        /// The path it resolved to.
        resolved: PathBuf,
    },
    /// The file exists but could not be read.
    Io {
        /// The logical name that was requested.
        logical: String,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// The file is not valid JSON.
    Json {
        /// The logical name that was requested.
        logical: String,
        /// The parser error, including line and column.
        source: JsonError,
    },
    /// The file is not a valid PNG.
    Png {
        /// The logical name that was requested.
        logical: String,
        /// The codec error.
        source: PngError,
    },
    /// The JSON was valid but does not describe the requested format.
    Format {
        /// The logical name that was requested.
        logical: String,
        /// The field-level error.
        source: FormatError,
    },
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { logical, resolved } => {
                write!(f, "asset `{logical}` not found at {}", resolved.display())
            }
            Self::Io { logical, source } => write!(f, "reading `{logical}` failed: {source}"),
            Self::Json { logical, source } => write!(f, "parsing `{logical}` failed: {source}"),
            Self::Png { logical, source } => write!(f, "decoding `{logical}` failed: {source}"),
            Self::Format { logical, source } => write!(f, "loading `{logical}` failed: {source}"),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound { .. } => None,
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Png { source, .. } => Some(source),
            Self::Format { source, .. } => Some(source),
        }
    }
}

/// A snapshot of the cache's state and effectiveness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetStats {
    /// Number of cached entries.
    pub entries: usize,
    /// Total size in bytes of the files backing those entries.
    pub bytes: u64,
    /// Cache lookups that were served without touching the disk.
    pub hits: u64,
    /// Cache lookups that had to load (or reload) from disk.
    pub misses: u64,
}

/// What a cache entry holds.
#[derive(Clone, Debug)]
enum EntryKind {
    Text(Arc<String>),
    Json(JsonValue),
    Image(Arc<Image>),
    Texture(Arc<Texture>),
    TileSet(Arc<TileSet>),
    Prefab(Arc<Prefab>),
    Palette(Arc<Palette>),
    Sprite(Arc<SpriteFile>),
    Manifest(Arc<AssetManifest>),
}

/// One cached file: what was parsed, plus the metadata hot reload compares.
#[derive(Clone, Debug)]
struct Entry {
    kind: EntryKind,
    modified: Option<SystemTime>,
    len: u64,
}

/// A root directory plus a cache of everything loaded from it.
///
/// The cache is keyed by logical name. A name can only hold one kind at a time:
/// asking for `"a.json"` as text and then as JSON replaces the entry (and counts
/// a miss), which keeps the [`AssetStats`] honest.
#[derive(Debug)]
pub struct AssetDb {
    root: PathBuf,
    entries: HashMap<String, Entry>,
    hits: u64,
    misses: u64,
}

impl AssetDb {
    /// Creates a database rooted at `root`. Nothing is read until you ask.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a logical name against the root.
    ///
    /// A leading `/` or `\` is ignored so that both `"textures/a.png"` and
    /// `"/textures/a.png"` resolve under the root rather than against the
    /// filesystem root.
    #[must_use]
    pub fn resolve(&self, logical: &str) -> PathBuf {
        let relative = logical.trim_start_matches(['/', '\\']);
        self.root.join(relative)
    }

    /// True when `logical` is currently cached (as any kind).
    #[must_use]
    pub fn cached(&self, logical: &str) -> bool {
        self.entries.contains_key(logical)
    }

    /// Drops one entry, returning whether it was cached.
    pub fn evict(&mut self, logical: &str) -> bool {
        self.entries.remove(logical).is_some()
    }

    /// Reads a text file, caching its contents.
    pub fn load_string(&mut self, logical: &str) -> Result<Arc<String>, AssetError> {
        if let Some(text) = self.hit(logical, |kind| match kind {
            EntryKind::Text(text) => Some(text.clone()),
            _ => None,
        }) {
            return Ok(text);
        }
        let text = self.read_string(logical)?;
        self.insert(logical, EntryKind::Text(text.clone()));
        Ok(text)
    }

    /// Reads and parses a JSON file.
    pub fn load_json(&mut self, logical: &str) -> Result<JsonValue, AssetError> {
        if let Some(value) = self.hit(logical, |kind| match kind {
            EntryKind::Json(value) => Some(value.clone()),
            _ => None,
        }) {
            return Ok(value);
        }
        let value = self.json_cached(logical)?;
        Ok(value)
    }

    /// Reads and decodes a PNG file.
    pub fn load_image(&mut self, logical: &str) -> Result<Arc<Image>, AssetError> {
        if let Some(image) = self.hit(logical, |kind| match kind {
            EntryKind::Image(image) => Some(image.clone()),
            _ => None,
        }) {
            return Ok(image);
        }
        self.load_image_uncached(logical)
    }

    /// Loads a PNG and wraps it as a [`Texture`].
    pub fn load_texture(&mut self, logical: &str) -> Result<Arc<Texture>, AssetError> {
        if let Some(texture) = self.hit(logical, |kind| match kind {
            EntryKind::Texture(texture) => Some(texture.clone()),
            _ => None,
        }) {
            return Ok(texture);
        }
        let image = self.load_image_uncached(logical)?;
        let texture = Arc::new(Texture::from_image((*image).clone()));
        self.insert(logical, EntryKind::Texture(texture.clone()));
        Ok(texture)
    }

    /// Loads a [`TileSet`] from JSON.
    pub fn tile_set(&mut self, logical: &str) -> Result<Arc<TileSet>, AssetError> {
        self.load_format(
            logical,
            |kind| match kind {
                EntryKind::TileSet(set) => Some(set.clone()),
                _ => None,
            },
            TileSet::from_json,
            EntryKind::TileSet,
        )
    }

    /// Loads a [`Prefab`] from JSON.
    pub fn prefab(&mut self, logical: &str) -> Result<Arc<Prefab>, AssetError> {
        self.load_format(
            logical,
            |kind| match kind {
                EntryKind::Prefab(prefab) => Some(prefab.clone()),
                _ => None,
            },
            Prefab::from_json,
            EntryKind::Prefab,
        )
    }

    /// Loads a [`SpriteFile`] from JSON.
    pub fn sprite(&mut self, logical: &str) -> Result<Arc<SpriteFile>, AssetError> {
        self.load_format(
            logical,
            |kind| match kind {
                EntryKind::Sprite(sprite) => Some(sprite.clone()),
                _ => None,
            },
            SpriteFile::from_json,
            EntryKind::Sprite,
        )
    }

    /// Loads an [`AssetManifest`] from JSON.
    pub fn manifest(&mut self, logical: &str) -> Result<Arc<AssetManifest>, AssetError> {
        self.load_format(
            logical,
            |kind| match kind {
                EntryKind::Manifest(manifest) => Some(manifest.clone()),
                _ => None,
            },
            AssetManifest::from_json,
            EntryKind::Manifest,
        )
    }

    /// Loads a palette file and converts it to the engine's [`Palette`].
    ///
    /// See [`PaletteFile::to_palette`] for the (small, deliberate) string leak
    /// this involves: `noxel_core`'s palette entries store `&'static str` names.
    pub fn palette(&mut self, logical: &str) -> Result<Arc<Palette>, AssetError> {
        self.load_format(
            logical,
            |kind| match kind {
                EntryKind::Palette(palette) => Some(palette.clone()),
                _ => None,
            },
            |json| PaletteFile::from_json(json).map(|file| file.to_palette()),
            EntryKind::Palette,
        )
    }

    /// Cache statistics: entry count, bytes held and hit/miss counters.
    #[must_use]
    pub fn stats(&self) -> AssetStats {
        AssetStats {
            entries: self.entries.len(),
            bytes: self.entries.values().map(|entry| entry.len).sum(),
            hits: self.hits,
            misses: self.misses,
        }
    }

    /// Empties the cache. The hit/miss counters are left alone so that they
    /// still describe the database's whole lifetime.
    pub fn clear_cache(&mut self) {
        self.entries.clear();
    }

    /// Re-reads the modification time of every cached file and drops the ones
    /// that changed, returning their logical names in sorted order.
    ///
    /// A file that has been deleted also counts as changed. Files that are not
    /// cached are not tracked: the database only watches what it has loaded.
    pub fn poll_changes(&mut self) -> Vec<String> {
        let mut changed: Vec<String> = Vec::new();
        for (logical, entry) in &self.entries {
            let resolved = self.resolve(logical);
            let stale = match fs::metadata(&resolved) {
                Ok(meta) => meta.modified().ok() != entry.modified || meta.len() != entry.len,
                Err(_) => true,
            };
            if stale {
                changed.push(logical.clone());
            }
        }
        changed.sort();
        for logical in &changed {
            self.entries.remove(logical);
        }
        changed
    }

    // --- internals ---------------------------------------------------------

    /// Looks an entry up and counts the lookup as a hit or a miss.
    fn hit<T>(
        &mut self,
        logical: &str,
        extract: impl FnOnce(&EntryKind) -> Option<T>,
    ) -> Option<T> {
        let found = self
            .entries
            .get(logical)
            .and_then(|entry| extract(&entry.kind));
        if found.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        found
    }

    /// Records a freshly loaded entry together with its file metadata.
    fn insert(&mut self, logical: &str, kind: EntryKind) {
        let resolved = self.resolve(logical);
        let (modified, len) = match fs::metadata(&resolved) {
            Ok(meta) => (meta.modified().ok(), meta.len()),
            Err(_) => (None, 0),
        };
        self.entries.insert(
            logical.to_string(),
            Entry {
                kind,
                modified,
                len,
            },
        );
    }

    fn read_string(&self, logical: &str) -> Result<Arc<String>, AssetError> {
        let resolved = self.resolve(logical);
        match fs::read_to_string(&resolved) {
            Ok(text) => Ok(Arc::new(text)),
            Err(source) => Err(io_error(logical, &resolved, source)),
        }
    }

    fn read_bytes(&self, logical: &str) -> Result<Vec<u8>, AssetError> {
        let resolved = self.resolve(logical);
        fs::read(&resolved).map_err(|source| io_error(logical, &resolved, source))
    }

    /// The cached JSON for `logical`, reading and parsing it when needed.
    ///
    /// Does not touch the hit/miss counters: those describe the public
    /// accessors, and a typed loader that consults the JSON cache is doing
    /// internal work rather than serving a request.
    fn json_cached(&mut self, logical: &str) -> Result<JsonValue, AssetError> {
        if let Some(value) = self
            .entries
            .get(logical)
            .and_then(|entry| match &entry.kind {
                EntryKind::Json(value) => Some(value.clone()),
                _ => None,
            })
        {
            return Ok(value);
        }
        let bytes = self.read_bytes(logical)?;
        let value = json::parse(&bytes).map_err(|source| AssetError::Json {
            logical: logical.to_string(),
            source,
        })?;
        self.insert(logical, EntryKind::Json(value.clone()));
        Ok(value)
    }

    /// Shared body of the typed `from_json` loaders: cache lookup, JSON read,
    /// parse, cache insert. A parse failure leaves nothing behind, so fixing the
    /// file and retrying behaves like the first attempt.
    fn load_format<T>(
        &mut self,
        logical: &str,
        extract: fn(&EntryKind) -> Option<Arc<T>>,
        build: fn(&JsonValue) -> Result<T, FormatError>,
        wrap: fn(Arc<T>) -> EntryKind,
    ) -> Result<Arc<T>, AssetError> {
        if let Some(value) = self.hit(logical, extract) {
            return Ok(value);
        }
        let json = self.json_cached(logical)?;
        match build(&json) {
            Ok(parsed) => {
                let value = Arc::new(parsed);
                self.insert(logical, wrap(value.clone()));
                Ok(value)
            }
            Err(source) => {
                self.entries.remove(logical);
                Err(AssetError::Format {
                    logical: logical.to_string(),
                    source,
                })
            }
        }
    }

    fn load_image_uncached(&mut self, logical: &str) -> Result<Arc<Image>, AssetError> {
        let bytes = self.read_bytes(logical)?;
        let image = png::decode(&bytes).map_err(|source| AssetError::Png {
            logical: logical.to_string(),
            source,
        })?;
        let image = Arc::new(image);
        self.insert(logical, EntryKind::Image(image.clone()));
        Ok(image)
    }
}

fn io_error(logical: &str, resolved: &Path, source: io::Error) -> AssetError {
    if source.kind() == io::ErrorKind::NotFound {
        AssetError::NotFound {
            logical: logical.to_string(),
            resolved: resolved.to_path_buf(),
        }
    } else {
        AssetError::Io {
            logical: logical.to_string(),
            source,
        }
    }
}

/// Writes `bytes` to `path` atomically: a sibling `.tmp` file is written and
/// then renamed over the target, so a crash mid-write can never leave a
/// half-written asset behind.
///
/// Missing parent directories are created, which is what the generator tool
/// wants. The temporary file is removed if the rename fails.
pub fn write_atomic(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A scratch directory under `std::env::temp_dir()` that deletes itself.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "noxel-asset-test-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create scratch dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let full = self.path.join(relative);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&full, bytes).expect("write file");
            full
        }

        fn db(&self) -> AssetDb {
            AssetDb::new(&self.path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn loads_and_caches_text_and_json() {
        let scratch = Scratch::new("text");
        scratch.write("config/a.json", br#"{"name":"town","tile_size":16}"#);
        scratch.write("readme.txt", b"hello");

        let mut db = scratch.db();
        assert_eq!(db.root(), scratch.path());
        assert!(!db.cached("config/a.json"));

        let text = db.load_string("readme.txt").unwrap();
        assert_eq!(*text, "hello");
        assert!(db.cached("readme.txt"));

        let value = db.load_json("config/a.json").unwrap();
        assert_eq!(value.get_str("name"), Some("town"));
        let again = db.load_json("config/a.json").unwrap();
        assert_eq!(again, value);
        assert!(Arc::ptr_eq(&text, &db.load_string("readme.txt").unwrap()));

        let stats = db.stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.hits, 2, "the two repeat reads");
        assert_eq!(stats.misses, 2, "the two cold reads");
        assert_eq!(stats.bytes, 30 + 5);
        assert_eq!(stats, db.stats());

        db.clear_cache();
        assert_eq!(db.stats().entries, 0);
        assert_eq!(db.stats().hits, 2);
        assert!(!db.cached("readme.txt"));

        // The same logical name with a different accessor reloads.
        db.load_string("config/a.json").unwrap();
        assert!(db.load_json("config/a.json").is_ok());
        assert!(db.evict("config/a.json"));
        assert!(!db.evict("config/a.json"));
    }

    #[test]
    fn missing_files_are_errors_not_panics() {
        let scratch = Scratch::new("missing");
        let mut db = scratch.db();

        let err = db.load_string("nope.txt").unwrap_err();
        match &err {
            AssetError::NotFound { logical, resolved } => {
                assert_eq!(logical, "nope.txt");
                assert!(resolved.ends_with("nope.txt"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
        assert!(err.to_string().contains("nope.txt"));
        assert!(err.source().is_none(), "NotFound has no underlying error");
        let _: &dyn std::error::Error = &err;

        assert!(matches!(
            db.load_json("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.load_image("nope.png").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.load_texture("nope.png").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.tile_set("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.prefab("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.palette("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.sprite("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert!(matches!(
            db.manifest("nope.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));
        assert_eq!(db.stats().entries, 0, "failed loads cache nothing");
    }

    #[test]
    fn loads_images_and_textures() {
        let scratch = Scratch::new("image");
        let image = Image::from_fn(4, 4, |x, y| {
            noxel_core::math::Color8::new(x as u8 * 10, y as u8 * 10, 7, 255)
        });
        scratch.write("textures/ground.png", &image.to_png_bytes());

        let mut db = scratch.db();
        let loaded = db.load_image("textures/ground.png").unwrap();
        assert_eq!(*loaded, image);
        assert!(Arc::ptr_eq(
            &loaded,
            &db.load_image("textures/ground.png").unwrap()
        ));

        let texture = db.load_texture("textures/ground.png").unwrap();
        assert_eq!(texture.width(), 4);
        assert_eq!(texture.sample_nearest(0.0, 0.0), image.get(0, 0).unwrap());
        assert!(Arc::ptr_eq(
            &texture,
            &db.load_texture("textures/ground.png").unwrap()
        ));

        // A PNG-named file that is not a PNG reports a codec error.
        scratch.write("textures/broken.png", b"definitely not a png");
        let err = db.load_image("textures/broken.png").unwrap_err();
        match &err {
            AssetError::Png { logical, source } => {
                assert_eq!(logical, "textures/broken.png");
                assert!(source.to_string().contains("signature"));
            }
            other => panic!("expected Png, got {other:?}"),
        }
        assert!(err.source().is_some());

        // A JSON file that is not JSON reports the parser position.
        scratch.write("bad.json", b"{\n  \"a\": }");
        let err = db.load_json("bad.json").unwrap_err();
        match &err {
            AssetError::Json { source, .. } => assert_eq!((source.line, source.column), (2, 8)),
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn loads_formats_and_reports_field_paths() {
        let scratch = Scratch::new("formats");
        scratch.write(
            "tiles/town.json",
            br#"{"name":"town","texture":"t.png","tile_size":16,
                 "tiles":[{"id":0,"name":"grass","uv":[0,0,16,16]}]}"#,
        );
        scratch.write(
            "tiles/broken.json",
            br#"{"name":"town","texture":"t.png","tile_size":16,
                 "tiles":[{"id":0,"name":"grass","uv":[0,0,16]}]}"#,
        );
        scratch.write(
            "prefabs/house.json",
            br#"{"name":"house","size":[2,2,2],"tags":["residential"],
                 "voxels":[{"x":0,"y":0,"z":0,"tile":0}],
                 "spawns":[{"name":"door","position":[0,0,1]}]}"#,
        );
        scratch.write(
            "config/palette.json",
            br##"{"name":"dawn","entries":[{"name":"grass","color":"#2E8B2E"},{"name":"sky","color":"#87CEEB"}]}"##,
        );
        scratch.write(
            "sprites/hero.json",
            br#"{"name":"hero","texture":"hero.png","uv":[0,0,16,24],"frames":[{"name":"idle","uv":[0,0,16,24]}]}"#,
        );
        scratch.write(
            "manifest.json",
            br#"{"name":"demo","version":"0.1.0","tile_sets":["tiles/town.json"]}"#,
        );

        let mut db = scratch.db();
        let set = db.tile_set("tiles/town.json").unwrap();
        assert_eq!(set.tile(0).unwrap().name, "grass");
        assert!(Arc::ptr_eq(&set, &db.tile_set("tiles/town.json").unwrap()));

        let prefab = db.prefab("prefabs/house.json").unwrap();
        assert_eq!(prefab.footprint(), (2, 2));
        assert!(prefab.has_tag("residential"));
        assert_eq!(prefab.voxel(0, 0, 0), Some(0));

        let palette = db.palette("config/palette.json").unwrap();
        assert_eq!(palette.len(), 2);
        assert_eq!(
            palette.find("sky"),
            Some(noxel_core::math::Color8::rgb(0x87, 0xCE, 0xEB))
        );

        let sprite = db.sprite("sprites/hero.json").unwrap();
        assert_eq!(sprite.frames.len(), 1);
        assert_eq!(sprite.size, [16.0, 24.0]);

        let manifest = db.manifest("manifest.json").unwrap();
        assert_eq!(manifest.tile_sets, ["tiles/town.json"]);
        assert_eq!(manifest.len(), 1);

        let err = db.tile_set("tiles/broken.json").unwrap_err();
        match &err {
            AssetError::Format { logical, source } => {
                assert_eq!(logical, "tiles/broken.json");
                assert_eq!(source.path, "tiles[0].uv");
            }
            other => panic!("expected Format, got {other:?}"),
        }
        assert!(err.to_string().contains("tiles[0].uv"));
        assert_eq!(db.stats().entries, 5);
    }

    #[test]
    fn poll_changes_detects_a_modified_file() {
        let scratch = Scratch::new("reload");
        scratch.write("a.json", br#"{"v":1}"#);
        scratch.write("b.json", br#"{"v":1}"#);

        let mut db = scratch.db();
        assert_eq!(db.load_json("a.json").unwrap().get_u32("v", 0), 1);
        assert_eq!(db.load_json("b.json").unwrap().get_u32("v", 0), 1);
        assert!(db.poll_changes().is_empty(), "nothing changed yet");

        // Rewrite one file with a different length: modified time plus length is
        // what the poll compares, so this is detected even on a coarse clock.
        scratch.write("a.json", br#"{"v":2,"longer":true}"#);
        let changed = db.poll_changes();
        assert_eq!(changed, vec!["a.json".to_string()]);
        assert!(!db.cached("a.json"));
        assert!(db.cached("b.json"), "untouched entries stay cached");
        assert_eq!(db.load_json("a.json").unwrap().get_u32("v", 0), 2);
        assert!(db.poll_changes().is_empty());

        // Deleting a cached file also counts as a change.
        fs::remove_file(scratch.path().join("b.json")).unwrap();
        assert_eq!(db.poll_changes(), vec!["b.json".to_string()]);
        assert!(matches!(
            db.load_json("b.json").unwrap_err(),
            AssetError::NotFound { .. }
        ));

        // Two files changing at once come back in sorted order.
        scratch.write("a.json", br#"{"v":3}"#);
        scratch.write("c.json", br#"{"v":1}"#);
        db.load_json("c.json").unwrap();
        scratch.write("a.json", br#"{"v":4,"more":true}"#);
        scratch.write("c.json", br#"{"v":2,"more":true}"#);
        assert_eq!(
            db.poll_changes(),
            vec!["a.json".to_string(), "c.json".to_string()]
        );
    }

    #[test]
    fn resolve_keeps_everything_under_the_root() {
        let scratch = Scratch::new("resolve");
        let db = scratch.db();
        assert_eq!(
            db.resolve("textures/a.png"),
            scratch.path().join("textures/a.png")
        );
        assert_eq!(
            db.resolve("/textures/a.png"),
            scratch.path().join("textures/a.png")
        );
        assert_eq!(db.resolve("a.png"), scratch.path().join("a.png"));
        assert!(db.resolve("a.png").starts_with(scratch.path()));

        // Both spellings find the same file.
        scratch.write("deep/nested/thing.txt", b"ok");
        let mut db = scratch.db();
        assert_eq!(*db.load_string("deep/nested/thing.txt").unwrap(), "ok");
        assert_eq!(*db.load_string("/deep/nested/thing.txt").unwrap(), "ok");
    }

    #[test]
    fn write_atomic_replaces_files_and_cleans_up() {
        let scratch = Scratch::new("atomic");
        let target = scratch.path().join("out/deep/file.bin");

        write_atomic(&target, b"first").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");

        write_atomic(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");

        let mut tmp = target.as_os_str().to_os_string();
        tmp.push(".tmp");
        assert!(
            !PathBuf::from(tmp).exists(),
            "the temp file is renamed away"
        );

        // An image survives the round trip through the atomic writer.
        let image = Image::new(3, 3, noxel_core::math::Color8::MAGENTA);
        image.save_png(&target).unwrap();
        assert_eq!(Image::load_png(&target).unwrap(), image);
    }
}
