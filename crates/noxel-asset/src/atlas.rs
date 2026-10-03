//! Sprite-sheet packing with a deterministic shelf/skyline packer.
//!
//! [`Atlas::build`] takes loose images and produces one texture plus a named
//! [`SpriteRegion`] per image. The packer is a shelf packer: entries are sorted
//! by descending height, then by name, and each one is placed on the first shelf
//! with room for it. The atlas grows downwards and never exceeds `max_size`.
//!
//! Sorting by height first is what makes the result deterministic — the same
//! input set always produces byte-identical UVs, whichever order the caller
//! happened to collect the images in.
//!
//! ## Padding
//!
//! Every sprite is inset by `padding` pixels inside a cell that is
//! `2 * padding` larger than the sprite on both axes. Two neighbouring sprites
//! therefore have at least `2 * padding` transparent pixels between them, so a
//! bilinear filter (or a slightly sloppy UV) cannot bleed one sprite into the
//! next.
//!
//! ## Example
//!
//! ```
//! use noxel_asset::atlas::Atlas;
//! use noxel_asset::image::Image;
//! use noxel_core::math::{Color8, Vec2};
//!
//! let entries = vec![
//!     ("hero".to_string(), Image::new(16, 24, Color8::RED)),
//!     ("coin".to_string(), Image::new(8, 8, Color8::rgb(255, 220, 0))),
//! ];
//! let atlas = Atlas::build(entries, 1, 256).unwrap();
//! assert_eq!(atlas.regions().len(), 2);
//! assert_eq!(atlas.region("coin").unwrap().pivot, Vec2::new(0.5, 0.5));
//! ```

use std::fmt;

use noxel_core::math::{Color8, Rect, Vec2};

use crate::image::Image;
use crate::json::JsonValue;
use crate::texture::Texture;

/// The default pivot for a packed sprite: its centre.
pub const DEFAULT_PIVOT: Vec2 = Vec2::new(0.5, 0.5);

/// A named rectangle inside an [`Atlas`] texture.
///
/// `uv_min`/`uv_max` are the normalised bounds used by the renderer;
/// `pixel_rect` is the same region in pixels, which tools and the debug overlay
/// use. `pivot` is normalised to the region, so `(0.5, 0.5)` is the sprite's
/// centre and `(0.5, 1.0)` its bottom-centre (the usual anchor for a top-down
/// RPG character standing on a tile).
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteRegion {
    /// Unique (within the atlas) name.
    pub name: String,
    /// Top-left UV corner.
    pub uv_min: Vec2,
    /// Bottom-right UV corner.
    pub uv_max: Vec2,
    /// The same region in texture pixels.
    pub pixel_rect: Rect,
    /// Normalised anchor point inside the region.
    pub pivot: Vec2,
}

impl SpriteRegion {
    /// Builds a region from a pixel rectangle, deriving the UVs from the
    /// texture size.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        pixel_rect: Rect,
        pivot: Vec2,
        texture_width: u32,
        texture_height: u32,
    ) -> Self {
        let w = texture_width.max(1) as f32;
        let h = texture_height.max(1) as f32;
        Self {
            name: name.into(),
            uv_min: Vec2::new(pixel_rect.min.x / w, pixel_rect.min.y / h),
            uv_max: Vec2::new(pixel_rect.max.x / w, pixel_rect.max.y / h),
            pixel_rect,
            pivot,
        }
    }

    /// Width of the region in pixels.
    #[must_use]
    pub fn width(&self) -> f32 {
        self.pixel_rect.width()
    }

    /// Height of the region in pixels.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.pixel_rect.height()
    }

    /// The region in texture pixels as `[x, y, w, h]`.
    #[must_use]
    pub fn rect_array(&self) -> [u32; 4] {
        let rect = self.pixel_rect;
        [
            rect.min.x.max(0.0) as u32,
            rect.min.y.max(0.0) as u32,
            rect.width().max(0.0) as u32,
            rect.height().max(0.0) as u32,
        ]
    }

    /// The world-space offset of the pivot inside a sprite of `size` pixels.
    #[must_use]
    pub fn pivot_offset(&self, size: Vec2) -> Vec2 {
        Vec2::new(self.pivot.x * size.x, self.pivot.y * size.y)
    }
}

/// Why an atlas could not be built or parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtlasError {
    /// One or more sprites did not fit within `max_size`. They are listed in
    /// packing order (height descending, then name).
    DoesNotFit {
        /// The names of the sprites that could not be placed.
        sprites: Vec<String>,
    },
    /// The JSON description of an atlas was malformed.
    InvalidData(String),
}

impl fmt::Display for AtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DoesNotFit { sprites } => {
                write!(
                    f,
                    "{} sprite(s) do not fit in the atlas: {}",
                    sprites.len(),
                    sprites.join(", ")
                )
            }
            Self::InvalidData(what) => write!(f, "invalid atlas data: {what}"),
        }
    }
}

impl std::error::Error for AtlasError {}

/// One packed texture plus its named regions.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    texture: Texture,
    regions: Vec<SpriteRegion>,
    width: u32,
    height: u32,
}

impl Atlas {
    /// Packs `entries` into a single texture.
    ///
    /// `padding` is applied around every sprite; `max_size` caps both atlas
    /// dimensions. Images with a zero width or height are skipped and get no
    /// region (there is nothing to pack). If any sprite cannot be placed, the
    /// whole build fails with [`AtlasError::DoesNotFit`] rather than returning a
    /// half-packed atlas.
    pub fn build(
        entries: Vec<(String, Image)>,
        padding: u32,
        max_size: u32,
    ) -> Result<Self, AtlasError> {
        // Deterministic order: tallest first, ties broken by name.
        let mut sorted = entries;
        sorted.sort_by(|a, b| b.1.height.cmp(&a.1.height).then_with(|| a.0.cmp(&b.0)));

        let mut shelves: Vec<Shelf> = Vec::new();
        let mut placed: Vec<(String, u32, u32, u32, u32)> = Vec::new();
        let mut failed: Vec<String> = Vec::new();

        for (name, image) in &sorted {
            if image.is_empty() {
                continue;
            }
            let cell_width = image.width + 2 * padding;
            let cell_height = image.height + 2 * padding;
            if cell_width > max_size || cell_height > max_size {
                failed.push(name.clone());
                continue;
            }
            let spot = shelves.iter_mut().find_map(|shelf| {
                if shelf.height >= cell_height && shelf.x + cell_width <= max_size {
                    let spot = (shelf.x, shelf.y);
                    shelf.x += cell_width;
                    Some(spot)
                } else {
                    None
                }
            });
            let (cell_x, cell_y) = match spot {
                Some(spot) => spot,
                None => {
                    let y = shelves.last().map_or(0, |shelf| shelf.y + shelf.height);
                    if y + cell_height > max_size {
                        failed.push(name.clone());
                        continue;
                    }
                    shelves.push(Shelf {
                        y,
                        height: cell_height,
                        x: cell_width,
                    });
                    (0, y)
                }
            };
            placed.push((
                name.clone(),
                cell_x + padding,
                cell_y + padding,
                image.width,
                image.height,
            ));
        }

        if !failed.is_empty() {
            return Err(AtlasError::DoesNotFit { sprites: failed });
        }

        let used_width = shelves
            .iter()
            .map(|shelf| shelf.x)
            .max()
            .unwrap_or(1)
            .max(1);
        let used_height = shelves
            .last()
            .map_or(1, |shelf| shelf.y + shelf.height)
            .max(1);
        let width = used_width.min(max_size.max(1));
        let height = used_height.min(max_size.max(1));

        let mut sheet = Image::new(width, height, Color8::TRANSPARENT);
        let mut regions = Vec::with_capacity(placed.len());
        for (name, x, y, w, h) in placed {
            let source = sorted
                .iter()
                .find(|(entry_name, _)| *entry_name == name)
                .map(|(_, image)| image)
                .expect("placed sprites came from the sorted list");
            sheet.stamp(source, x as i32, y as i32);
            regions.push(SpriteRegion::new(
                name,
                Rect::from_min_max(
                    Vec2::new(x as f32, y as f32),
                    Vec2::new((x + w) as f32, (y + h) as f32),
                ),
                DEFAULT_PIVOT,
                width,
                height,
            ));
        }

        Ok(Self {
            texture: Texture::from_image(sheet),
            regions,
            width,
            height,
        })
    }

    /// Builds an atlas around an existing texture and a set of regions.
    ///
    /// This is the constructor [`Atlas::from_json`] uses. The regions are taken
    /// exactly as given — nothing is re-packed and no overlap is checked — so a
    /// region whose pixel rect no longer matches the texture keeps the UVs it
    /// was authored with.
    #[must_use]
    pub fn from_regions(texture: Texture, regions: Vec<SpriteRegion>) -> Self {
        let width = texture.width();
        let height = texture.height();
        Self {
            texture,
            regions,
            width,
            height,
        }
    }

    /// The packed texture.
    #[must_use]
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    /// Looks a region up by name.
    #[must_use]
    pub fn region(&self, name: &str) -> Option<&SpriteRegion> {
        self.regions.iter().find(|region| region.name == name)
    }

    /// True when a region with this name exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.region(name).is_some()
    }

    /// Every region, in packing order.
    #[must_use]
    pub fn regions(&self) -> &[SpriteRegion] {
        &self.regions
    }

    /// Number of regions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// True when the atlas has no regions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Atlas width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Atlas height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Fraction of the atlas area covered by sprite pixels, in `[0, 1]`.
    ///
    /// Padding and shelf slack both count as wasted space, which is exactly what
    /// you want to see when tuning `padding` and `max_size`.
    #[must_use]
    pub fn occupancy(&self) -> f32 {
        let total = (self.width as f32) * (self.height as f32);
        if total <= 0.0 {
            return 0.0;
        }
        let used: f32 = self.regions.iter().map(|r| r.pixel_rect.area()).sum();
        (used / total).clamp(0.0, 1.0)
    }

    /// Serialises the atlas layout (not the pixels) to JSON.
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let regions = self
            .regions
            .iter()
            .map(|region| {
                let rect = region.rect_array();
                JsonValue::object([
                    ("name", JsonValue::from(region.name.as_str())),
                    (
                        "uv_min",
                        JsonValue::array([
                            JsonValue::from(region.uv_min.x),
                            JsonValue::from(region.uv_min.y),
                        ]),
                    ),
                    (
                        "uv_max",
                        JsonValue::array([
                            JsonValue::from(region.uv_max.x),
                            JsonValue::from(region.uv_max.y),
                        ]),
                    ),
                    (
                        "rect",
                        JsonValue::array([
                            JsonValue::from(rect[0]),
                            JsonValue::from(rect[1]),
                            JsonValue::from(rect[2]),
                            JsonValue::from(rect[3]),
                        ]),
                    ),
                    (
                        "pivot",
                        JsonValue::array([
                            JsonValue::from(region.pivot.x),
                            JsonValue::from(region.pivot.y),
                        ]),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        JsonValue::object([
            ("width", JsonValue::from(self.width)),
            ("height", JsonValue::from(self.height)),
            ("regions", JsonValue::Array(regions)),
        ])
    }

    /// Rebuilds an atlas from [`Atlas::to_json`] output and its texture.
    ///
    /// A region needs a `name` and a `rect` of four integers; `uv_min`,
    /// `uv_max` and `pivot` are optional and are recomputed or defaulted when
    /// absent, so hand-written atlas files stay terse.
    pub fn from_json(texture: Texture, json: &JsonValue) -> Result<Self, AtlasError> {
        let regions_json = json.get_array("regions").ok_or_else(|| {
            AtlasError::InvalidData("missing required field `regions`".to_string())
        })?;
        let mut regions = Vec::with_capacity(regions_json.len());
        for (i, entry) in regions_json.iter().enumerate() {
            let path = format!("regions[{i}]");
            let name = entry
                .get_str("name")
                .ok_or_else(|| {
                    AtlasError::InvalidData(format!("{path}.name is missing or not a string"))
                })?
                .to_string();
            let rect_values = entry.get_array("rect").ok_or_else(|| {
                AtlasError::InvalidData(format!("{path}.rect is missing or not an array"))
            })?;
            if rect_values.len() != 4 {
                return Err(AtlasError::InvalidData(format!(
                    "{path}.rect must have 4 numbers, found {}",
                    rect_values.len()
                )));
            }
            let mut rect = [0.0f32; 4];
            for (slot, value) in rect.iter_mut().zip(rect_values) {
                *slot = value.as_f32().ok_or_else(|| {
                    AtlasError::InvalidData(format!("{path}.rect contains a non-number"))
                })?;
            }
            let pivot_values = entry.get_array("pivot");
            let pivot = match pivot_values {
                Some(values) if values.len() == 2 => Vec2::new(
                    values[0].as_f32().unwrap_or(DEFAULT_PIVOT.x),
                    values[1].as_f32().unwrap_or(DEFAULT_PIVOT.y),
                ),
                None => DEFAULT_PIVOT,
                Some(values) => {
                    return Err(AtlasError::InvalidData(format!(
                        "{path}.pivot must have 2 numbers, found {}",
                        values.len()
                    )));
                }
            };
            let pixel_rect = Rect::from_min_max(
                Vec2::new(rect[0], rect[1]),
                Vec2::new(rect[0] + rect[2], rect[1] + rect[3]),
            );
            let mut region =
                SpriteRegion::new(name, pixel_rect, pivot, texture.width(), texture.height());
            region.uv_min = read_uv(entry, "uv_min").unwrap_or(region.uv_min);
            region.uv_max = read_uv(entry, "uv_max").unwrap_or(region.uv_max);
            regions.push(region);
        }
        Ok(Self::from_regions(texture, regions))
    }
}

fn read_uv(entry: &JsonValue, key: &str) -> Option<Vec2> {
    let values = entry.get_array(key)?;
    if values.len() != 2 {
        return None;
    }
    Some(Vec2::new(values[0].as_f32()?, values[1].as_f32()?))
}

/// One horizontal row of the shelf packer.
struct Shelf {
    y: u32,
    height: u32,
    /// The next free x coordinate on this shelf.
    x: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Color8;

    fn sprite(width: u32, height: u32, color: Color8) -> Image {
        let mut image = Image::new(width, height, color);
        // A corner marker makes it obvious if a sprite is copied from the wrong
        // place in the sheet.
        image.set(0, 0, Color8::MAGENTA);
        image
    }

    fn entries() -> Vec<(String, Image)> {
        vec![
            ("a".to_string(), sprite(16, 16, Color8::RED)),
            ("b".to_string(), sprite(8, 8, Color8::GREEN)),
            ("c".to_string(), sprite(32, 12, Color8::BLUE)),
            ("d".to_string(), sprite(4, 20, Color8::WHITE)),
        ]
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
    }

    #[test]
    fn packs_every_sprite_within_bounds() {
        let atlas = Atlas::build(entries(), 1, 256).unwrap();
        assert_eq!(atlas.len(), 4);
        assert_eq!(atlas.width(), atlas.texture().width());
        assert_eq!(atlas.height(), atlas.texture().height());
        assert!(atlas.width() <= 256 && atlas.height() <= 256);
        for name in ["a", "b", "c", "d"] {
            assert!(atlas.contains(name), "{name} is missing");
        }
        assert!(!atlas.contains("nope"));
        assert!(!atlas.is_empty());
        assert!(atlas.region("nope").is_none());

        // Every region is inside the texture.
        for region in atlas.regions() {
            assert!(region.pixel_rect.min.x >= 0.0 && region.pixel_rect.min.y >= 0.0);
            assert!(region.pixel_rect.max.x <= atlas.width() as f32);
            assert!(region.pixel_rect.max.y <= atlas.height() as f32);
            assert!(region.uv_min.x >= 0.0 && region.uv_max.x <= 1.0);
            assert!(region.uv_min.y >= 0.0 && region.uv_max.y <= 1.0);
        }
    }

    #[test]
    fn regions_never_overlap_and_keep_their_padding() {
        let padding = 2;
        let atlas = Atlas::build(entries(), padding, 256).unwrap();
        let regions = atlas.regions();
        for (i, a) in regions.iter().enumerate() {
            for b in regions.iter().skip(i + 1) {
                assert!(
                    !overlaps(a.pixel_rect, b.pixel_rect),
                    "{:?} overlaps {:?}",
                    a.name,
                    b.name
                );
                let gap = padding as f32;
                let grown_a = a.pixel_rect.expanded(Vec2::new(gap, gap));
                let grown_b = b.pixel_rect.expanded(Vec2::new(gap, gap));
                assert!(
                    !overlaps(grown_a, grown_b),
                    "{} and {} are closer than the padding",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn packed_pixels_come_from_the_right_sprite() {
        let atlas = Atlas::build(entries(), 1, 256).unwrap();
        let region = atlas.region("c").unwrap();
        let texture = atlas.texture();
        let x = region.pixel_rect.min.x as u32;
        let y = region.pixel_rect.min.y as u32;
        assert_eq!(
            texture.image().get(x, y),
            Some(Color8::MAGENTA),
            "corner marker"
        );
        assert_eq!(texture.image().get(x + 1, y + 1), Some(Color8::BLUE));
        assert_eq!(texture.image().get(x + 31, y + 11), Some(Color8::BLUE));
        // The padding ring around the sprite is untouched.
        assert_eq!(texture.image().get(x - 1, y), Some(Color8::TRANSPARENT));
        assert_eq!(texture.image().get(x, y - 1), Some(Color8::TRANSPARENT));
    }

    #[test]
    fn packing_is_deterministic_and_order_independent() {
        let first = Atlas::build(entries(), 1, 256).unwrap();
        let mut shuffled = entries();
        shuffled.reverse();
        let second = Atlas::build(shuffled, 1, 256).unwrap();
        assert_eq!(first.regions(), second.regions());
        assert_eq!(first.width(), second.width());
        assert_eq!(first.height(), second.height());

        // Tallest first, ties by name.
        let heights: Vec<u32> = first.regions().iter().map(|r| r.height() as u32).collect();
        assert_eq!(heights, vec![20, 16, 12, 8]);
    }

    #[test]
    fn too_large_sprites_are_reported_by_name() {
        let entries = vec![
            ("small".to_string(), sprite(8, 8, Color8::RED)),
            ("huge".to_string(), sprite(300, 4, Color8::BLUE)),
            ("tall".to_string(), sprite(4, 300, Color8::GREEN)),
        ];
        let err = Atlas::build(entries, 1, 64).unwrap_err();
        match &err {
            AtlasError::DoesNotFit { sprites } => {
                assert_eq!(sprites, &vec!["tall".to_string(), "huge".to_string()]);
            }
            other => panic!("expected DoesNotFit, got {other:?}"),
        }
        assert!(err.to_string().contains("huge"));
        assert!(err.to_string().contains("tall"));

        // The same set fits once the limit is raised.
        let entries = vec![
            ("small".to_string(), sprite(8, 8, Color8::RED)),
            ("huge".to_string(), sprite(300, 4, Color8::BLUE)),
        ];
        assert!(Atlas::build(entries, 1, 512).is_ok());
    }

    #[test]
    fn padding_is_the_difference_between_fitting_and_not() {
        let big = vec![
            ("a".to_string(), sprite(40, 40, Color8::RED)),
            ("b".to_string(), sprite(40, 40, Color8::GREEN)),
        ];
        assert!(Atlas::build(big.clone(), 0, 80).is_ok());
        let err = Atlas::build(big, 2, 80).unwrap_err();
        assert!(matches!(err, AtlasError::DoesNotFit { .. }), "{err}");
    }

    #[test]
    fn occupancy_counts_wasted_space() {
        let atlas =
            Atlas::build(vec![("a".to_string(), sprite(50, 50, Color8::RED))], 0, 64).unwrap();
        assert_eq!((atlas.width(), atlas.height()), (50, 50));
        assert!((atlas.occupancy() - 1.0).abs() < 1e-6);

        let padded =
            Atlas::build(vec![("a".to_string(), sprite(50, 50, Color8::RED))], 5, 64).unwrap();
        assert_eq!((padded.width(), padded.height()), (60, 60));
        assert!((padded.occupancy() - (2500.0 / 3600.0)).abs() < 1e-4);

        let empty = Atlas::build(Vec::new(), 1, 64).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.occupancy(), 0.0);
        assert_eq!((empty.width(), empty.height()), (1, 1));
    }

    #[test]
    fn zero_sized_images_are_skipped() {
        let entries = vec![
            ("empty".to_string(), Image::new(0, 0, Color8::RED)),
            ("real".to_string(), sprite(4, 4, Color8::BLUE)),
        ];
        let atlas = Atlas::build(entries, 0, 64).unwrap();
        assert_eq!(atlas.len(), 1);
        assert!(atlas.region("real").is_some());
        assert!(atlas.region("empty").is_none());
    }

    #[test]
    fn json_round_trip_preserves_uvs() {
        let atlas = Atlas::build(entries(), 1, 256).unwrap();
        let json = atlas.to_json();
        assert_eq!(json.get_u32("width", 0), atlas.width());
        assert_eq!(json.get_array("regions").unwrap().len(), 4);
        assert_eq!(
            json.get_array("regions").unwrap()[0].get_str("name"),
            Some("d")
        );

        let parsed = Atlas::from_json(atlas.texture().clone(), &json).unwrap();
        assert_eq!(parsed.regions(), atlas.regions());
        assert_eq!(parsed.width(), atlas.width());
        assert_eq!(parsed.height(), atlas.height());
        assert_eq!(parsed.occupancy(), atlas.occupancy());

        // The JSON survives a textual round trip too.
        let text = json.to_string_pretty();
        let reparsed = crate::json::parse_str(&text).unwrap();
        let again = Atlas::from_json(atlas.texture().clone(), &reparsed).unwrap();
        assert_eq!(again.regions(), atlas.regions());
    }

    #[test]
    fn from_json_rejects_malformed_input() {
        let texture = Texture::from_image(Image::new(16, 16, Color8::RED));
        assert!(Atlas::from_json(texture.clone(), &JsonValue::Null).is_err());
        assert!(
            Atlas::from_json(
                texture.clone(),
                &JsonValue::object([("regions", JsonValue::array([]))])
            )
            .is_ok()
        );

        let missing_name = JsonValue::object([(
            "regions",
            JsonValue::array([JsonValue::object([(
                "rect",
                JsonValue::array([
                    JsonValue::from(0u32),
                    JsonValue::from(0u32),
                    JsonValue::from(8u32),
                    JsonValue::from(8u32),
                ]),
            )])]),
        )]);
        let err = Atlas::from_json(texture.clone(), &missing_name).unwrap_err();
        assert!(err.to_string().contains("regions[0].name"), "{err}");

        let bad_rect = JsonValue::object([(
            "regions",
            JsonValue::array([JsonValue::object([
                ("name", JsonValue::from("a")),
                (
                    "rect",
                    JsonValue::array([
                        JsonValue::from(0u32),
                        JsonValue::from(0u32),
                        JsonValue::from(8u32),
                    ]),
                ),
            ])]),
        )]);
        let err = Atlas::from_json(texture.clone(), &bad_rect).unwrap_err();
        assert!(err.to_string().contains("4 numbers"), "{err}");

        let bad_pivot = JsonValue::object([(
            "regions",
            JsonValue::array([JsonValue::object([
                ("name", JsonValue::from("a")),
                (
                    "rect",
                    JsonValue::array([
                        JsonValue::from(0u32),
                        JsonValue::from(0u32),
                        JsonValue::from(8u32),
                        JsonValue::from(8u32),
                    ]),
                ),
                (
                    "pivot",
                    JsonValue::array([
                        JsonValue::from(0u32),
                        JsonValue::from(0u32),
                        JsonValue::from(0u32),
                    ]),
                ),
            ])]),
        )]);
        assert!(Atlas::from_json(texture.clone(), &bad_pivot).is_err());

        let non_number = JsonValue::object([(
            "regions",
            JsonValue::array([JsonValue::object([
                ("name", JsonValue::from("a")),
                (
                    "rect",
                    JsonValue::array([
                        JsonValue::from("x"),
                        JsonValue::from(0u32),
                        JsonValue::from(8u32),
                        JsonValue::from(8u32),
                    ]),
                ),
            ])]),
        )]);
        assert!(Atlas::from_json(texture, &non_number).is_err());
    }

    #[test]
    fn regions_derive_uvs_and_pivots() {
        let texture = Texture::from_image(Image::new(64, 32, Color8::RED));
        let atlas = Atlas::from_regions(
            texture,
            vec![SpriteRegion::new(
                "hero",
                Rect::from_min_max(Vec2::new(16.0, 8.0), Vec2::new(32.0, 24.0)),
                Vec2::new(0.5, 1.0),
                64,
                32,
            )],
        );
        let region = atlas.region("hero").unwrap();
        assert_eq!(region.uv_min, Vec2::new(0.25, 0.25));
        assert_eq!(region.uv_max, Vec2::new(0.5, 0.75));
        assert_eq!(region.width(), 16.0);
        assert_eq!(region.height(), 16.0);
        assert_eq!(region.rect_array(), [16, 8, 16, 16]);
        assert_eq!(
            region.pivot_offset(Vec2::new(16.0, 16.0)),
            Vec2::new(8.0, 16.0)
        );
        assert_eq!(atlas.occupancy(), 0.125);
    }
}
