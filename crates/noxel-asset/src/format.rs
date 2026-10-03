//! The engine's on-disk data formats, parsed from JSON.
//!
//! These types are the contract between the asset files (authored by hand or
//! emitted by `noxel-gen`) and the world generator, the renderer and the
//! prefab tooling. They are plain data: no behaviour beyond parsing, writing
//! and the odd lookup.
//!
//! ## Error paths
//!
//! Every `from_json` reports a [`FormatError`] whose `path` names the exact
//! field that was wrong, using the same syntax you would write in code:
//!
//! ```
//! use noxel_asset::format::TileSet;
//! use noxel_asset::json::parse_str;
//!
//! let broken = parse_str(r#"{"name":"ground","texture":"t.png","tile_size":16,
//!     "tiles":[{"id":0,"name":"grass","uv":[0,0,16]}]}"#).unwrap();
//! let err = TileSet::from_json(&broken).unwrap_err();
//! assert_eq!(err.path, "tiles[0].uv");
//! ```
//!
//! Missing required fields produce the same shaped error with the message
//! "missing required field"; unknown fields are ignored so that a newer
//! generator can add metadata without breaking an older engine.

use std::fmt;

use noxel_core::math::{Aabb, Color8, Palette, Vec3};

use crate::json::JsonValue;

/// A JSON field was missing or had the wrong shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    /// Dotted path to the offending field, e.g. `"tiles[3].uv"`.
    pub path: String,
    /// What was wrong with it.
    pub message: String,
}

impl FormatError {
    /// Builds an error for `path`.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }

    /// "missing required field".
    pub fn missing(path: impl Into<String>) -> Self {
        Self::new(path, "missing required field")
    }

    /// "expected X, found Y".
    pub fn expected(path: impl Into<String>, expected: &str, found: &JsonValue) -> Self {
        Self::new(
            path,
            format!("expected {expected}, found {}", found.type_name()),
        )
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for FormatError {}

/// Joins a parent path and a key: `("tiles[3]", "uv")` becomes `"tiles[3].uv"`.
fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

// --- field readers ---------------------------------------------------------

fn field<'a>(v: &'a JsonValue, key: &str, path: &str) -> Result<&'a JsonValue, FormatError> {
    v.get(key)
        .ok_or_else(|| FormatError::missing(child(path, key)))
}

fn req_str(v: &JsonValue, key: &str, path: &str) -> Result<String, FormatError> {
    let value = field(v, key, path)?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| FormatError::expected(child(path, key), "a string", value))
}

fn opt_str(v: &JsonValue, key: &str) -> Option<String> {
    v.get(key).and_then(JsonValue::as_str).map(str::to_string)
}

fn opt_f32(v: &JsonValue, key: &str) -> Option<f32> {
    v.get(key).and_then(JsonValue::as_f32)
}

fn req_u32(v: &JsonValue, key: &str, path: &str) -> Result<u32, FormatError> {
    let value = field(v, key, path)?;
    value
        .as_u32()
        .ok_or_else(|| FormatError::expected(child(path, key), "a non-negative integer", value))
}

fn opt_u32(v: &JsonValue, key: &str) -> Option<u32> {
    v.get(key).and_then(JsonValue::as_u32)
}

fn opt_bool(v: &JsonValue, key: &str) -> Option<bool> {
    v.get(key).and_then(JsonValue::as_bool)
}

fn req_u8(v: &JsonValue, key: &str, path: &str) -> Result<u8, FormatError> {
    let value = field(v, key, path)?;
    let number = value.as_i64().ok_or_else(|| {
        FormatError::expected(child(path, key), "an integer from 0 to 255", value)
    })?;
    u8::try_from(number).map_err(|_| {
        FormatError::new(
            child(path, key),
            format!("{number} is out of the 0..=255 range"),
        )
    })
}

fn req_array<'a>(
    v: &'a JsonValue,
    key: &str,
    path: &str,
) -> Result<&'a Vec<JsonValue>, FormatError> {
    let value = field(v, key, path)?;
    value
        .as_array()
        .ok_or_else(|| FormatError::expected(child(path, key), "an array", value))
}

fn number_array<const N: usize>(
    v: &JsonValue,
    key: &str,
    path: &str,
) -> Result<[f32; N], FormatError> {
    let path = child(path, key);
    let values = v
        .get(key)
        .ok_or_else(|| FormatError::missing(&path))?
        .as_array()
        .ok_or_else(|| FormatError::new(&path, "expected an array of numbers"))?;
    if values.len() != N {
        return Err(FormatError::new(
            &path,
            format!("expected {N} numbers, found {}", values.len()),
        ));
    }
    let mut out = [0.0f32; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = value
            .as_f32()
            .ok_or_else(|| FormatError::new(&path, "expected an array of numbers"))?;
    }
    Ok(out)
}

fn uint_array<const N: usize>(
    v: &JsonValue,
    key: &str,
    path: &str,
) -> Result<[u32; N], FormatError> {
    let path = child(path, key);
    let values = v
        .get(key)
        .ok_or_else(|| FormatError::missing(&path))?
        .as_array()
        .ok_or_else(|| FormatError::new(&path, "expected an array of integers"))?;
    if values.len() != N {
        return Err(FormatError::new(
            &path,
            format!("expected {N} integers, found {}", values.len()),
        ));
    }
    let mut out = [0u32; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = value
            .as_u32()
            .ok_or_else(|| FormatError::new(&path, "expected an array of non-negative integers"))?;
    }
    Ok(out)
}

fn byte_array<const N: usize>(
    v: &JsonValue,
    key: &str,
    path: &str,
) -> Result<[u8; N], FormatError> {
    let path = child(path, key);
    let values = v
        .get(key)
        .ok_or_else(|| FormatError::missing(&path))?
        .as_array()
        .ok_or_else(|| FormatError::new(&path, "expected an array of integers"))?;
    byte_values::<N>(values, &path)
}

fn byte_values<const N: usize>(values: &[JsonValue], path: &str) -> Result<[u8; N], FormatError> {
    if values.len() != N {
        return Err(FormatError::new(
            path,
            format!("expected {N} integers, found {}", values.len()),
        ));
    }
    let mut out = [0u8; N];
    for (slot, value) in out.iter_mut().zip(values) {
        let number = value
            .as_i64()
            .ok_or_else(|| FormatError::new(path, "expected an array of integers"))?;
        *slot = u8::try_from(number)
            .map_err(|_| FormatError::new(path, format!("{number} is out of the 0..=255 range")))?;
    }
    Ok(out)
}

/// Reads an optional array field, rejecting a present-but-wrong-typed value.
fn opt_array_checked<'a>(
    v: &'a JsonValue,
    key: &str,
    path: &str,
) -> Result<Option<&'a Vec<JsonValue>>, FormatError> {
    match v.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_array()
            .map(Some)
            .ok_or_else(|| FormatError::expected(child(path, key), "an array", value)),
    }
}

fn vec3(v: &JsonValue, key: &str, path: &str) -> Result<Vec3, FormatError> {
    let [x, y, z] = number_array::<3>(v, key, path)?;
    Ok(Vec3::new(x, y, z))
}

fn strings(v: &JsonValue, key: &str, path: &str) -> Result<Vec<String>, FormatError> {
    let Some(value) = v.get(key) else {
        return Ok(Vec::new());
    };
    let base = child(path, key);
    let items = value
        .as_array()
        .ok_or_else(|| FormatError::expected(&base, "an array of strings", value))?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let text = item
            .as_str()
            .ok_or_else(|| FormatError::expected(format!("{base}[{i}]"), "a string", item))?;
        out.push(text.to_string());
    }
    Ok(out)
}

fn json_strings(values: &[String]) -> JsonValue {
    JsonValue::Array(values.iter().map(|s| JsonValue::from(s.as_str())).collect())
}

fn json_vec3(v: Vec3) -> JsonValue {
    JsonValue::array([
        JsonValue::from(v.x),
        JsonValue::from(v.y),
        JsonValue::from(v.z),
    ])
}

// --- palette ---------------------------------------------------------------

/// A palette file: a named list of colours.
///
/// ```
/// use noxel_asset::format::PaletteFile;
/// use noxel_asset::json::parse_str;
///
/// let file = PaletteFile::from_json(&parse_str(
///     r##"{"name":"dawn","entries":[{"name":"grass","color":"#2E8B2E"}]}"##,
/// )
/// .unwrap())
/// .unwrap();
/// assert_eq!(file.entries[0].color, noxel_core::math::Color8::rgb(0x2E, 0x8B, 0x2E));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteFile {
    /// Palette name, e.g. `"dawn"`.
    pub name: String,
    /// The colours, in authoring order.
    pub entries: Vec<PaletteEntryFile>,
}

/// One named colour in a [`PaletteFile`].
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntryFile {
    /// Entry name, e.g. `"grass_dark"`.
    pub name: String,
    /// The colour.
    pub color: Color8,
    /// Optional note for tooling and docs.
    pub description: Option<String>,
}

impl PaletteFile {
    /// Parses a palette file.
    pub fn from_json(v: &JsonValue) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected("", "an object", v));
        }
        let name = req_str(v, "name", "")?;
        let entries_json = req_array(v, "entries", "")?;
        let mut entries = Vec::with_capacity(entries_json.len());
        for (i, entry) in entries_json.iter().enumerate() {
            let path = format!("entries[{i}]");
            if !entry.is_object() {
                return Err(FormatError::expected(&path, "an object", entry));
            }
            let entry_name = req_str(entry, "name", &path)?;
            let color_value = field(entry, "color", &path)?;
            let color = parse_color(color_value, &child(&path, "color"))?;
            let description = opt_str(entry, "description");
            entries.push(PaletteEntryFile {
                name: entry_name,
                color,
                description,
            });
        }
        Ok(Self { name, entries })
    }

    /// Writes the palette back to JSON. Colours are written as `#RRGGBBAA`.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let mut fields = vec![
                    ("name".to_string(), JsonValue::from(entry.name.as_str())),
                    (
                        "color".to_string(),
                        JsonValue::from(format_color(entry.color)),
                    ),
                ];
                if let Some(description) = &entry.description {
                    fields.push((
                        "description".to_string(),
                        JsonValue::from(description.as_str()),
                    ));
                }
                JsonValue::Object(fields)
            })
            .collect();
        JsonValue::object([
            ("name", JsonValue::from(self.name.as_str())),
            ("entries", JsonValue::Array(entries)),
        ])
    }

    /// Converts to the engine's [`Palette`].
    ///
    /// `noxel_core::PaletteEntry` stores its name as a `&'static str`, so the
    /// names are leaked here. That is a deliberate, one-off cost of loading a
    /// palette: a palette is small, loaded once, and lives for the process (and
    /// this crate forbids `unsafe`, so interning is not an option). Reloading
    /// the same palette repeatedly with hot reload will leak one small string
    /// per entry per reload.
    #[must_use]
    pub fn to_palette(&self) -> Palette {
        let mut palette = Palette::new();
        for entry in &self.entries {
            let name: &'static str = Box::leak(entry.name.clone().into_boxed_str());
            palette.push(name, entry.color);
        }
        palette
    }
}

/// Formats a colour as `#RRGGBBAA`.
#[must_use]
pub fn format_color(color: Color8) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        color.r, color.g, color.b, color.a
    )
}

/// Parses `#RGB`, `#RRGGBB`, `#RRGGBBAA`, the same with a `0x` prefix, or a
/// bare integer.
///
/// A bare integer of `0xFFFFFF` or less is read as opaque `0xRRGGBB`; anything
/// larger is read as `0xAARRGGBB`, matching [`Color8::from_hex`].
pub fn parse_color(v: &JsonValue, path: &str) -> Result<Color8, FormatError> {
    if let Some(text) = v.as_str() {
        let trimmed = text.trim();
        let hex = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("0x"))
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        return match hex.len() {
            3 => {
                let mut digits = [0u8; 3];
                for (slot, c) in digits.iter_mut().zip(hex.chars()) {
                    let value = c.to_digit(16).ok_or_else(|| {
                        FormatError::new(path, format!("`{text}` is not a hex colour"))
                    })?;
                    *slot = (value * 17) as u8;
                }
                Ok(Color8::new(digits[0], digits[1], digits[2], 255))
            }
            6 | 8 => {
                let mut bytes = [255u8; 4];
                for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
                    let pair = std::str::from_utf8(pair).unwrap_or("");
                    bytes[i] = u8::from_str_radix(pair, 16).map_err(|_| {
                        FormatError::new(path, format!("`{text}` is not a hex colour"))
                    })?;
                }
                Ok(Color8::new(bytes[0], bytes[1], bytes[2], bytes[3]))
            }
            _ => Err(FormatError::new(
                path,
                format!("`{text}` is neither #RGB, #RRGGBB nor #RRGGBBAA"),
            )),
        };
    }
    if let Some(number) = v.as_u32() {
        return Ok(if number > 0x00FF_FFFF {
            Color8::from_hex(number)
        } else {
            Color8::new(
                ((number >> 16) & 0xFF) as u8,
                ((number >> 8) & 0xFF) as u8,
                (number & 0xFF) as u8,
                255,
            )
        });
    }
    Err(FormatError::expected(
        path,
        "a hex colour string or an integer",
        v,
    ))
}

// --- tiles -----------------------------------------------------------------

/// Gameplay flags for a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileFlags {
    /// Actors may walk on this tile.
    pub walkable: bool,
    /// The tile blocks line of sight (used by the visibility system).
    pub blocks_sight: bool,
    /// The tile can hide the player when the camera is behind it.
    pub occluder: bool,
    /// Deep water: not walkable, but swimmable by special actors.
    pub water: bool,
    /// Paved: the pathfinder prefers it and props may not spawn on it.
    pub road: bool,
    /// The world generator may place buildings here.
    pub buildable: bool,
}

impl Default for TileFlags {
    /// Everything off except [`TileFlags::walkable`]: a plain floor tile.
    fn default() -> Self {
        Self {
            walkable: true,
            blocks_sight: false,
            occluder: false,
            water: false,
            road: false,
            buildable: false,
        }
    }
}

impl TileFlags {
    /// A tile an actor can stand on.
    #[must_use]
    pub const fn walkable() -> Self {
        Self {
            walkable: true,
            blocks_sight: false,
            occluder: false,
            water: false,
            road: false,
            buildable: true,
        }
    }

    /// A tile that blocks both movement and sight, such as a wall.
    #[must_use]
    pub const fn solid() -> Self {
        Self {
            walkable: false,
            blocks_sight: true,
            occluder: true,
            water: false,
            road: false,
            buildable: false,
        }
    }

    /// A tile with no flags at all set.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            walkable: false,
            blocks_sight: false,
            occluder: false,
            water: false,
            road: false,
            buildable: false,
        }
    }

    /// Reads the flags from a JSON object. Absent fields keep their default,
    /// and `null` means "all defaults".
    pub fn from_json(v: &JsonValue, path: &str) -> Result<Self, FormatError> {
        if v.is_null() {
            return Ok(Self::default());
        }
        if !v.is_object() {
            return Err(FormatError::expected(path, "an object", v));
        }
        let defaults = Self::default();
        Ok(Self {
            walkable: opt_bool(v, "walkable").unwrap_or(defaults.walkable),
            blocks_sight: opt_bool(v, "blocks_sight").unwrap_or(defaults.blocks_sight),
            occluder: opt_bool(v, "occluder").unwrap_or(defaults.occluder),
            water: opt_bool(v, "water").unwrap_or(defaults.water),
            road: opt_bool(v, "road").unwrap_or(defaults.road),
            buildable: opt_bool(v, "buildable").unwrap_or(defaults.buildable),
        })
    }

    /// Writes the flags as a JSON object.
    #[must_use]
    pub fn to_json(self) -> JsonValue {
        JsonValue::object([
            ("walkable", JsonValue::from(self.walkable)),
            ("blocks_sight", JsonValue::from(self.blocks_sight)),
            ("occluder", JsonValue::from(self.occluder)),
            ("water", JsonValue::from(self.water)),
            ("road", JsonValue::from(self.road)),
            ("buildable", JsonValue::from(self.buildable)),
        ])
    }

    /// True when no flag is set.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !(self.walkable
            || self.blocks_sight
            || self.occluder
            || self.water
            || self.road
            || self.buildable)
    }
}

/// One tile in a [`TileSet`].
#[derive(Clone, Debug, PartialEq)]
pub struct TileDef {
    /// Stable numeric id used by the world voxel format.
    pub id: u32,
    /// Name, e.g. `"grass"`.
    pub name: String,
    /// Texture file, or an empty string to use the tile set's texture.
    pub texture: String,
    /// `[x, y, w, h]` region inside the texture, in pixels.
    pub uv: [u32; 4],
    /// Gameplay flags.
    pub flags: TileFlags,
    /// Height of the tile's top surface above the ground plane, in metres.
    pub height: f32,
    /// Draw layer; higher layers win when tiles overlap in a column.
    pub layer: u32,
}

impl TileDef {
    /// Parses one tile definition. `path` is used for error reporting, e.g.
    /// `"tiles[3]"`.
    pub fn from_json(v: &JsonValue, path: &str) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected(path, "an object", v));
        }
        let id = req_u32(v, "id", path)?;
        let name = req_str(v, "name", path)?;
        let texture = opt_str(v, "texture").unwrap_or_default();
        let uv = uint_array::<4>(v, "uv", path)?;
        let flags = match v.get("flags") {
            Some(flags_json) => TileFlags::from_json(flags_json, &child(path, "flags"))?,
            None => TileFlags::default(),
        };
        let height = opt_f32(v, "height").unwrap_or(0.0);
        let layer = opt_u32(v, "layer").unwrap_or(0);
        Ok(Self {
            id,
            name,
            texture,
            uv,
            flags,
            height,
            layer,
        })
    }

    /// Writes one tile definition.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("id", JsonValue::from(self.id)),
            ("name", JsonValue::from(self.name.as_str())),
            ("texture", JsonValue::from(self.texture.as_str())),
            (
                "uv",
                JsonValue::array(
                    self.uv
                        .iter()
                        .map(|n| JsonValue::from(*n))
                        .collect::<Vec<_>>(),
                ),
            ),
            ("flags", self.flags.to_json()),
            ("height", JsonValue::from(self.height)),
            ("layer", JsonValue::from(self.layer)),
        ])
    }

    /// Width of the tile's texture region in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.uv[2]
    }

    /// Height of the tile's texture region in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.uv[3]
    }
}

/// A named collection of tiles that share a texture and a tile size.
#[derive(Clone, Debug, PartialEq)]
pub struct TileSet {
    /// Tile set name, e.g. `"town"`.
    pub name: String,
    /// Default texture for tiles that do not name their own.
    pub texture: String,
    /// Edge length of one tile in pixels.
    pub tile_size: u32,
    /// The tiles.
    pub tiles: Vec<TileDef>,
}

impl Default for TileSet {
    /// An empty tile set, used when no asset tree is present.
    ///
    /// The world generator treats a missing tile as "fall back to the biome's
    /// base tile", so an empty set yields a plain but valid world rather than a
    /// failure.
    fn default() -> Self {
        Self {
            name: String::new(),
            texture: String::new(),
            tile_size: 16,
            tiles: Vec::new(),
        }
    }
}

impl TileSet {
    /// Parses a tile set.
    pub fn from_json(v: &JsonValue) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected("", "an object", v));
        }
        let name = req_str(v, "name", "")?;
        let texture = req_str(v, "texture", "")?;
        let tile_size = req_u32(v, "tile_size", "")?;
        if tile_size == 0 {
            return Err(FormatError::new("tile_size", "must be greater than zero"));
        }
        let tiles_json = req_array(v, "tiles", "")?;
        let mut tiles = Vec::with_capacity(tiles_json.len());
        for (i, tile) in tiles_json.iter().enumerate() {
            tiles.push(TileDef::from_json(tile, &format!("tiles[{i}]"))?);
        }
        Ok(Self {
            name,
            texture,
            tile_size,
            tiles,
        })
    }

    /// Writes the tile set back to JSON.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("name", JsonValue::from(self.name.as_str())),
            ("texture", JsonValue::from(self.texture.as_str())),
            ("tile_size", JsonValue::from(self.tile_size)),
            (
                "tiles",
                JsonValue::Array(self.tiles.iter().map(TileDef::to_json).collect()),
            ),
        ])
    }

    /// Looks a tile up by id.
    #[must_use]
    pub fn tile(&self, id: u32) -> Option<&TileDef> {
        self.tiles.iter().find(|tile| tile.id == id)
    }

    /// Looks a tile up by name.
    #[must_use]
    pub fn tile_by_name(&self, name: &str) -> Option<&TileDef> {
        self.tiles.iter().find(|tile| tile.name == name)
    }

    /// The tiles that hide whatever is behind them.
    pub fn occluders(&self) -> impl Iterator<Item = &TileDef> {
        self.tiles.iter().filter(|tile| tile.flags.occluder)
    }

    /// The texture a tile should be drawn from: its own, or the set's.
    #[must_use]
    pub fn texture_of<'a>(&'a self, tile: &'a TileDef) -> &'a str {
        if tile.texture.is_empty() {
            &self.texture
        } else {
            &tile.texture
        }
    }

    /// Number of tiles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// True when the set has no tiles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

// --- prefabs ---------------------------------------------------------------

/// One voxel of a [`Prefab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefabVoxel {
    /// Grid x, in tiles.
    pub x: u8,
    /// Grid y (up), in tiles.
    pub y: u8,
    /// Grid z (depth), in tiles.
    pub z: u8,
    /// The tile id placed here.
    pub tile: u32,
}

/// A prop placed inside a [`Prefab`] (a sign, a lamp, a crate).
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabProp {
    /// Prop kind, resolved by the game code.
    pub kind: String,
    /// Position in tiles, relative to the prefab origin.
    pub position: Vec3,
    /// Rotation about the up axis, in radians.
    pub yaw: f32,
    /// Uniform scale multiplier.
    pub scale: f32,
}

/// A named marker inside a [`Prefab`] (a player start, a patrol node, a door).
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabSpawn {
    /// Marker name.
    pub name: String,
    /// Position in tiles, relative to the prefab origin.
    pub position: Vec3,
    /// Rotation about the up axis, in radians.
    pub yaw: f32,
}

/// A reusable building block: voxels plus props and markers.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefab {
    /// Prefab name, e.g. `"house_small"`.
    pub name: String,
    /// `[x, y, z]` size in tiles.
    pub size: [u8; 3],
    /// Free-form tags used by the world generator, e.g. `["residential"]`.
    pub tags: Vec<String>,
    /// The filled voxels. Absent cells are empty space.
    pub voxels: Vec<PrefabVoxel>,
    /// Props attached to the prefab.
    pub props: Vec<PrefabProp>,
    /// Named markers.
    pub spawns: Vec<PrefabSpawn>,
    /// Voxel bounds `[x0, y0, z0, x1, y1, z1]` of the parts that hide the
    /// player when the camera is behind them.
    pub occluders: Vec<[u8; 6]>,
}

impl Prefab {
    /// Parses a prefab.
    pub fn from_json(v: &JsonValue) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected("", "an object", v));
        }
        let name = req_str(v, "name", "")?;
        let size = byte_array::<3>(v, "size", "")?;
        let tags = strings(v, "tags", "")?;

        let mut voxels = Vec::new();
        if let Some(list) = opt_array_checked(v, "voxels", "")? {
            for (i, entry) in list.iter().enumerate() {
                let path = format!("voxels[{i}]");
                if !entry.is_object() {
                    return Err(FormatError::expected(&path, "an object", entry));
                }
                voxels.push(PrefabVoxel {
                    x: req_u8(entry, "x", &path)?,
                    y: req_u8(entry, "y", &path)?,
                    z: req_u8(entry, "z", &path)?,
                    tile: req_u32(entry, "tile", &path)?,
                });
            }
        }

        let mut props = Vec::new();
        if let Some(list) = opt_array_checked(v, "props", "")? {
            for (i, entry) in list.iter().enumerate() {
                let path = format!("props[{i}]");
                if !entry.is_object() {
                    return Err(FormatError::expected(&path, "an object", entry));
                }
                props.push(PrefabProp {
                    kind: req_str(entry, "kind", &path)?,
                    position: vec3(entry, "position", &path)?,
                    yaw: opt_f32(entry, "yaw").unwrap_or(0.0),
                    scale: opt_f32(entry, "scale").unwrap_or(1.0),
                });
            }
        }

        let mut spawns = Vec::new();
        if let Some(list) = opt_array_checked(v, "spawns", "")? {
            for (i, entry) in list.iter().enumerate() {
                let path = format!("spawns[{i}]");
                if !entry.is_object() {
                    return Err(FormatError::expected(&path, "an object", entry));
                }
                spawns.push(PrefabSpawn {
                    name: req_str(entry, "name", &path)?,
                    position: vec3(entry, "position", &path)?,
                    yaw: opt_f32(entry, "yaw").unwrap_or(0.0),
                });
            }
        }

        let mut occluders = Vec::new();
        if let Some(list) = opt_array_checked(v, "occluders", "")? {
            for (i, entry) in list.iter().enumerate() {
                let path = format!("occluders[{i}]");
                let values = entry
                    .as_array()
                    .ok_or_else(|| FormatError::expected(&path, "an array of 6 integers", entry))?;
                occluders.push(byte_values::<6>(values, &path)?);
            }
        }

        Ok(Self {
            name,
            size,
            tags,
            voxels,
            props,
            spawns,
            occluders,
        })
    }

    /// Writes the prefab back to JSON.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let voxels = self
            .voxels
            .iter()
            .map(|voxel| {
                JsonValue::object([
                    ("x", JsonValue::from(u32::from(voxel.x))),
                    ("y", JsonValue::from(u32::from(voxel.y))),
                    ("z", JsonValue::from(u32::from(voxel.z))),
                    ("tile", JsonValue::from(voxel.tile)),
                ])
            })
            .collect::<Vec<_>>();
        let props = self
            .props
            .iter()
            .map(|prop| {
                JsonValue::object([
                    ("kind", JsonValue::from(prop.kind.as_str())),
                    ("position", json_vec3(prop.position)),
                    ("yaw", JsonValue::from(prop.yaw)),
                    ("scale", JsonValue::from(prop.scale)),
                ])
            })
            .collect::<Vec<_>>();
        let spawns = self
            .spawns
            .iter()
            .map(|spawn| {
                JsonValue::object([
                    ("name", JsonValue::from(spawn.name.as_str())),
                    ("position", json_vec3(spawn.position)),
                    ("yaw", JsonValue::from(spawn.yaw)),
                ])
            })
            .collect::<Vec<_>>();
        let occluders = self
            .occluders
            .iter()
            .map(|bounds| {
                JsonValue::array(
                    bounds
                        .iter()
                        .map(|n| JsonValue::from(u32::from(*n)))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        JsonValue::object([
            ("name", JsonValue::from(self.name.as_str())),
            (
                "size",
                JsonValue::array(
                    self.size
                        .iter()
                        .map(|n| JsonValue::from(u32::from(*n)))
                        .collect::<Vec<_>>(),
                ),
            ),
            ("tags", json_strings(&self.tags)),
            ("voxels", JsonValue::Array(voxels)),
            ("props", JsonValue::Array(props)),
            ("spawns", JsonValue::Array(spawns)),
            ("occluders", JsonValue::Array(occluders)),
        ])
    }

    /// The tile id at a voxel coordinate, or `None` when the cell is empty.
    ///
    /// This is a linear search: prefabs are small and are expanded into the
    /// world once at load time, so an index would cost more than it saves.
    #[must_use]
    pub fn voxel(&self, x: u8, y: u8, z: u8) -> Option<u32> {
        self.voxels
            .iter()
            .find(|voxel| voxel.x == x && voxel.y == y && voxel.z == z)
            .map(|voxel| voxel.tile)
    }

    /// The `(x, z)` footprint in tiles.
    #[must_use]
    pub fn footprint(&self) -> (u32, u32) {
        (u32::from(self.size[0]), u32::from(self.size[2]))
    }

    /// Volume of the prefab's bounding box in cubic tiles.
    #[must_use]
    pub fn volume(&self) -> u32 {
        u32::from(self.size[0]) * u32::from(self.size[1]) * u32::from(self.size[2])
    }

    /// The world-space axis-aligned box the prefab occupies when its
    /// `(0, 0, 0)` corner is placed at `origin`.
    ///
    /// `y` is up: the box spans `size[1] * tile_size` vertically. The box covers
    /// the whole declared volume, not just the filled voxels, so callers can
    /// cull and collide without walking the voxel list.
    #[must_use]
    pub fn bounds_world(&self, origin: Vec3, tile_size: f32) -> Aabb {
        let size = Vec3::new(
            f32::from(self.size[0]) * tile_size,
            f32::from(self.size[1]) * tile_size,
            f32::from(self.size[2]) * tile_size,
        );
        Aabb::new(origin, origin + size)
    }

    /// Looks a spawn marker up by name.
    #[must_use]
    pub fn spawn(&self, name: &str) -> Option<&PrefabSpawn> {
        self.spawns.iter().find(|spawn| spawn.name == name)
    }

    /// True when the prefab carries `tag`.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

// --- sprites ---------------------------------------------------------------

/// One animation frame of a [`SpriteFile`].
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteFrame {
    /// Frame name, e.g. `"walk_02"`.
    pub name: String,
    /// `[x, y, w, h]` region inside the texture, in pixels.
    pub uv: [u32; 4],
    /// How long the frame is shown, in seconds.
    pub duration: f32,
}

/// A sprite sheet description: one texture plus its frames.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteFile {
    /// Sprite name.
    pub name: String,
    /// Texture file holding the frames.
    pub texture: String,
    /// The default frame's `[x, y, w, h]` region.
    pub uv: [u32; 4],
    /// Normalised anchor inside a frame; defaults to the centre.
    pub pivot: [f32; 2],
    /// Display size in world units; defaults to the frame size in pixels.
    pub size: [f32; 2],
    /// Animation frames, in order.
    pub frames: Vec<SpriteFrame>,
}

impl SpriteFile {
    /// Parses a sprite file.
    pub fn from_json(v: &JsonValue) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected("", "an object", v));
        }
        let name = req_str(v, "name", "")?;
        let texture = req_str(v, "texture", "")?;
        let uv = uint_array::<4>(v, "uv", "")?;
        let pivot = match v.get("pivot") {
            Some(_) => number_array::<2>(v, "pivot", "")?,
            None => [0.5, 0.5],
        };
        let size = match v.get("size") {
            Some(_) => number_array::<2>(v, "size", "")?,
            None => [uv[2] as f32, uv[3] as f32],
        };
        let mut frames = Vec::new();
        if let Some(list) = opt_array_checked(v, "frames", "")? {
            for (i, entry) in list.iter().enumerate() {
                let path = format!("frames[{i}]");
                if !entry.is_object() {
                    return Err(FormatError::expected(&path, "an object", entry));
                }
                frames.push(SpriteFrame {
                    name: req_str(entry, "name", &path)?,
                    uv: uint_array::<4>(entry, "uv", &path)?,
                    duration: opt_f32(entry, "duration").unwrap_or(0.1),
                });
            }
        }
        Ok(Self {
            name,
            texture,
            uv,
            pivot,
            size,
            frames,
        })
    }

    /// Writes the sprite file back to JSON.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let frames = self
            .frames
            .iter()
            .map(|frame| {
                JsonValue::object([
                    ("name", JsonValue::from(frame.name.as_str())),
                    (
                        "uv",
                        JsonValue::array(
                            frame
                                .uv
                                .iter()
                                .map(|n| JsonValue::from(*n))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    ("duration", JsonValue::from(frame.duration)),
                ])
            })
            .collect::<Vec<_>>();
        JsonValue::object([
            ("name", JsonValue::from(self.name.as_str())),
            ("texture", JsonValue::from(self.texture.as_str())),
            (
                "uv",
                JsonValue::array(
                    self.uv
                        .iter()
                        .map(|n| JsonValue::from(*n))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "pivot",
                JsonValue::array([
                    JsonValue::from(self.pivot[0]),
                    JsonValue::from(self.pivot[1]),
                ]),
            ),
            (
                "size",
                JsonValue::array([JsonValue::from(self.size[0]), JsonValue::from(self.size[1])]),
            ),
            ("frames", JsonValue::Array(frames)),
        ])
    }

    /// Total animation length in seconds.
    #[must_use]
    pub fn duration(&self) -> f32 {
        self.frames.iter().map(|frame| frame.duration).sum()
    }

    /// Looks a frame up by name.
    #[must_use]
    pub fn frame(&self, name: &str) -> Option<&SpriteFrame> {
        self.frames.iter().find(|frame| frame.name == name)
    }
}

// --- manifest --------------------------------------------------------------

/// The list of assets a game or a scene needs.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AssetManifest {
    /// Manifest name.
    pub name: String,
    /// Free-form version string.
    pub version: String,
    /// Tile set files, relative to the asset root.
    pub tile_sets: Vec<String>,
    /// Prefab files.
    pub prefabs: Vec<String>,
    /// Palette files.
    pub palettes: Vec<String>,
    /// Sprite files.
    pub sprites: Vec<String>,
}

impl AssetManifest {
    /// Parses a manifest. Every list is optional and defaults to empty.
    pub fn from_json(v: &JsonValue) -> Result<Self, FormatError> {
        if !v.is_object() {
            return Err(FormatError::expected("", "an object", v));
        }
        Ok(Self {
            name: req_str(v, "name", "")?,
            version: req_str(v, "version", "")?,
            tile_sets: strings(v, "tile_sets", "")?,
            prefabs: strings(v, "prefabs", "")?,
            palettes: strings(v, "palettes", "")?,
            sprites: strings(v, "sprites", "")?,
        })
    }

    /// Writes the manifest back to JSON.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("name", JsonValue::from(self.name.as_str())),
            ("version", JsonValue::from(self.version.as_str())),
            ("tile_sets", json_strings(&self.tile_sets)),
            ("prefabs", json_strings(&self.prefabs)),
            ("palettes", json_strings(&self.palettes)),
            ("sprites", json_strings(&self.sprites)),
        ])
    }

    /// Total number of asset paths listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tile_sets.len() + self.prefabs.len() + self.palettes.len() + self.sprites.len()
    }

    /// True when the manifest lists no assets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_str;

    const TILE_SET: &str = r#"{
        "name": "town",
        "texture": "textures/town.png",
        "tile_size": 16,
        "future_field": {"ignored": true},
        "tiles": [
            {"id": 0, "name": "grass", "uv": [0, 0, 16, 16], "height": 0.0, "layer": 0},
            {"id": 1, "name": "wall", "texture": "textures/walls.png", "uv": [16, 0, 16, 32],
             "flags": {"walkable": false, "blocks_sight": true, "occluder": true},
             "height": 3.0, "layer": 1},
            {"id": 2, "name": "road", "uv": [32, 0, 16, 16], "flags": {"road": true, "buildable": true}}
        ]
    }"#;

    #[test]
    fn tile_set_round_trip() {
        let json = parse_str(TILE_SET).unwrap();
        let set = TileSet::from_json(&json).unwrap();
        assert_eq!(set.name, "town");
        assert_eq!(set.texture, "textures/town.png");
        assert_eq!(set.tile_size, 16);
        assert_eq!(set.len(), 3);
        assert!(!set.is_empty());

        let grass = set.tile(0).unwrap();
        assert_eq!(grass.name, "grass");
        assert!(grass.flags.walkable);
        assert!(!grass.flags.occluder);
        assert_eq!(grass.uv, [0, 0, 16, 16]);
        assert_eq!(grass.height, 0.0);
        assert!(grass.texture.is_empty());
        assert_eq!(set.texture_of(grass), "textures/town.png");
        assert_eq!(grass.width(), 16);
        assert_eq!(grass.height(), 16);

        let wall = set.tile_by_name("wall").unwrap();
        assert_eq!(wall.id, 1);
        assert!(!wall.flags.walkable);
        assert!(wall.flags.blocks_sight);
        assert_eq!(wall.layer, 1);
        assert_eq!(set.texture_of(wall), "textures/walls.png");
        assert_eq!(set.occluders().count(), 1);
        assert_eq!(set.occluders().next().unwrap().name, "wall");
        assert!(set.tile(99).is_none());
        assert!(set.tile_by_name("nope").is_none());

        // The default flags for a tile with a partial flags object.
        let road = set.tile_by_name("road").unwrap();
        assert!(road.flags.road && road.flags.buildable && road.flags.walkable);
        assert!(!road.flags.water);

        let written = set.to_json();
        assert_eq!(TileSet::from_json(&written).unwrap(), set);
        let text = written.to_string();
        assert_eq!(TileSet::from_json(&parse_str(&text).unwrap()).unwrap(), set);
    }

    #[test]
    fn tile_set_error_paths_are_precise() {
        let json = parse_str(r#"{"name":"x","texture":"t.png","tile_size":16,"tiles":[{"id":0,"name":"a","uv":[0,0,16]}]}"#).unwrap();
        let err = TileSet::from_json(&json).unwrap_err();
        assert_eq!(err.path, "tiles[0].uv");
        assert!(err.message.contains("4 integers"), "{}", err.message);
        assert!(err.to_string().starts_with("tiles[0].uv: "));

        let json = parse_str(
            r#"{"name":"x","texture":"t.png","tile_size":16,"tiles":[{"id":0,"uv":[0,0,16,16]}]}"#,
        )
        .unwrap();
        assert_eq!(TileSet::from_json(&json).unwrap_err().path, "tiles[0].name");

        let json = parse_str(r#"{"name":"x","texture":"t.png","tiles":[]}"#).unwrap();
        let err = TileSet::from_json(&json).unwrap_err();
        assert_eq!(err.path, "tile_size");
        assert!(err.message.contains("missing required field"));

        let json = parse_str(r#"{"name":"x","texture":"t.png","tile_size":0,"tiles":[]}"#).unwrap();
        assert_eq!(TileSet::from_json(&json).unwrap_err().path, "tile_size");

        let json = parse_str(r#"{"name":"x","texture":"t.png","tile_size":16}"#).unwrap();
        assert_eq!(TileSet::from_json(&json).unwrap_err().path, "tiles");

        let json = parse_str(r#"{"name":"x","texture":"t.png","tile_size":16,"tiles":[{"id":"no","name":"a","uv":[0,0,1,1]}]}"#).unwrap();
        let err = TileSet::from_json(&json).unwrap_err();
        assert_eq!(err.path, "tiles[0].id");
        assert!(err.message.contains("non-negative integer"));

        let json = parse_str(r#"{"name":"x","texture":"t.png","tile_size":16,"tiles":[{"id":0,"name":"a","uv":[0,0,1,1],"flags":7}]}"#).unwrap();
        assert_eq!(
            TileSet::from_json(&json).unwrap_err().path,
            "tiles[0].flags"
        );

        let json =
            parse_str(r#"{"name":"x","texture":"t.png","tile_size":16,"tiles":[7]}"#).unwrap();
        assert_eq!(TileSet::from_json(&json).unwrap_err().path, "tiles[0]");

        assert_eq!(TileSet::from_json(&JsonValue::Null).unwrap_err().path, "");
    }

    const PREFAB: &str = r#"{
        "name": "house_small",
        "size": [4, 3, 5],
        "tags": ["residential", "roofed"],
        "voxels": [
            {"x": 0, "y": 0, "z": 0, "tile": 1},
            {"x": 1, "y": 0, "z": 0, "tile": 1},
            {"x": 1, "y": 1, "z": 0, "tile": 2}
        ],
        "props": [
            {"kind": "lamp", "position": [1.5, 0.0, 2.5]},
            {"kind": "sign", "position": [0.0, 1.0, 0.0], "yaw": 1.5, "scale": 2.0}
        ],
        "spawns": [
            {"name": "player_start", "position": [2.0, 0.0, 3.0], "yaw": 2.5},
            {"name": "patrol", "position": [1.0, 0.0, 1.0]}
        ],
        "occluders": [[0, 2, 0, 4, 3, 5]]
    }"#;

    #[test]
    fn prefab_round_trip() {
        let json = parse_str(PREFAB).unwrap();
        let prefab = Prefab::from_json(&json).unwrap();
        assert_eq!(prefab.name, "house_small");
        assert_eq!(prefab.size, [4, 3, 5]);
        assert_eq!(prefab.tags.len(), 2);
        assert!(prefab.has_tag("roofed"));
        assert!(!prefab.has_tag("industrial"));
        assert_eq!(prefab.voxels.len(), 3);
        assert_eq!(prefab.voxel(1, 0, 0), Some(1));
        assert_eq!(prefab.voxel(1, 1, 0), Some(2));
        assert_eq!(prefab.voxel(3, 3, 3), None);
        assert_eq!(prefab.footprint(), (4, 5));
        assert_eq!(prefab.volume(), 60);

        assert_eq!(prefab.props.len(), 2);
        assert_eq!(prefab.props[0].kind, "lamp");
        assert_eq!(prefab.props[0].scale, 1.0, "scale defaults to 1");
        assert_eq!(prefab.props[1].scale, 2.0);
        assert_eq!(prefab.props[1].yaw, 1.5);

        assert_eq!(prefab.spawns.len(), 2);
        assert_eq!(prefab.spawn("player_start").unwrap().yaw, 2.5);
        assert_eq!(prefab.spawn("patrol").unwrap().yaw, 0.0);
        assert!(prefab.spawn("nope").is_none());
        assert_eq!(prefab.occluders, vec![[0, 2, 0, 4, 3, 5]]);

        let bounds = prefab.bounds_world(Vec3::new(10.0, 0.0, 20.0), 2.0);
        assert_eq!(bounds.min, Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(bounds.max, Vec3::new(18.0, 6.0, 30.0));
        assert_eq!(bounds.size(), Vec3::new(8.0, 6.0, 10.0));

        let written = prefab.to_json();
        assert_eq!(Prefab::from_json(&written).unwrap(), prefab);
        assert_eq!(
            Prefab::from_json(&parse_str(&written.to_string()).unwrap()).unwrap(),
            prefab
        );
    }

    #[test]
    fn prefab_error_paths_are_precise() {
        let json = parse_str(r#"{"name":"a","size":[1,1]}"#).unwrap();
        assert_eq!(Prefab::from_json(&json).unwrap_err().path, "size");

        let json = parse_str(r#"{"name":"a","size":[1,300,1]}"#).unwrap();
        let err = Prefab::from_json(&json).unwrap_err();
        assert_eq!(err.path, "size");
        assert!(err.message.contains("0..=255"), "{}", err.message);

        let json = parse_str(r#"{"size":[1,1,1]}"#).unwrap();
        assert_eq!(Prefab::from_json(&json).unwrap_err().path, "name");

        let json =
            parse_str(r#"{"name":"a","size":[1,1,1],"voxels":[{"x":0,"y":0,"tile":1}]}"#).unwrap();
        assert_eq!(Prefab::from_json(&json).unwrap_err().path, "voxels[0].z");

        let json = parse_str(r#"{"name":"a","size":[1,1,1],"props":[{"kind":"lamp"}]}"#).unwrap();
        assert_eq!(
            Prefab::from_json(&json).unwrap_err().path,
            "props[0].position"
        );

        let json =
            parse_str(r#"{"name":"a","size":[1,1,1],"spawns":[{"position":[0,0,0]}]}"#).unwrap();
        assert_eq!(Prefab::from_json(&json).unwrap_err().path, "spawns[0].name");

        let json = parse_str(r#"{"name":"a","size":[1,1,1],"occluders":[[0,0,0,1,1]]}"#).unwrap();
        let err = Prefab::from_json(&json).unwrap_err();
        assert_eq!(err.path, "occluders[0]");
        assert!(err.message.contains("6 integers"), "{}", err.message);

        let json = parse_str(r#"{"name":"a","size":[1,1,1],"occluders":[5]}"#).unwrap();
        assert_eq!(Prefab::from_json(&json).unwrap_err().path, "occluders[0]");

        let json =
            parse_str(r#"{"name":"a","size":[1,1,1],"voxels":[{"x":0,"y":0,"z":0,"tile":1}]}"#)
                .unwrap();
        let prefab = Prefab::from_json(&json).unwrap();
        assert!(prefab.props.is_empty() && prefab.spawns.is_empty() && prefab.occluders.is_empty());
        assert!(prefab.tags.is_empty());
    }

    #[test]
    fn palette_file_parses_every_colour_form() {
        let json = parse_str(
            r##"{"name":"dawn","entries":[
                {"name":"grass","color":"#2E8B2E","description":"the lawn"},
                {"name":"sky","color":"#87CEEBFF"},
                {"name":"short","color":"#f00"},
                {"name":"alpha","color":"#11223344"},
                {"name":"packed","color":4278190335},
                {"name":"rgb","color":16711680}
            ]}"##,
        )
        .unwrap();
        let file = PaletteFile::from_json(&json).unwrap();
        assert_eq!(file.name, "dawn");
        assert_eq!(file.entries.len(), 6);
        assert_eq!(file.entries[0].color, Color8::rgb(0x2E, 0x8B, 0x2E));
        assert_eq!(file.entries[0].description.as_deref(), Some("the lawn"));
        assert_eq!(file.entries[1].color, Color8::rgb(0x87, 0xCE, 0xEB));
        assert_eq!(file.entries[2].color, Color8::rgb(0xFF, 0x00, 0x00));
        assert_eq!(file.entries[3].color, Color8::new(0x11, 0x22, 0x33, 0x44));
        assert_eq!(file.entries[4].color, Color8::from_hex(4278190335));
        assert_eq!(file.entries[5].color, Color8::rgb(0xFF, 0x00, 0x00));
        assert!(file.entries[1].description.is_none());
        assert_eq!(format_color(file.entries[3].color), "#11223344");

        let written = file.to_json();
        assert_eq!(PaletteFile::from_json(&written).unwrap(), file);
        assert!(written.to_string_pretty().lines().count() > 5);

        let palette = file.to_palette();
        assert_eq!(palette.len(), 6);
        assert_eq!(palette.find("grass"), Some(Color8::rgb(0x2E, 0x8B, 0x2E)));
        assert_eq!(
            palette.nearest(Color8::rgb(0x2F, 0x8A, 0x2F)).unwrap().name,
            "grass"
        );
    }

    #[test]
    fn palette_error_paths_are_precise() {
        let json = parse_str(r#"{"entries":[]}"#).unwrap();
        assert_eq!(PaletteFile::from_json(&json).unwrap_err().path, "name");

        let json = parse_str(r#"{"name":"p","entries":[{"name":"a","color":"nope"}]}"#).unwrap();
        let err = PaletteFile::from_json(&json).unwrap_err();
        assert_eq!(err.path, "entries[0].color");
        assert!(err.message.contains("neither #RGB"), "{}", err.message);

        let json = parse_str(r#"{"name":"p","entries":[{"name":"a"}]}"#).unwrap();
        assert_eq!(
            PaletteFile::from_json(&json).unwrap_err().path,
            "entries[0].color"
        );

        let json = parse_str(r##"{"name":"p","entries":[{"color":"#fff"}]}"##).unwrap();
        assert_eq!(
            PaletteFile::from_json(&json).unwrap_err().path,
            "entries[0].name"
        );

        let json = parse_str(r#"{"name":"p","entries":[3]}"#).unwrap();
        assert_eq!(
            PaletteFile::from_json(&json).unwrap_err().path,
            "entries[0]"
        );

        let json =
            parse_str(r##"{"name":"p","entries":[{"name":"a","color":"#12345"}]}"##).unwrap();
        assert!(
            PaletteFile::from_json(&json)
                .unwrap_err()
                .message
                .contains("neither")
        );

        let json = parse_str(r#"{"name":"p","entries":[{"name":"a","color":true}]}"#).unwrap();
        assert!(
            PaletteFile::from_json(&json)
                .unwrap_err()
                .message
                .contains("expected")
        );
    }

    #[test]
    fn sprite_file_round_trip_and_defaults() {
        let json = parse_str(
            r#"{"name":"hero","texture":"sprites/hero.png","uv":[0,0,16,24],
                "pivot":[0.5,1.0],
                "frames":[
                    {"name":"idle","uv":[0,0,16,24],"duration":0.25},
                    {"name":"walk","uv":[16,0,16,24]}
                ]}"#,
        )
        .unwrap();
        let sprite = SpriteFile::from_json(&json).unwrap();
        assert_eq!(sprite.name, "hero");
        assert_eq!(sprite.pivot, [0.5, 1.0]);
        assert_eq!(sprite.size, [16.0, 24.0], "size defaults to the frame size");
        assert_eq!(sprite.frames.len(), 2);
        assert_eq!(sprite.frames[0].duration, 0.25);
        assert_eq!(sprite.frames[1].duration, 0.1);
        assert_eq!(sprite.duration(), 0.35);
        assert_eq!(sprite.frame("walk").unwrap().uv, [16, 0, 16, 24]);
        assert!(sprite.frame("jump").is_none());

        let written = sprite.to_json();
        assert_eq!(SpriteFile::from_json(&written).unwrap(), sprite);

        // Minimal sprite: pivot and size default, no frames.
        let minimal = SpriteFile::from_json(
            &parse_str(r#"{"name":"coin","texture":"c.png","uv":[0,0,8,8]}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(minimal.pivot, [0.5, 0.5]);
        assert_eq!(minimal.size, [8.0, 8.0]);
        assert!(minimal.frames.is_empty());
        assert_eq!(minimal.duration(), 0.0);

        let err = SpriteFile::from_json(
            &parse_str(r#"{"name":"c","texture":"c.png","uv":[0,0,8,8],"frames":[{"name":"a"}]}"#)
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.path, "frames[0].uv");
        let err = SpriteFile::from_json(
            &parse_str(r#"{"name":"c","texture":"c.png","uv":[0,0],"pivot":[0.5,1.0]}"#).unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.path, "uv");
    }

    #[test]
    fn manifest_round_trip() {
        let json = parse_str(
            r#"{"name":"town-demo","version":"0.1.0",
                "tile_sets":["tiles/town.json"],
                "prefabs":["prefabs/house.json","prefabs/well.json"],
                "palettes":["config/palette.json"],
                "sprites":["sprites/hero.json"],
                "extra":"ignored"}"#,
        )
        .unwrap();
        let manifest = AssetManifest::from_json(&json).unwrap();
        assert_eq!(manifest.name, "town-demo");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.tile_sets, ["tiles/town.json"]);
        assert_eq!(manifest.prefabs.len(), 2);
        assert_eq!(manifest.len(), 5);
        assert!(!manifest.is_empty());

        let written = manifest.to_json();
        assert_eq!(AssetManifest::from_json(&written).unwrap(), manifest);
        assert_eq!(
            written.to_string(),
            parse_str(&written.to_string()).unwrap().to_string()
        );

        // A manifest with no lists is valid and empty.
        let sparse =
            AssetManifest::from_json(&parse_str(r#"{"name":"a","version":"1"}"#).unwrap()).unwrap();
        assert!(sparse.is_empty());
        assert!(sparse.tile_sets.is_empty());
        assert_eq!(
            AssetManifest::default(),
            AssetManifest::from_json(&AssetManifest::default().to_json()).unwrap()
        );

        let err = AssetManifest::from_json(&parse_str(r#"{"name":"a"}"#).unwrap()).unwrap_err();
        assert_eq!(err.path, "version");

        let err = AssetManifest::from_json(
            &parse_str(r#"{"name":"a","version":"1","prefabs":"nope"}"#).unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.path, "prefabs");
        assert!(err.message.contains("array of strings"), "{}", err.message);
    }

    #[test]
    fn tile_flags_defaults_and_json() {
        let flags = TileFlags::default();
        assert!(flags.walkable);
        assert!(
            !flags.blocks_sight
                && !flags.occluder
                && !flags.water
                && !flags.road
                && !flags.buildable
        );
        assert!(!flags.is_empty());

        let json = flags.to_json();
        assert!(json.get_bool("walkable", false));
        assert_eq!(TileFlags::from_json(&json, "flags").unwrap(), flags);

        let partial = parse_str(r#"{"water":true,"walkable":false}"#).unwrap();
        let parsed = TileFlags::from_json(&partial, "flags").unwrap();
        assert!(parsed.water && !parsed.walkable && !parsed.occluder);

        assert!(TileFlags::from_json(&JsonValue::from(3), "flags").is_err());
        let none = TileFlags::from_json(&JsonValue::Null, "flags").unwrap();
        assert_eq!(none, TileFlags::default());
        assert!(TileFlags::from_json(&JsonValue::Null, "tiles[0].flags").is_ok());
    }

    #[test]
    fn colour_parsing_edge_cases() {
        assert_eq!(
            parse_color(&JsonValue::from("#FFFFFF"), "c").unwrap(),
            Color8::WHITE
        );
        assert_eq!(
            parse_color(&JsonValue::from("#ffffffff"), "c").unwrap(),
            Color8::WHITE
        );
        assert_eq!(
            parse_color(&JsonValue::from("0x000000"), "c").unwrap(),
            Color8::BLACK
        );
        assert_eq!(
            parse_color(&JsonValue::from(" #000000 "), "c").unwrap(),
            Color8::BLACK
        );
        assert_eq!(
            parse_color(&JsonValue::from(0xFF00_0000u32), "c").unwrap(),
            Color8::new(0, 0, 0, 255)
        );
        assert!(parse_color(&JsonValue::from("#1234567890"), "c").is_err());
        assert!(parse_color(&JsonValue::from("#GGGGGG"), "c").is_err());
        assert!(parse_color(&JsonValue::Null, "c").is_err());
        assert!(parse_color(&JsonValue::from(-1i32), "c").is_err());
    }
}
