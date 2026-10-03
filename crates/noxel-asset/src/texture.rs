//! Texture sampling: nearest, bilinear and the pixel-art special case.
//!
//! A [`Texture`] is an [`Image`] plus the sampling metadata the renderer needs:
//! today that is the [`WrapMode`]. Every sampler takes normalised UV
//! coordinates, where `(0, 0)` is the top-left corner of the image and `(1, 1)`
//! the bottom-right.
//!
//! ## Which sampler should I use?
//!
//! | Sampler | Use for |
//! |---|---|
//! | [`Texture::sample_nearest`] | crisp magnification of arbitrary images |
//! | [`Texture::sample_bilinear`] | smooth magnification, UI scaling |
//! | [`Texture::sample_pixel_art`] | sprites and atlas cells: snapped to the nearest texel *centre* so neighbouring atlas cells can never bleed in |
//! | [`Texture::sample_region_nearest`] | one cell of a sprite sheet, given its pixel rect |
//!
//! ## Example
//!
//! ```
//! use noxel_asset::image::Image;
//! use noxel_asset::texture::Texture;
//! use noxel_core::math::Color8;
//!
//! let texture = Texture::from_image(Image::new(2, 1, Color8::RED));
//! // The centre of texel 0 is at u = 0.25 for a two-pixel-wide texture.
//! assert_eq!(texture.sample_nearest(0.25, 0.5), Color8::RED);
//! ```

use crate::image::Image;
use noxel_core::math::{Color8, Rect, Vec2};

/// How a sampler resolves coordinates outside `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Coordinates are clamped to the edge texel.
    #[default]
    Clamp,
    /// Coordinates wrap around, so the texture tiles.
    Repeat,
    /// Coordinates reflect back and forth across the edges.
    Mirror,
}

/// An image plus the sampling metadata needed to read from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    image: Image,
    wrap: WrapMode,
}

impl Texture {
    /// Wraps an image with the default [`WrapMode::Clamp`].
    #[must_use]
    pub fn from_image(image: Image) -> Self {
        Self {
            image,
            wrap: WrapMode::Clamp,
        }
    }

    /// Wraps an image with an explicit wrap mode.
    #[must_use]
    pub fn from_image_with_wrap(image: Image, wrap: WrapMode) -> Self {
        Self { image, wrap }
    }

    /// A fully transparent texture of the given size.
    #[must_use]
    pub fn transparent(width: u32, height: u32) -> Self {
        Self::from_image(Image::transparent(width, height))
    }

    /// Width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.image.width
    }

    /// Height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.image.height
    }

    /// The underlying image.
    #[must_use]
    pub fn image(&self) -> &Image {
        &self.image
    }

    /// Consumes the texture and returns the image.
    #[must_use]
    pub fn into_image(self) -> Image {
        self.image
    }

    /// The wrap mode used by every sampler on this texture.
    #[must_use]
    pub fn wrap(&self) -> WrapMode {
        self.wrap
    }

    /// Changes the wrap mode.
    pub fn set_wrap(&mut self, wrap: WrapMode) {
        self.wrap = wrap;
    }

    /// Builder-style [`Texture::set_wrap`].
    #[must_use]
    pub fn with_wrap(mut self, wrap: WrapMode) -> Self {
        self.wrap = wrap;
        self
    }

    /// Nearest-neighbour sample at normalised `(u, v)`, honouring the wrap mode.
    #[must_use]
    pub fn sample_nearest(&self, u: f32, v: f32) -> Color8 {
        if self.image.is_empty() {
            return Color8::TRANSPARENT;
        }
        let x = (u * self.width() as f32).floor() as i32;
        let y = (v * self.height() as f32).floor() as i32;
        self.fetch(x, y)
    }

    /// Bilinear sample at normalised `(u, v)`, honouring the wrap mode.
    ///
    /// Sampling exactly on a texel centre returns that texel unchanged, which is
    /// what makes a bilinear-filtered sprite sheet stable when the camera is
    /// pixel-aligned.
    #[must_use]
    pub fn sample_bilinear(&self, u: f32, v: f32) -> Color8 {
        if self.image.is_empty() {
            return Color8::TRANSPARENT;
        }
        let fx = u * self.width() as f32 - 0.5;
        let fy = v * self.height() as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (x0, y0) = (x0 as i32, y0 as i32);

        let top = self.fetch(x0, y0).lerp(self.fetch(x0 + 1, y0), tx);
        let bottom = self.fetch(x0, y0 + 1).lerp(self.fetch(x0 + 1, y0 + 1), tx);
        top.lerp(bottom, ty)
    }

    /// Nearest sample of a pixel rectangle inside the texture.
    ///
    /// `u` and `v` are local to `rect`: `(0, 0)` is the rect's top-left texel and
    /// `(1, 1)` its bottom-right. Coordinates are clamped to the rect, so no
    /// sample can land outside it. Use this to read one frame of a sprite sheet.
    #[must_use]
    pub fn sample_region_nearest(&self, rect: Rect, u: f32, v: f32) -> Color8 {
        if self.image.is_empty() {
            return Color8::TRANSPARENT;
        }
        let width = rect.width();
        let height = rect.height();
        if width <= 0.0 || height <= 0.0 {
            return Color8::TRANSPARENT;
        }
        let x0 = rect.min.x.floor() as i32;
        let y0 = rect.min.y.floor() as i32;
        let last_x = (rect.max.x.ceil() as i32 - 1).max(x0);
        let last_y = (rect.max.y.ceil() as i32 - 1).max(y0);
        let x = x0 + (u.clamp(0.0, 1.0) * width).floor() as i32;
        let y = y0 + (v.clamp(0.0, 1.0) * height).floor() as i32;
        self.fetch(x.clamp(x0, last_x), y.clamp(y0, last_y))
    }

    /// Nearest sample with a half-texel inset, for pixel-art renderers.
    ///
    /// Snapping to the texel *centre* is what stops adjacent atlas cells from
    /// bleeding into each other: a `u` that falls exactly on the boundary
    /// between two texels resolves to the left/top one, and `u == 1.0` resolves
    /// to the last texel rather than wrapping around to the first.
    #[must_use]
    pub fn sample_pixel_art(&self, u: f32, v: f32) -> Color8 {
        if self.image.is_empty() {
            return Color8::TRANSPARENT;
        }
        let width = self.width() as f32;
        let height = self.height() as f32;
        let u = u.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let x = ((u * width).floor() + 0.5).clamp(0.5, width - 0.5);
        let y = ((v * height).floor() + 0.5).clamp(0.5, height - 0.5);
        self.image
            .get_or_transparent(x.floor() as u32, y.floor() as u32)
    }

    /// A half-size, box-filtered copy of this texture.
    ///
    /// Each destination texel averages up to 2×2 source texels. RGB is weighted
    /// by alpha so that a transparent texel next to an opaque one does not drag
    /// a dark fringe into the result; the alpha channel is a plain average. A
    /// one-texel axis stops shrinking at 1.
    #[must_use]
    pub fn generate_mip(&self) -> Texture {
        let width = self.width().max(1);
        let height = self.height().max(1);
        let new_width = (width / 2).max(1);
        let new_height = (height / 2).max(1);
        let mut out = Image::new(new_width, new_height, Color8::TRANSPARENT);
        for y in 0..new_height {
            for x in 0..new_width {
                out.set(x, y, self.average_block(x * 2, y * 2, width, height));
            }
        }
        Texture {
            image: out,
            wrap: self.wrap,
        }
    }

    /// `levels` mip levels, starting with a copy of this texture as level 0.
    ///
    /// The chain stops early when a level measures 1×1 on both axes, and returns
    /// an empty vector for `levels == 0`.
    #[must_use]
    pub fn generate_mip_chain(&self, levels: u32) -> Vec<Texture> {
        let mut chain = Vec::new();
        if levels == 0 {
            return chain;
        }
        chain.push(self.clone());
        while (chain.len() as u32) < levels {
            let last = chain.last().expect("chain is non-empty");
            if last.width() <= 1 && last.height() <= 1 {
                break;
            }
            chain.push(last.generate_mip());
        }
        chain
    }

    /// Alpha-weighted average of the up-to-2×2 source block at `(x, y)`.
    fn average_block(&self, x: u32, y: u32, width: u32, height: u32) -> Color8 {
        let mut alpha_sum = 0u32;
        let mut red = 0u32;
        let mut green = 0u32;
        let mut blue = 0u32;
        for dy in 0..2u32 {
            for dx in 0..2u32 {
                let sx = (x + dx).min(width - 1);
                let sy = (y + dy).min(height - 1);
                let pixel = self.image.get_or_transparent(sx, sy);
                let a = u32::from(pixel.a);
                alpha_sum += a;
                red += u32::from(pixel.r) * a;
                green += u32::from(pixel.g) * a;
                blue += u32::from(pixel.b) * a;
            }
        }
        if alpha_sum == 0 {
            return Color8::TRANSPARENT;
        }
        Color8::new(
            (red / alpha_sum) as u8,
            (green / alpha_sum) as u8,
            (blue / alpha_sum) as u8,
            ((alpha_sum + 2) / 4) as u8,
        )
    }

    /// Fetches a texel by integer coordinate, applying the wrap mode.
    fn fetch(&self, x: i32, y: i32) -> Color8 {
        if self.image.is_empty() {
            return Color8::TRANSPARENT;
        }
        let wx = self.wrap_coordinate(x, self.width());
        let wy = self.wrap_coordinate(y, self.height());
        self.image.get_or_transparent(wx, wy)
    }

    fn wrap_coordinate(&self, value: i32, size: u32) -> u32 {
        let size = size as i32;
        match self.wrap {
            WrapMode::Clamp => value.clamp(0, size - 1) as u32,
            WrapMode::Repeat => value.rem_euclid(size) as u32,
            WrapMode::Mirror => {
                let period = 2 * size;
                let m = value.rem_euclid(period);
                if m < size {
                    m as u32
                } else {
                    (period - 1 - m) as u32
                }
            }
        }
    }

    /// The UV rectangle of the whole texture.
    #[must_use]
    pub fn uv_rect(&self) -> Rect {
        Rect::from_min_max(Vec2::ZERO, Vec2::new(1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×4 texture whose texel `(x, y)` has a unique, easily predicted colour.
    fn grid() -> Texture {
        Texture::from_image(Image::from_fn(4, 4, |x, y| {
            Color8::new(x as u8 * 10, y as u8 * 10, 0, 255)
        }))
    }

    /// The centre of texel `(x, y)` in a `w`×`h` texture.
    fn centre(x: u32, y: u32, w: u32, h: u32) -> (f32, f32) {
        ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
    }

    #[test]
    fn nearest_hits_exact_texel_centres() {
        let texture = grid();
        for y in 0..4 {
            for x in 0..4 {
                let (u, v) = centre(x, y, 4, 4);
                assert_eq!(
                    texture.sample_nearest(u, v),
                    Color8::rgb(x as u8 * 10, y as u8 * 10, 0)
                );
            }
        }
        assert_eq!(texture.sample_nearest(0.0, 0.0), Color8::rgb(0, 0, 0));
        assert_eq!(texture.sample_nearest(0.999, 0.999), Color8::rgb(30, 30, 0));
        assert_eq!(
            Texture::transparent(0, 0).sample_nearest(0.5, 0.5),
            Color8::TRANSPARENT
        );
    }

    #[test]
    fn nearest_clamps_outside_the_unit_square() {
        let texture = grid();
        assert_eq!(texture.sample_nearest(-1.0, -1.0), Color8::rgb(0, 0, 0));
        assert_eq!(texture.sample_nearest(2.0, 2.0), Color8::rgb(30, 30, 0));
        assert_eq!(texture.sample_nearest(1.0, 1.0), Color8::rgb(30, 30, 0));
    }

    #[test]
    fn wrap_modes_behave_differently() {
        let base = grid();
        assert_eq!(base.wrap(), WrapMode::Clamp);

        let repeat = base.clone().with_wrap(WrapMode::Repeat);
        assert_eq!(
            repeat.sample_nearest(1.25, 0.125),
            base.sample_nearest(0.25, 0.125)
        );
        assert_eq!(
            repeat.sample_nearest(-0.75, 0.125),
            base.sample_nearest(0.25, 0.125)
        );
        assert_eq!(repeat.sample_nearest(1.0, 0.0), Color8::rgb(0, 0, 0));

        let mirror = base.clone().with_wrap(WrapMode::Mirror);
        // Texel -1 reflects onto texel 0, and texel 5 (u = 1.25) onto texel 2.
        assert_eq!(mirror.sample_nearest(-0.25, 0.125), Color8::rgb(0, 0, 0));
        assert_eq!(mirror.sample_nearest(1.25, 0.125), Color8::rgb(20, 0, 0));
        assert_eq!(mirror.sample_nearest(1.75, 0.125), Color8::rgb(0, 0, 0));

        let mut mutable = base.clone();
        mutable.set_wrap(WrapMode::Repeat);
        assert_eq!(mutable.wrap(), WrapMode::Repeat);
        assert_eq!(
            mutable.sample_nearest(1.25, 0.125),
            repeat.sample_nearest(1.25, 0.125)
        );
        assert_eq!(WrapMode::default(), WrapMode::Clamp);
    }

    #[test]
    fn bilinear_is_exact_on_texel_centres() {
        let texture = grid();
        for y in 0..4 {
            for x in 0..4 {
                let (u, v) = centre(x, y, 4, 4);
                assert_eq!(
                    texture.sample_bilinear(u, v),
                    Color8::rgb(x as u8 * 10, y as u8 * 10, 0),
                    "texel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn bilinear_blends_between_texels() {
        let texture = Texture::from_image(
            Image::from_pixels(2, 1, vec![Color8::rgb(0, 0, 0), Color8::rgb(200, 200, 200)])
                .unwrap(),
        );
        // Halfway between the two texel centres: u = 0.5 in a 2-wide texture.
        let mid = texture.sample_bilinear(0.5, 0.5);
        assert!(mid.r > 90 && mid.r < 110, "{mid:?}");
        assert_eq!(mid.a, 255);
        assert_eq!(texture.sample_bilinear(0.0, 0.5), Color8::rgb(0, 0, 0));
    }

    #[test]
    fn bilinear_uses_the_wrap_mode() {
        let image =
            Image::from_pixels(2, 1, vec![Color8::rgb(0, 0, 0), Color8::rgb(100, 100, 100)])
                .unwrap();
        let repeat = Texture::from_image_with_wrap(image.clone(), WrapMode::Repeat);
        // u = 0 samples the centre of texel 1 wrapped around to texel 0… but at
        // u = 1.0 the "next" texel wraps back to the first, so the blend is
        // between texel 1 and texel 0.
        let left = repeat.sample_bilinear(0.0, 0.5);
        assert!(left.r > 40 && left.r < 60, "{left:?}");
        let clamp = Texture::from_image_with_wrap(image, WrapMode::Clamp);
        assert_eq!(clamp.sample_bilinear(0.0, 0.5), Color8::rgb(0, 0, 0));
    }

    #[test]
    fn pixel_art_snaps_to_texel_centres_and_never_bleeds() {
        let texture = grid();
        // Every sample lands on a texel centre, so it can never pick up a
        // fraction of a neighbour: a boundary value resolves to the texel that
        // starts there, and the edges stay inside the image.
        assert_eq!(texture.sample_pixel_art(0.0, 0.0), Color8::rgb(0, 0, 0));
        assert_eq!(texture.sample_pixel_art(0.25, 0.5), Color8::rgb(10, 20, 0));
        assert_eq!(texture.sample_pixel_art(0.5, 0.5), Color8::rgb(20, 20, 0));
        assert_eq!(
            texture.sample_pixel_art(0.999, 0.999),
            Color8::rgb(30, 30, 0)
        );
        assert_eq!(texture.sample_pixel_art(1.0, 1.0), Color8::rgb(30, 30, 0));
        assert_eq!(texture.sample_pixel_art(2.0, 2.0), Color8::rgb(30, 30, 0));
        assert_eq!(texture.sample_pixel_art(-5.0, 9.0), Color8::rgb(0, 30, 0));

        // The distinguishing property against `sample_nearest`: with wrapping
        // on, u = 1.0 stays on the last texel instead of jumping to the first,
        // which is exactly what stops atlas cells bleeding into each other.
        let repeat = grid().with_wrap(WrapMode::Repeat);
        assert_eq!(repeat.sample_nearest(1.0, 0.0), Color8::rgb(0, 0, 0));
        assert_eq!(repeat.sample_pixel_art(1.0, 0.0), Color8::rgb(30, 0, 0));
        assert_eq!(
            Texture::transparent(0, 0).sample_pixel_art(0.5, 0.5),
            Color8::TRANSPARENT
        );

        // A 1×1 texture is sampled by its only texel for every input.
        let one = Texture::from_image(Image::new(1, 1, Color8::MAGENTA));
        assert_eq!(one.sample_pixel_art(0.0, 0.0), Color8::MAGENTA);
        assert_eq!(one.sample_pixel_art(1.0, 1.0), Color8::MAGENTA);
    }

    #[test]
    fn region_sampling_stays_inside_the_region() {
        // A 8×2 sheet with two 4×2 cells; the right cell is red, the left blue.
        let sheet = Texture::from_image(Image::from_fn(8, 2, |x, _| {
            if x < 4 { Color8::BLUE } else { Color8::RED }
        }));
        let left = Rect::from_min_max(Vec2::ZERO, Vec2::new(4.0, 2.0));
        let right = Rect::from_min_max(Vec2::new(4.0, 0.0), Vec2::new(8.0, 2.0));

        assert_eq!(sheet.sample_region_nearest(left, 0.0, 0.0), Color8::BLUE);
        assert_eq!(sheet.sample_region_nearest(left, 1.0, 1.0), Color8::BLUE);
        assert_eq!(sheet.sample_region_nearest(right, 0.0, 0.0), Color8::RED);
        assert_eq!(
            sheet.sample_region_nearest(right, 0.999, 0.999),
            Color8::RED
        );
        // Out-of-range local UVs clamp rather than reading the neighbouring cell.
        assert_eq!(sheet.sample_region_nearest(right, -3.0, -3.0), Color8::RED);
        assert_eq!(sheet.sample_region_nearest(left, 17.0, 17.0), Color8::BLUE);
        // An empty region samples transparent.
        assert_eq!(
            sheet.sample_region_nearest(Rect::EMPTY, 0.5, 0.5),
            Color8::TRANSPARENT
        );
        assert_eq!(
            Texture::transparent(0, 0).sample_region_nearest(left, 0.5, 0.5),
            Color8::TRANSPARENT
        );
    }

    #[test]
    fn mip_halves_and_averages() {
        let image = Image::from_pixels(
            2,
            2,
            vec![
                Color8::rgb(0, 0, 0),
                Color8::rgb(100, 100, 100),
                Color8::rgb(200, 200, 200),
                Color8::rgb(40, 40, 40),
            ],
        )
        .unwrap();
        let mip = Texture::from_image(image).generate_mip();
        assert_eq!((mip.width(), mip.height()), (1, 1));
        assert_eq!(mip.sample_nearest(0.5, 0.5), Color8::rgb(85, 85, 85));
    }

    #[test]
    fn mip_is_alpha_weighted() {
        let image = Image::from_pixels(
            2,
            1,
            vec![Color8::new(255, 255, 255, 255), Color8::new(0, 0, 0, 0)],
        )
        .unwrap();
        let mip = Texture::from_image(image).generate_mip();
        assert_eq!((mip.width(), mip.height()), (1, 1));
        let pixel = mip.sample_nearest(0.5, 0.5);
        assert_eq!(
            pixel.r, 255,
            "the transparent texel must not darken the result"
        );
        assert_eq!(pixel.a, 128);
    }

    #[test]
    fn mip_chain_stops_at_one_texel() {
        let texture = Texture::from_image(Image::new(8, 4, Color8::GREEN));
        let chain = texture.generate_mip_chain(8);
        let sizes: Vec<(u32, u32)> = chain.iter().map(|t| (t.width(), t.height())).collect();
        assert_eq!(sizes, [(8, 4), (4, 2), (2, 1), (1, 1)]);
        assert!(
            chain
                .iter()
                .all(|t| t.sample_nearest(0.5, 0.5) == Color8::GREEN)
        );
        assert!(texture.generate_mip_chain(0).is_empty());
        assert_eq!(texture.generate_mip_chain(1).len(), 1);
        assert_eq!(texture.generate_mip_chain(3).len(), 3);

        // An odd size keeps at least one texel per axis.
        let odd = Texture::from_image(Image::new(3, 1, Color8::RED));
        let chain = odd.generate_mip_chain(4);
        let sizes: Vec<(u32, u32)> = chain.iter().map(|t| (t.width(), t.height())).collect();
        assert_eq!(sizes, [(3, 1), (1, 1)]);
    }

    #[test]
    fn mip_preserves_the_wrap_mode_and_empty_textures_are_safe() {
        let texture =
            Texture::from_image(Image::new(4, 4, Color8::WHITE)).with_wrap(WrapMode::Repeat);
        assert_eq!(texture.generate_mip().wrap(), WrapMode::Repeat);
        let empty = Texture::transparent(0, 0);
        assert_eq!(
            (empty.generate_mip().width(), empty.generate_mip().height()),
            (1, 1)
        );
        assert_eq!(
            empty.generate_mip().sample_nearest(0.5, 0.5),
            Color8::TRANSPARENT
        );
        assert_eq!(empty.uv_rect().width(), 1.0);
        assert_eq!(empty.clone().into_image().pixel_count(), 0);
        assert_eq!(empty.image().width, 0);
        assert_eq!(empty.height(), 0);
    }
}
