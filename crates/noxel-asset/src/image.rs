//! The in-memory pixel buffer and the sprite-authoring operations on it.
//!
//! [`Image`] is deliberately plain: a `width`, a `height` and a row-major
//! `Vec<Color8>`. There is no `unsafe`, no SIMD, no hidden stride — the same
//! buffer can be handed straight to the renderer, written to a PNG, or packed
//! into an atlas.
//!
//! ## Bounds behaviour
//!
//! Nothing in this module panics on an out-of-bounds coordinate:
//!
//! | Method | Out of bounds behaviour |
//! |---|---|
//! | [`Image::get`] | returns `None` |
//! | [`Image::get_or_transparent`] | returns [`Color8::TRANSPARENT`] |
//! | [`Image::set`] | silently ignored |
//! | [`Image::fill_rect`] / [`Image::blit`] | clipped to the image |
//!
//! Indexing is always `y * width + x`; [`Image::index_of`] is the only place
//! that arithmetic lives.

use std::io;
use std::path::Path;

use noxel_core::math::{Color8, Palette, Rect, Vec2};

use crate::png::{self, PngError};

/// Conventional 16×16 pixel-art tile size.
pub const TILE_16: u32 = 16;
/// Conventional 32×32 pixel-art tile size.
pub const TILE_32: u32 = 32;
/// Conventional 64×64 pixel-art tile size.
pub const TILE_64: u32 = 64;

/// A row-major RGBA image.
///
/// `pixels.len()` is always `width * height`; the fields are public so the
/// renderer can borrow the buffer directly, but every constructor here keeps
/// that invariant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major pixels, `y * width + x`.
    pub pixels: Vec<Color8>,
}

impl Image {
    /// A 16×16 tile size constant, for call sites that prefer it namespaced.
    pub const TILE_16: u32 = TILE_16;
    /// A 32×32 tile size constant, for call sites that prefer it namespaced.
    pub const TILE_32: u32 = TILE_32;
    /// A 64×64 tile size constant, for call sites that prefer it namespaced.
    pub const TILE_64: u32 = TILE_64;

    /// A `width` × `height` image filled with `fill`.
    ///
    /// A zero width or height produces an empty image rather than an error.
    #[must_use]
    pub fn new(width: u32, height: u32, fill: Color8) -> Self {
        if width == 0 || height == 0 {
            return Self {
                width,
                height,
                pixels: Vec::new(),
            };
        }
        let len = (width as usize) * (height as usize);
        Self {
            width,
            height,
            pixels: vec![fill; len],
        }
    }

    /// A fully transparent `width` × `height` image.
    #[must_use]
    pub fn transparent(width: u32, height: u32) -> Self {
        Self::new(width, height, Color8::TRANSPARENT)
    }

    /// Wraps an existing pixel buffer, checking that its length matches.
    ///
    /// Returns `None` when `pixels.len() != width * height`.
    #[must_use]
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<Color8>) -> Option<Self> {
        if pixels.len() != (width as usize) * (height as usize) {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Builds an image from a closure called with each `(x, y)`.
    #[must_use]
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> Color8) -> Self {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        for y in 0..height {
            for x in 0..width {
                pixels.push(f(x, y));
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Number of pixels (`width * height`).
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// True when the image has no pixels.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The whole image as a rectangle in pixels: `(0, 0)` to `(width, height)`.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect::from_min_max(Vec2::ZERO, Vec2::new(self.width as f32, self.height as f32))
    }

    /// Row-major index of `(x, y)`, or `None` when out of bounds.
    #[must_use]
    pub fn index_of(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize) * (self.width as usize) + (x as usize))
    }

    /// The pixel at `(x, y)`, or `None` when out of bounds.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<Color8> {
        self.index_of(x, y).map(|i| self.pixels[i])
    }

    /// The pixel at `(x, y)`, or [`Color8::TRANSPARENT`] when out of bounds.
    #[must_use]
    pub fn get_or_transparent(&self, x: u32, y: u32) -> Color8 {
        self.get(x, y).unwrap_or(Color8::TRANSPARENT)
    }

    /// Signed-coordinate lookup for blitting and outlining.
    #[must_use]
    pub fn get_at(&self, x: i64, y: i64) -> Option<Color8> {
        if x < 0 || y < 0 {
            return None;
        }
        self.get(x as u32, y as u32)
    }

    /// Writes a pixel. Out-of-bounds coordinates are **ignored**, never a panic.
    pub fn set(&mut self, x: u32, y: u32, color: Color8) {
        if let Some(i) = self.index_of(x, y) {
            self.pixels[i] = color;
        }
    }

    /// Writes a pixel, returning `false` when the coordinate was out of bounds.
    pub fn try_set(&mut self, x: u32, y: u32, color: Color8) -> bool {
        match self.index_of(x, y) {
            Some(i) => {
                self.pixels[i] = color;
                true
            }
            None => false,
        }
    }

    /// Replaces every pixel with `color`.
    pub fn clear(&mut self, color: Color8) {
        self.pixels.fill(color);
    }

    /// Fills the axis-aligned `rect`, clipped to the image.
    ///
    /// The rectangle is converted to whole pixels with `floor` on the minimum
    /// edge and `ceil` on the maximum edge, so a fractional rect never leaves a
    /// half-covered pixel untouched.
    pub fn fill_rect(&mut self, rect: Rect, color: Color8) {
        let Some((x0, y0, x1, y1)) = self.clip_i32(rect) else {
            return;
        };
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x as u32, y as u32, color);
            }
        }
    }

    /// Draws `src` with its top-left corner at `(x, y)`, compositing each pixel
    /// over the destination with [`Color8::blend_over`].
    ///
    /// Negative offsets and overhang are clipped. Fully transparent source
    /// pixels are skipped, so a sprite blit never erases what is beneath it.
    pub fn blit(&mut self, src: &Image, x: i32, y: i32) {
        if src.is_empty() || self.is_empty() {
            return;
        }
        for sy in 0..src.height as i64 {
            let dy = i64::from(y) + sy;
            if dy < 0 || dy >= i64::from(self.height) {
                continue;
            }
            for sx in 0..src.width as i64 {
                let dx = i64::from(x) + sx;
                if dx < 0 || dx >= i64::from(self.width) {
                    continue;
                }
                let pixel = src.pixels[(sy as usize) * (src.width as usize) + sx as usize];
                if pixel.is_transparent() {
                    continue;
                }
                let dst = self.pixels[(dy as usize) * (self.width as usize) + dx as usize];
                self.pixels[(dy as usize) * (self.width as usize) + dx as usize] =
                    pixel.blend_over(dst);
            }
        }
    }

    /// Draws the `src_rect` sub-rectangle of `src` at `(x, y)`.
    ///
    /// The region's top-left corner lands on `(x, y)`; both the region and the
    /// destination are clipped.
    pub fn blit_region(&mut self, src: &Image, src_rect: Rect, x: i32, y: i32) {
        let Some((x0, y0, x1, y1)) = src.clip_i32(src_rect) else {
            return;
        };
        for sy in y0..y1 {
            for sx in x0..x1 {
                let pixel = src.pixels[(sy as usize) * (src.width as usize) + sx as usize];
                if pixel.is_transparent() {
                    continue;
                }
                let dx = i64::from(x) + i64::from(sx - x0);
                let dy = i64::from(y) + i64::from(sy - y0);
                if dx < 0 || dy < 0 || dx >= i64::from(self.width) || dy >= i64::from(self.height) {
                    continue;
                }
                let di = (dy as usize) * (self.width as usize) + dx as usize;
                self.pixels[di] = pixel.blend_over(self.pixels[di]);
            }
        }
    }

    /// Mirrors the image horizontally.
    #[must_use]
    pub fn flip_x(&self) -> Image {
        let mut out = self.clone();
        for y in 0..self.height {
            let row = (y as usize) * (self.width as usize);
            for x in 0..(self.width as usize) / 2 {
                let a = row + x;
                let b = row + (self.width as usize) - 1 - x;
                out.pixels.swap(a, b);
            }
        }
        out
    }

    /// Mirrors the image vertically.
    #[must_use]
    pub fn flip_y(&self) -> Image {
        let mut out = self.clone();
        let w = self.width as usize;
        for y in 0..(self.height as usize) / 2 {
            let top = y * w;
            let bottom = ((self.height as usize) - 1 - y) * w;
            for x in 0..w {
                out.pixels.swap(top + x, bottom + x);
            }
        }
        out
    }

    /// Rotates the image 90° clockwise, swapping width and height.
    #[must_use]
    pub fn rotate_90_cw(&self) -> Image {
        let mut out = Image::new(self.height, self.width, Color8::TRANSPARENT);
        for y in 0..self.height {
            for x in 0..self.width {
                let pixel = self.pixels[(y as usize) * (self.width as usize) + x as usize];
                // (x, y) -> (height - 1 - y, x) in the rotated frame.
                out.set(self.height - 1 - y, x, pixel);
            }
        }
        out
    }

    /// Rotates the image 90° counter-clockwise.
    #[must_use]
    pub fn rotate_90_ccw(&self) -> Image {
        let mut out = Image::new(self.height, self.width, Color8::TRANSPARENT);
        for y in 0..self.height {
            for x in 0..self.width {
                let pixel = self.pixels[(y as usize) * (self.width as usize) + x as usize];
                out.set(y, self.width - 1 - x, pixel);
            }
        }
        out
    }

    /// Copies the `rect` region into a new image, clipped to the bounds.
    ///
    /// A region that does not overlap the image produces a 0×0 image.
    #[must_use]
    pub fn crop(&self, rect: Rect) -> Image {
        let Some((x0, y0, x1, y1)) = self.clip_i32(rect) else {
            return Image::new(0, 0, Color8::TRANSPARENT);
        };
        let w = (x1 - x0) as u32;
        let h = (y1 - y0) as u32;
        let mut out = Image::new(w, h, Color8::TRANSPARENT);
        for y in 0..h {
            for x in 0..w {
                out.set(x, y, self.get_or_transparent(x0 as u32 + x, y0 as u32 + y));
            }
        }
        out
    }

    /// Nearest-neighbour resample to `width` × `height`.
    ///
    /// Nearest is the right default for pixel art: it never invents colours.
    /// A zero-sized result, or a resample of an empty image, is empty.
    #[must_use]
    pub fn scale_nearest(&self, width: u32, height: u32) -> Image {
        if width == 0 || height == 0 || self.is_empty() {
            return Image::new(width, height, Color8::TRANSPARENT);
        }
        let mut out = Image::new(width, height, Color8::TRANSPARENT);
        for y in 0..height {
            let sy = ((y as u64) * (self.height as u64) / (height as u64)) as u32;
            for x in 0..width {
                let sx = ((x as u64) * (self.width as u64) / (width as u64)) as u32;
                out.set(x, y, self.get_or_transparent(sx, sy));
            }
        }
        out
    }

    /// Returns a copy grown by one pixel on every side, with a solid `color`
    /// border drawn around the non-transparent pixels of the original.
    ///
    /// The original pixels themselves are copied unchanged, so the interior is
    /// never touched. The outline is 8-connected: diagonal steps are included,
    /// which is what makes a sprite read as a single shape.
    #[must_use]
    pub fn outline(&self, color: Color8) -> Image {
        let mut out = Image::new(self.width + 2, self.height + 2, Color8::TRANSPARENT);
        for y in 0..out.height {
            for x in 0..out.width {
                let sx = i64::from(x) - 1;
                let sy = i64::from(y) - 1;
                match self.get_at(sx, sy) {
                    Some(pixel) if !pixel.is_transparent() => out.set(x, y, pixel),
                    _ => {
                        let touches = (-1i64..=1).any(|dy| {
                            (-1i64..=1).any(|dx| {
                                (dx != 0 || dy != 0)
                                    && self
                                        .get_at(sx + dx, sy + dy)
                                        .is_some_and(|n| !n.is_transparent())
                            })
                        });
                        if touches {
                            out.set(x, y, color);
                        }
                    }
                }
            }
        }
        out
    }

    /// Returns a copy in which every non-transparent pixel keeps its alpha but
    /// takes the RGB of `color`.
    #[must_use]
    pub fn silhouette(&self, color: Color8) -> Image {
        let mut out = self.clone();
        for pixel in &mut out.pixels {
            if pixel.a != 0 {
                pixel.r = color.r;
                pixel.g = color.g;
                pixel.b = color.b;
            }
        }
        out
    }

    /// Replaces every pixel exactly equal to `from` with `to`, returning how
    /// many pixels changed.
    pub fn replace_color(&mut self, from: Color8, to: Color8) -> usize {
        let mut count = 0;
        for pixel in &mut self.pixels {
            if *pixel == from {
                *pixel = to;
                count += 1;
            }
        }
        count
    }

    /// The distinct colours in the image, in first-appearance (row-major) order.
    #[must_use]
    pub fn unique_colors(&self) -> Vec<Color8> {
        let mut seen: Vec<Color8> = Vec::new();
        for &pixel in &self.pixels {
            if !seen.contains(&pixel) {
                seen.push(pixel);
            }
        }
        seen
    }

    /// Snaps every pixel to the nearest palette entry, returning a new image.
    ///
    /// Fully transparent pixels stay transparent (a palette is a colour list,
    /// not a mask), and an empty palette leaves the image untouched.
    #[must_use]
    pub fn to_palette(&self, palette: &Palette) -> Image {
        let mut out = self.clone();
        out.apply_palette(palette);
        out
    }

    /// In-place variant of [`Image::to_palette`].
    pub fn apply_palette(&mut self, palette: &Palette) {
        if palette.is_empty() {
            return;
        }
        for pixel in &mut self.pixels {
            if pixel.a == 0 {
                *pixel = Color8::TRANSPARENT;
                continue;
            }
            if let Some(entry) = palette.nearest(*pixel) {
                *pixel = entry.color;
            }
        }
    }

    /// Four-connected flood fill starting at `(x, y)`.
    ///
    /// Does nothing when the start is out of bounds or already `color`.
    pub fn flood_fill(&mut self, x: u32, y: u32, color: Color8) {
        let Some(start) = self.get(x, y) else {
            return;
        };
        if start == color {
            return;
        }
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            if self.get(cx, cy) != Some(start) {
                continue;
            }
            self.set(cx, cy, color);
            if cx > 0 {
                stack.push((cx - 1, cy));
            }
            if cy > 0 {
                stack.push((cx, cy - 1));
            }
            if cx + 1 < self.width {
                stack.push((cx + 1, cy));
            }
            if cy + 1 < self.height {
                stack.push((cx, cy + 1));
            }
        }
    }

    /// Repeats the image `cols` × `rows` times into one seamless sheet.
    ///
    /// The result is `width * cols` by `height * rows`; because every cell is an
    /// exact copy, the seams of genuinely seamless source art line up. A zero
    /// count, or an empty source, produces an empty image.
    #[must_use]
    pub fn tile(&self, cols: u32, rows: u32) -> Image {
        if cols == 0 || rows == 0 || self.is_empty() {
            return Image::new(0, 0, Color8::TRANSPARENT);
        }
        let width = self.width.saturating_mul(cols);
        let height = self.height.saturating_mul(rows);
        let mut out = Image::new(width, height, Color8::TRANSPARENT);
        for row in 0..rows {
            for col in 0..cols {
                let ox = (col * self.width) as i32;
                let oy = (row * self.height) as i32;
                out.blit(self, ox, oy);
            }
        }
        out
    }

    /// Alpha-aware paste used by [`Image::tile`] that also copies transparent
    /// pixels, overriding [`Image::blit`]'s "skip transparent" rule.
    ///
    /// Useful when stamping a rectangular chunk of a sprite sheet.
    pub fn stamp(&mut self, src: &Image, x: i32, y: i32) {
        for sy in 0..src.height as i64 {
            let dy = i64::from(y) + sy;
            if dy < 0 || dy >= i64::from(self.height) {
                continue;
            }
            for sx in 0..src.width as i64 {
                let dx = i64::from(x) + sx;
                if dx < 0 || dx >= i64::from(self.width) {
                    continue;
                }
                let pixel = src.pixels[(sy as usize) * (src.width as usize) + sx as usize];
                self.pixels[(dy as usize) * (self.width as usize) + dx as usize] = pixel;
            }
        }
    }

    /// Decodes a PNG byte slice.
    pub fn from_png_bytes(bytes: &[u8]) -> Result<Image, PngError> {
        png::decode(bytes)
    }

    /// Encodes the image as a PNG byte vector.
    #[must_use]
    pub fn to_png_bytes(&self) -> Vec<u8> {
        png::encode(self)
    }

    /// Reads and decodes a PNG file.
    ///
    /// A codec error is reported as [`io::ErrorKind::InvalidData`].
    pub fn load_png(path: impl AsRef<Path>) -> io::Result<Image> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        png::decode(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Writes the image as a PNG file, atomically (temp file + rename).
    pub fn save_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        crate::db::write_atomic(path, &self.to_png_bytes())
    }

    /// Clips `rect` to the image bounds, in whole pixels.
    ///
    /// Returns `None` when the result would be empty.
    fn clip_i32(&self, rect: Rect) -> Option<(i32, i32, i32, i32)> {
        let (rx0, ry0, rx1, ry1) = rect.to_pixel_bounds();
        let x0 = rx0.clamp(0, self.width as i32);
        let y0 = ry0.clamp(0, self.height as i32);
        let x1 = rx1.clamp(0, self.width as i32);
        let y1 = ry1.clamp(0, self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1, y1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(size: u32, a: Color8, b: Color8) -> Image {
        Image::from_fn(size, size, |x, y| if (x + y) % 2 == 0 { a } else { b })
    }

    #[test]
    fn new_and_accessors() {
        let img = Image::new(4, 3, Color8::RED);
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.pixels.len(), 12);
        assert_eq!(img.get(3, 2), Some(Color8::RED));
        assert_eq!(img.get(4, 2), None);
        assert_eq!(img.get(0, 3), None);
        assert_eq!(img.get_or_transparent(9, 9), Color8::TRANSPARENT);
        assert_eq!(img.get_at(-1, 0), None);
        assert_eq!(img.get_at(0, 0), Some(Color8::RED));
        assert_eq!(img.index_of(1, 1), Some(5));
        assert_eq!(img.pixel_count(), 12);
        assert!(!img.is_empty());
        assert_eq!(img.rect().width(), 4.0);
        assert_eq!(img.rect().height(), 3.0);
        assert!(Image::new(0, 5, Color8::RED).is_empty());
        assert_eq!(Image::new(0, 5, Color8::RED).pixels.len(), 0);
    }

    #[test]
    fn from_pixels_validates_length() {
        let ok = Image::from_pixels(2, 2, vec![Color8::WHITE; 4]);
        assert!(ok.is_some());
        assert!(Image::from_pixels(2, 2, vec![Color8::WHITE; 3]).is_none());
        assert!(Image::from_pixels(0, 0, Vec::new()).is_some());
    }

    #[test]
    fn set_ignores_out_of_bounds_and_reports_it() {
        let mut img = Image::new(2, 2, Color8::BLACK);
        img.set(5, 5, Color8::WHITE);
        img.set(1, 1, Color8::GREEN);
        assert_eq!(img.get(1, 1), Some(Color8::GREEN));
        assert!(!img.try_set(2, 0, Color8::WHITE));
        assert!(img.try_set(0, 0, Color8::WHITE));
        assert_eq!(img.unique_colors().len(), 3);
        img.clear(Color8::BLUE);
        assert_eq!(img.unique_colors(), vec![Color8::BLUE]);
    }

    #[test]
    fn fill_rect_clips_and_covers_whole_pixels() {
        let mut img = Image::new(4, 4, Color8::BLACK);
        img.fill_rect(
            Rect::from_min_max(Vec2::new(1.2, 1.2), Vec2::new(2.1, 2.9)),
            Color8::WHITE,
        );
        assert_eq!(img.get(1, 1), Some(Color8::WHITE));
        assert_eq!(img.get(2, 2), Some(Color8::WHITE));
        assert_eq!(img.get(3, 3), Some(Color8::BLACK));
        assert_eq!(img.get(0, 0), Some(Color8::BLACK));

        img.fill_rect(
            Rect::from_min_max(Vec2::new(-5.0, -5.0), Vec2::new(50.0, 50.0)),
            Color8::RED,
        );
        assert!(img.pixels.iter().all(|p| *p == Color8::RED));

        img.fill_rect(Rect::EMPTY, Color8::GREEN);
        assert!(img.pixels.iter().all(|p| *p == Color8::RED));
    }

    #[test]
    fn blit_composites_alpha_over_destination() {
        let mut dst = Image::new(2, 2, Color8::new(0, 0, 0, 255));
        let src = Image::new(2, 2, Color8::new(255, 255, 255, 128));
        dst.blit(&src, 0, 0);
        let px = dst.get(0, 0).unwrap();
        assert!(px.r > 120 && px.r < 136, "{px:?}");
        assert_eq!(px.a, 255);

        // A fully transparent source leaves the destination alone.
        let clear = Image::new(2, 2, Color8::TRANSPARENT);
        dst.blit(&clear, 0, 0);
        assert_eq!(dst.get(0, 0), Some(px));

        // Out-of-bounds overhang is clipped, not panicking.
        let mut small = Image::new(2, 2, Color8::BLACK);
        small.blit(&Image::new(3, 3, Color8::WHITE), -1, -1);
        assert_eq!(small.get(0, 0), Some(Color8::WHITE));
        assert_eq!(small.get(1, 1), Some(Color8::WHITE));
        small.blit(&Image::new(2, 2, Color8::RED), 10, 10);
        assert_eq!(small.get(0, 0), Some(Color8::WHITE));
    }

    #[test]
    fn blit_region_uses_the_sub_rectangle() {
        let mut sheet = Image::from_fn(4, 1, |x, _| Color8::rgb(x as u8 * 10, 0, 0));
        let region = Rect::from_min_max(Vec2::new(1.0, 0.0), Vec2::new(3.0, 1.0));
        let mut dst = Image::new(2, 1, Color8::TRANSPARENT);
        dst.blit_region(&sheet, region, 0, 0);
        assert_eq!(dst.get(0, 0), Some(Color8::rgb(10, 0, 0)));
        assert_eq!(dst.get(1, 0), Some(Color8::rgb(20, 0, 0)));

        // Clipping: a region hanging off the right edge only copies what exists.
        let mut clip = Image::new(4, 1, Color8::TRANSPARENT);
        clip.blit_region(
            &sheet,
            Rect::from_min_max(Vec2::new(2.0, 0.0), Vec2::new(9.0, 1.0)),
            2,
            0,
        );
        assert_eq!(clip.get(2, 0), Some(Color8::rgb(20, 0, 0)));
        assert_eq!(clip.get(3, 0), Some(Color8::rgb(30, 0, 0)));

        sheet.set(0, 0, Color8::TRANSPARENT);
        let mut dst2 = Image::new(1, 1, Color8::WHITE);
        dst2.blit_region(
            &sheet,
            Rect::from_min_max(Vec2::ZERO, Vec2::new(1.0, 1.0)),
            0,
            0,
        );
        assert_eq!(
            dst2.get(0, 0),
            Some(Color8::WHITE),
            "transparent source is skipped"
        );
    }

    #[test]
    fn flips_are_involutions() {
        let img = checker(4, Color8::RED, Color8::BLUE);
        assert_eq!(img.flip_x().flip_x(), img);
        assert_eq!(img.flip_y().flip_y(), img);
        assert_ne!(img.flip_x(), img);
        let row = Image::from_fn(4, 1, |x, _| Color8::rgb(x as u8, 0, 0));
        let flipped = row.flip_x();
        assert_eq!(flipped.get(0, 0), Some(Color8::rgb(3, 0, 0)));
        assert_eq!(flipped.get(3, 0), Some(Color8::rgb(0, 0, 0)));
    }

    #[test]
    fn rotate_90_four_times_is_identity() {
        let img = Image::from_fn(3, 2, |x, y| Color8::rgb(x as u8, y as u8, 7));
        let r1 = img.rotate_90_cw();
        assert_eq!((r1.width, r1.height), (2, 3));
        assert_eq!(r1.get(1, 0), img.get(0, 0));
        let r4 = r1.rotate_90_cw().rotate_90_cw().rotate_90_cw();
        assert_eq!(r4, img);
        assert_eq!(img.rotate_90_ccw().rotate_90_cw(), img);
        let wide = Image::new(5, 1, Color8::GREEN);
        assert_eq!(wide.rotate_90_cw().rotate_90_ccw(), wide);
    }

    #[test]
    fn crop_clips_to_bounds() {
        let img = Image::from_fn(4, 4, |x, y| Color8::rgb(x as u8, y as u8, 0));
        let c = img.crop(Rect::from_min_max(Vec2::new(1.0, 1.0), Vec2::new(3.0, 3.0)));
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.get(0, 0), Some(Color8::rgb(1, 1, 0)));
        assert_eq!(c.get(1, 1), Some(Color8::rgb(2, 2, 0)));

        let over = img.crop(Rect::from_min_max(
            Vec2::new(2.0, 2.0),
            Vec2::new(99.0, 99.0),
        ));
        assert_eq!((over.width, over.height), (2, 2));
        assert!(img.crop(Rect::EMPTY).is_empty());
    }

    #[test]
    fn scale_nearest_keeps_the_checkerboard() {
        let img = checker(2, Color8::WHITE, Color8::BLACK);
        let up = img.scale_nearest(4, 4);
        assert_eq!((up.width, up.height), (4, 4));
        assert_eq!(up.get(0, 0), Some(Color8::WHITE));
        assert_eq!(up.get(1, 0), Some(Color8::WHITE));
        assert_eq!(up.get(2, 0), Some(Color8::BLACK));
        assert_eq!(up.get(1, 1), Some(Color8::WHITE));
        let down = up.scale_nearest(2, 2);
        assert_eq!(down, img);
        assert_eq!(img.scale_nearest(4, 0).pixel_count(), 0);
        let empty = Image::new(0, 0, Color8::WHITE).scale_nearest(2, 2);
        assert!(empty.pixels.iter().all(|p| p.is_transparent()));
    }

    #[test]
    fn outline_adds_a_border_without_touching_the_interior() {
        let mut img = Image::new(3, 3, Color8::TRANSPARENT);
        img.set(1, 1, Color8::RED);
        let out = img.outline(Color8::BLACK);
        assert_eq!((out.width, out.height), (5, 5));
        for y in 0..5 {
            for x in 0..5 {
                let expected = match (x, y) {
                    (2, 2) => Color8::RED,
                    (1..=3, 1..=3) => Color8::BLACK,
                    _ => Color8::TRANSPARENT,
                };
                assert_eq!(out.get(x, y), Some(expected), "at {x},{y}");
            }
        }
        // A fully transparent image never gains an outline.
        let empty = Image::new(2, 2, Color8::TRANSPARENT).outline(Color8::BLACK);
        assert!(empty.pixels.iter().all(|p| p.is_transparent()));
    }

    #[test]
    fn silhouette_recolours_and_keeps_alpha() {
        let img = Image::from_fn(2, 1, |x, _| {
            if x == 0 {
                Color8::new(10, 20, 30, 255)
            } else {
                Color8::new(40, 50, 60, 100)
            }
        });
        let sil = img.silhouette(Color8::new(255, 0, 0, 255));
        assert_eq!(sil.get(0, 0), Some(Color8::new(255, 0, 0, 255)));
        assert_eq!(sil.get(1, 0), Some(Color8::new(255, 0, 0, 100)));
    }

    #[test]
    fn replace_color_and_unique_colors() {
        let mut img = checker(4, Color8::RED, Color8::BLUE);
        assert_eq!(img.unique_colors(), vec![Color8::RED, Color8::BLUE]);
        assert_eq!(img.replace_color(Color8::RED, Color8::GREEN), 8);
        assert_eq!(img.unique_colors(), vec![Color8::GREEN, Color8::BLUE]);
        assert_eq!(img.replace_color(Color8::WHITE, Color8::BLACK), 0);
    }

    #[test]
    fn flood_fill_fills_a_region() {
        let mut img = Image::new(4, 4, Color8::WHITE);
        img.fill_rect(
            Rect::from_min_max(Vec2::new(1.0, 1.0), Vec2::new(3.0, 3.0)),
            Color8::BLACK,
        );
        img.flood_fill(0, 0, Color8::BLUE);
        assert_eq!(img.get(0, 0), Some(Color8::BLUE));
        assert_eq!(img.get(3, 3), Some(Color8::BLUE));
        assert_eq!(
            img.get(1, 1),
            Some(Color8::BLACK),
            "the wall is not crossed"
        );
        assert_eq!(img.get(2, 2), Some(Color8::BLACK));
        // Filling with the colour already there is a no-op; OOB starts do nothing.
        img.flood_fill(1, 1, Color8::BLACK);
        img.flood_fill(99, 99, Color8::RED);
        assert_eq!(img.get(1, 1), Some(Color8::BLACK));
    }

    #[test]
    fn tile_repeats_seamlessly() {
        let img = Image::from_fn(2, 2, |x, y| Color8::rgb(x as u8, y as u8, 0));
        let tiled = img.tile(3, 2);
        assert_eq!((tiled.width, tiled.height), (6, 4));
        assert_eq!(tiled.get(0, 0), img.get(0, 0));
        assert_eq!(tiled.get(2, 0), img.get(0, 0));
        assert_eq!(tiled.get(5, 3), img.get(1, 1));
        // The repeat is exact on both axes: sampling (x + w, y) == (x, y).
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(tiled.get(x + 2, y), tiled.get(x, y));
            }
        }
        assert!(img.tile(0, 3).is_empty());
    }

    #[test]
    fn palette_snapping_keeps_transparency() {
        let mut palette = Palette::new();
        palette.push("red", Color8::rgb(255, 0, 0));
        palette.push("blue", Color8::rgb(0, 0, 255));
        let img =
            Image::from_pixels(2, 1, vec![Color8::rgb(250, 4, 4), Color8::TRANSPARENT]).unwrap();
        let snapped = img.to_palette(&palette);
        assert_eq!(snapped.get(0, 0), Some(Color8::rgb(255, 0, 0)));
        assert_eq!(snapped.get(1, 0), Some(Color8::TRANSPARENT));
        let untouched = img.to_palette(&Palette::new());
        assert_eq!(untouched, img);
    }

    #[test]
    fn png_byte_round_trip_through_image_helpers() {
        let img = checker(8, Color8::MAGENTA, Color8::new(1, 2, 3, 40));
        let bytes = img.to_png_bytes();
        assert_eq!(Image::from_png_bytes(&bytes).unwrap(), img);
        assert!(Image::from_png_bytes(b"not a png").is_err());
    }

    #[test]
    fn stamp_copies_transparent_pixels_too() {
        let mut dst = Image::new(2, 2, Color8::WHITE);
        dst.stamp(&Image::new(1, 1, Color8::TRANSPARENT), 0, 0);
        assert_eq!(dst.get(0, 0), Some(Color8::TRANSPARENT));
        assert_eq!(dst.get(1, 1), Some(Color8::WHITE));
        dst.stamp(&Image::new(2, 2, Color8::RED), -5, -5);
        assert_eq!(dst.get(0, 0), Some(Color8::TRANSPARENT));
    }
}
