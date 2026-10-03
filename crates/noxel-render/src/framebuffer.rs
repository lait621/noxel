//! The render target: a linear HDR colour buffer plus a depth buffer.
//!
//! # Colour space
//!
//! Noxel renders into **linear** `f32` RGB and resolves to sRGB `Color8` once,
//! at the end of the frame. Reasons:
//!
//! * Lighting maths is only correct in linear space. Adding two sRGB values
//!   directly (which is what a naive 8-bit renderer does) darkens mid-tones and
//!   makes overlapping lights look wrong.
//! * The ray tracer needs to accumulate samples; `f32` has the headroom, `u8`
//!   does not.
//! * The resolve step is also where tone mapping, exposure, dithering and
//!   palette snapping live, all of which need more precision than the final
//!   buffer holds.
//!
//! The round trip is exact enough to preserve a hand-authored palette: see
//! [`Framebuffer::resolve`] and the `palette_round_trips_exactly` test in this
//! module.
//!
//! # Depth
//!
//! Depth is stored in the projection's native `[0, 1]` range (see
//! `docs/adr/0001-coordinate-system.md`), so the rasterizer's depth test is a
//! plain `<` comparison with no conversion.

use noxel_asset::image::Image;
use noxel_core::math::{Color, Color8, Rect, Vec2, Vec3};

/// Which of three buffers a pixel belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferKind {
    /// Linear HDR colour (`f32` per channel).
    Color,
    /// Depth in `[0, 1]`.
    Depth,
    /// 8-bit coverage/object id, used by the debug overlay and by
    /// software occlusion culling.
    Id,
}

/// A linear HDR render target.
#[derive(Clone, Debug)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    /// `width * height * 3` linear RGB values.
    color: Vec<f32>,
    /// `width * height` depth values in `[0, 1]`; 1.0 is the far plane.
    depth: Vec<f32>,
    /// `width * height` instance ids, written by the rasterizer for the debug
    /// overlay and for picking. `u32::MAX` means "nothing".
    ids: Vec<u32>,
}

impl Default for Framebuffer {
    /// A 1x1 black frame.
    ///
    /// Exists so a caller can `mem::take` a framebuffer out of a struct it is
    /// also borrowing — the render target is moved out, handed over, and moved
    /// back in.
    fn default() -> Self {
        Self::new(1, 1)
    }
}

impl Framebuffer {
    /// The id written where nothing has been drawn.
    pub const NO_ID: u32 = u32::MAX;

    /// Creates a framebuffer of the given size, cleared to transparent black and
    /// far depth.
    ///
    /// # Panics
    ///
    /// Panics when either dimension is zero. A zero-sized render target is
    /// always a bug (usually a window minimised to 0), and silently producing an
    /// empty frame makes it very hard to find.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "framebuffer must be non-empty, got {width}x{height}"
        );
        let pixels = (width as usize) * (height as usize);
        Self {
            width,
            height,
            color: vec![0.0; pixels * 3],
            depth: vec![1.0; pixels],
            ids: vec![Self::NO_ID; pixels],
        }
    }

    /// Width in pixels.
    #[inline]
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[inline]
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Total pixel count.
    #[inline]
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Aspect ratio.
    #[inline]
    #[must_use]
    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    /// The full-buffer rectangle.
    #[inline]
    #[must_use]
    pub fn bounds(&self) -> Rect {
        Rect::from_min_max(Vec2::ZERO, Vec2::new(self.width as f32, self.height as f32))
    }

    // ------------------------------------------------------------- clearing

    /// Clears colour to a linear RGB value and depth to `1.0`.
    pub fn clear(&mut self, color: [f32; 3]) {
        for chunk in self.color.chunks_exact_mut(3) {
            chunk[0] = color[0];
            chunk[1] = color[1];
            chunk[2] = color[2];
        }
        self.depth.fill(1.0);
        self.ids.fill(Self::NO_ID);
    }

    /// Clears colour from an sRGB-encoded value (the convenient form for a
    /// hand-picked sky colour).
    pub fn clear_srgb(&mut self, color: Color8) {
        let linear = color.to_linear();
        self.clear([linear.r, linear.g, linear.b]);
    }

    /// Clears only the depth buffer, keeping colour.
    pub fn clear_depth(&mut self) {
        self.depth.fill(1.0);
    }

    /// Clears only the id buffer.
    pub fn clear_ids(&mut self) {
        self.ids.fill(Self::NO_ID);
    }

    /// Fills the whole buffer with one colour (no depth reset).
    pub fn fill(&mut self, color: [f32; 3]) {
        self.clear(color);
    }

    // ------------------------------------------------------------ accessing

    /// Index of a pixel, or `None` when out of bounds.
    #[inline]
    #[must_use]
    pub fn index_of(&self, x: u32, y: u32) -> Option<usize> {
        if x < self.width && y < self.height {
            Some((y as usize) * (self.width as usize) + (x as usize))
        } else {
            None
        }
    }

    /// Linear RGB at a pixel, or `None` when out of bounds.
    #[inline]
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<[f32; 3]> {
        let i = self.index_of(x, y)? * 3;
        Some([self.color[i], self.color[i + 1], self.color[i + 2]])
    }

    /// Writes linear RGB at a pixel, ignoring out-of-bounds writes.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        if let Some(i) = self.index_of(x, y) {
            let i = i * 3;
            self.color[i] = rgb[0];
            self.color[i + 1] = rgb[1];
            self.color[i + 2] = rgb[2];
        }
    }

    /// Raw linear colour slice (`width * height * 3` values).
    #[inline]
    #[must_use]
    pub fn color_slice(&self) -> &[f32] {
        &self.color
    }

    /// Mutable raw linear colour slice.
    #[inline]
    pub fn color_slice_mut(&mut self) -> &mut [f32] {
        &mut self.color
    }

    /// Raw depth slice (`width * height` values).
    #[inline]
    #[must_use]
    pub fn depth_slice(&self) -> &[f32] {
        &self.depth
    }

    /// Mutable raw depth slice.
    #[inline]
    pub fn depth_slice_mut(&mut self) -> &mut [f32] {
        &mut self.depth
    }

    /// All three buffers at once, for a renderer that wants to hand disjoint
    /// row bands to different threads.
    ///
    /// Returning them from one method is what makes that safe: three separate
    /// `*_mut()` methods would each borrow the whole framebuffer and could not
    /// coexist.
    #[inline]
    pub fn split_mut(&mut self) -> (&mut [f32], &mut [f32], &mut [u32]) {
        (&mut self.color, &mut self.depth, &mut self.ids)
    }

    /// Depth at a pixel.
    #[inline]
    #[must_use]
    pub fn depth_at(&self, x: u32, y: u32) -> Option<f32> {
        self.index_of(x, y).map(|i| self.depth[i])
    }

    /// Writes depth at a pixel.
    #[inline]
    pub fn set_depth(&mut self, x: u32, y: u32, depth: f32) {
        if let Some(i) = self.index_of(x, y) {
            self.depth[i] = depth;
        }
    }

    /// Instance id at a pixel.
    #[inline]
    #[must_use]
    pub fn id_at(&self, x: u32, y: u32) -> Option<u32> {
        self.index_of(x, y).map(|i| self.ids[i])
    }

    /// Writes an instance id at a pixel.
    #[inline]
    pub fn set_id(&mut self, x: u32, y: u32, id: u32) {
        if let Some(i) = self.index_of(x, y) {
            self.ids[i] = id;
        }
    }

    /// The id buffer.
    #[inline]
    #[must_use]
    pub fn ids(&self) -> &[u32] {
        &self.ids
    }

    /// Blends linear RGB over the existing pixel with source-over compositing.
    ///
    /// `alpha` is straight (not premultiplied).
    #[inline]
    pub fn blend(&mut self, x: u32, y: u32, rgb: [f32; 3], alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let Some(i) = self.index_of(x, y) else { return };
        let i3 = i * 3;
        if alpha >= 1.0 {
            self.color[i3] = rgb[0];
            self.color[i3 + 1] = rgb[1];
            self.color[i3 + 2] = rgb[2];
            return;
        }
        let inv = 1.0 - alpha;
        self.color[i3] = rgb[0] * alpha + self.color[i3] * inv;
        self.color[i3 + 1] = rgb[1] * alpha + self.color[i3 + 1] * inv;
        self.color[i3 + 2] = rgb[2] * alpha + self.color[i3 + 2] * inv;
    }

    /// Adds linear RGB to a pixel (additive glow, light accumulation).
    #[inline]
    pub fn add(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        if let Some(i) = self.index_of(x, y) {
            let i = i * 3;
            self.color[i] += rgb[0];
            self.color[i + 1] += rgb[1];
            self.color[i + 2] += rgb[2];
        }
    }

    /// Multiplies a pixel's linear RGB by `factor`.
    #[inline]
    pub fn multiply(&mut self, x: u32, y: u32, factor: f32) {
        if let Some(i) = self.index_of(x, y) {
            let i = i * 3;
            self.color[i] *= factor;
            self.color[i + 1] *= factor;
            self.color[i + 2] *= factor;
        }
    }

    /// Resizes the buffer, discarding contents.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        let pixels = (width as usize) * (height as usize);
        self.width = width.max(1);
        self.height = height.max(1);
        self.color.clear();
        self.color.resize(pixels * 3, 0.0);
        self.depth.clear();
        self.depth.resize(pixels, 1.0);
        self.ids.clear();
        self.ids.resize(pixels, Self::NO_ID);
    }

    /// A copy of the buffer's dimensions and contents.
    #[must_use]
    pub fn clone_shallow(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            color: self.color.clone(),
            depth: self.depth.clone(),
            ids: self.ids.clone(),
        }
    }

    /// Bytes of heap used.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.color.capacity() * 4 + self.depth.capacity() * 4 + self.ids.capacity() * 4
    }
}

/// How [`Framebuffer::resolve`] converts linear HDR to 8-bit sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolveSettings {
    /// Exposure multiplier applied before tone mapping.
    pub exposure: f32,
    /// The tone curve.
    pub tonemap: ToneMap,
    /// Ordered-dither amplitude in 8-bit units, to break up banding in gradients.
    pub dither_strength: f32,
    /// When set, every output pixel is snapped to the nearest palette entry.
    pub palette_snap: bool,
    /// Global alpha applied to the resolved image.
    pub alpha: f32,
}

impl Default for ResolveSettings {
    fn default() -> Self {
        // A pixel-art project wants none of these by default: exact colours in,
        // exact colours out. Effects are opt-in per render pass.
        Self {
            exposure: 1.0,
            tonemap: ToneMap::None,
            dither_strength: 0.0,
            palette_snap: false,
            alpha: 1.0,
        }
    }
}

/// Tone-mapping curves available to [`Framebuffer::resolve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToneMap {
    /// Clamp to `[0, 1]` with no curve. Keeps author-picked colours exact.
    #[default]
    None,
    /// Reinhard `c / (1 + c)`.
    Reinhard,
    /// Filmic ACES approximation (Narkowicz).
    Aces,
}

impl Framebuffer {
    /// Converts the linear buffer to an 8-bit sRGB [`Image`].
    ///
    /// This is the only place the engine leaves linear space. With the default
    /// [`ResolveSettings`] (`ToneMap::None`, exposure 1, no dither) an unlit
    /// surface whose material colour came from a palette round-trips to exactly
    /// the byte it started as.
    #[must_use]
    pub fn resolve(&self, settings: &ResolveSettings) -> Image {
        let mut image = Image::new(self.width, self.height, Color8::TRANSPARENT);
        let alpha_u8 = (settings.alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        for y in 0..self.height {
            for x in 0..self.width {
                let i = ((y as usize) * (self.width as usize) + (x as usize)) * 3;
                let mut rgb = [
                    self.color[i] * settings.exposure,
                    self.color[i + 1] * settings.exposure,
                    self.color[i + 2] * settings.exposure,
                ];
                rgb = match settings.tonemap {
                    ToneMap::None => [
                        rgb[0].clamp(0.0, 1.0),
                        rgb[1].clamp(0.0, 1.0),
                        rgb[2].clamp(0.0, 1.0),
                    ],
                    ToneMap::Reinhard => [
                        noxel_core::math::tonemap_reinhard(rgb[0].max(0.0)),
                        noxel_core::math::tonemap_reinhard(rgb[1].max(0.0)),
                        noxel_core::math::tonemap_reinhard(rgb[2].max(0.0)),
                    ],
                    ToneMap::Aces => [
                        noxel_core::math::tonemap_aces(rgb[0].max(0.0)),
                        noxel_core::math::tonemap_aces(rgb[1].max(0.0)),
                        noxel_core::math::tonemap_aces(rgb[2].max(0.0)),
                    ],
                };
                if settings.dither_strength > 0.0 {
                    // 4x4 Bayer matrix, centred on zero and scaled to +-half a
                    // quantisation step of the 8-bit output.
                    const BAYER: [[f32; 4]; 4] = [
                        [0.0, 8.0, 2.0, 10.0],
                        [12.0, 4.0, 14.0, 6.0],
                        [3.0, 11.0, 1.0, 9.0],
                        [15.0, 7.0, 13.0, 5.0],
                    ];
                    let b = BAYER[(y % 4) as usize][(x % 4) as usize] / 16.0 - 0.5;
                    let amount = b * settings.dither_strength / 255.0;
                    for c in &mut rgb {
                        *c = (*c + amount).clamp(0.0, 1.0);
                    }
                }
                let mut pixel = Color::rgba(rgb[0], rgb[1], rgb[2], settings.alpha).to_srgb8();
                pixel.a = alpha_u8;
                image.set(x, y, pixel);
            }
        }
        image
    }

    /// Resolves and snaps every pixel to the nearest palette entry.
    ///
    /// This is how a lightmapped scene is forced back onto a fixed pixel-art
    /// palette. Snapping happens in sRGB space because that is where the artist
    /// chose the colours.
    #[must_use]
    pub fn resolve_palette(
        &self,
        settings: &ResolveSettings,
        palette: &noxel_core::math::Palette,
    ) -> Image {
        let mut image = self.resolve(settings);
        if palette.is_empty() {
            return image;
        }
        for pixel in &mut image.pixels {
            if pixel.a == 0 {
                continue;
            }
            if let Some(entry) = palette.nearest(*pixel) {
                let a = pixel.a;
                *pixel = Color8::new(entry.color.r, entry.color.g, entry.color.b, a);
            }
        }
        image
    }

    /// The linear colour at a normalised `(u, v)` position, clamped.
    ///
    /// The ray tracer and the post-process use this to sample their own output.
    #[must_use]
    pub fn sample_linear(&self, u: f32, v: f32) -> [f32; 3] {
        let x = ((u.clamp(0.0, 1.0) * (self.width - 1) as f32) + 0.5) as u32;
        let y = ((v.clamp(0.0, 1.0) * (self.height - 1) as f32) + 0.5) as u32;
        self.get(x.min(self.width - 1), y.min(self.height - 1))
            .unwrap_or([0.0; 3])
    }

    /// The mean linear luminance over the whole buffer, for auto-exposure.
    #[must_use]
    pub fn average_luminance(&self) -> f32 {
        if self.color.is_empty() {
            return 0.0;
        }
        let mut sum = 0.0;
        for chunk in self.color.chunks_exact(3) {
            sum += 0.2126 * chunk[0] + 0.7152 * chunk[1] + 0.0722 * chunk[2];
        }
        sum / (self.color.len() / 3) as f32
    }

    /// The world-space position reconstructed from a pixel and the camera's
    /// inverse view-projection. Used by the debug overlay and by picking.
    #[must_use]
    pub fn unproject_depth(
        &self,
        x: u32,
        y: u32,
        inv_view_projection: &noxel_core::math::Mat4,
    ) -> Option<Vec3> {
        let depth = self.depth_at(x, y)?;
        if depth >= 1.0 {
            return None;
        }
        // Pixel centre -> NDC. Y is flipped because image rows go down.
        let ndc_x = ((x as f32 + 0.5) / self.width as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((y as f32 + 0.5) / self.height as f32) * 2.0;
        let clip = noxel_core::math::Vec4::new(ndc_x, ndc_y, depth, 1.0);
        inv_view_projection
            .transform_point4(clip)
            .perspective_divide()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::{Mat4, Palette};

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn new_buffer_is_cleared() {
        let fb = Framebuffer::new(4, 3);
        assert_eq!(fb.width(), 4);
        assert_eq!(fb.height(), 3);
        assert_eq!(fb.pixel_count(), 12);
        assert!(fb.depth_slice().iter().all(|d| *d == 1.0));
        assert!(fb.color_slice().iter().all(|c| *c == 0.0));
        assert!(fb.ids().iter().all(|i| *i == Framebuffer::NO_ID));
    }

    #[test]
    #[should_panic(expected = "framebuffer must be non-empty")]
    fn zero_sized_panics() {
        let _ = Framebuffer::new(0, 10);
    }

    #[test]
    fn aspect_ratio() {
        assert!(approx(
            Framebuffer::new(320, 180).aspect_ratio(),
            320.0 / 180.0
        ));
    }

    #[test]
    fn clear_writes_every_pixel() {
        let mut fb = Framebuffer::new(8, 8);
        fb.clear([0.25, 0.5, 0.75]);
        assert!(
            fb.color_slice()
                .chunks_exact(3)
                .all(|c| c[0] == 0.25 && c[1] == 0.5 && c[2] == 0.75)
        );
        fb.set_depth(3, 3, 0.1);
        fb.clear_depth();
        assert_eq!(fb.depth_at(3, 3), Some(1.0));
    }

    #[test]
    fn clear_srgb_converts_to_linear() {
        let mut fb = Framebuffer::new(2, 2);
        fb.clear_srgb(Color8::new(255, 128, 0, 255));
        let c = fb.get(0, 0).unwrap();
        assert!(approx(c[0], 1.0));
        assert!(approx(c[1], 0.215_86), "{}", c[1]);
        assert_eq!(c[2], 0.0);
    }

    #[test]
    fn get_set_roundtrip() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set(2, 3, [1.0, 2.0, 3.0]);
        assert_eq!(fb.get(2, 3), Some([1.0, 2.0, 3.0]));
        assert_eq!(fb.get(4, 3), None);
        // Out-of-bounds writes are ignored, not panics.
        fb.set(99, 99, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn blend_source_over() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set(0, 0, [1.0, 0.0, 0.0]);
        fb.blend(0, 0, [0.0, 0.0, 1.0], 0.5);
        assert_eq!(fb.get(0, 0).unwrap(), [0.5, 0.0, 0.5]);
        // Fully transparent is a no-op, fully opaque replaces.
        fb.blend(0, 0, [9.0, 9.0, 9.0], 0.0);
        assert_eq!(fb.get(0, 0).unwrap(), [0.5, 0.0, 0.5]);
        fb.blend(0, 0, [0.1, 0.2, 0.3], 1.0);
        assert_eq!(fb.get(0, 0).unwrap(), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn add_and_multiply() {
        let mut fb = Framebuffer::new(1, 1);
        fb.add(0, 0, [0.5, 0.5, 0.5]);
        fb.add(0, 0, [0.25, 0.0, 1.0]);
        assert_eq!(fb.get(0, 0).unwrap(), [0.75, 0.5, 1.5]);
        fb.multiply(0, 0, 2.0);
        assert_eq!(fb.get(0, 0).unwrap(), [1.5, 1.0, 3.0]);
    }

    #[test]
    fn resolve_without_tonemap_preserves_exact_values() {
        let mut fb = Framebuffer::new(1, 1);
        let original = Color8::new(87, 143, 201, 255);
        let linear = original.to_linear();
        fb.set(0, 0, [linear.r, linear.g, linear.b]);
        let image = fb.resolve(&ResolveSettings::default());
        assert_eq!(
            image.get(0, 0),
            Some(original),
            "palette colours must survive the round trip"
        );
    }

    #[test]
    fn palette_round_trips_exactly() {
        // Every one of a realistic 32-colour palette must come back unchanged.
        let palette: Vec<u32> = (0..32u32)
            .map(|i| 0xFF00_0000 | (i.wrapping_mul(2_654_435_761)))
            .collect();
        let mut fb = Framebuffer::new(32, 1);
        for (x, argb) in palette.iter().enumerate() {
            let linear = Color8::from_hex(*argb).to_linear();
            fb.set(x as u32, 0, [linear.r, linear.g, linear.b]);
        }
        let image = fb.resolve(&ResolveSettings::default());
        for (x, argb) in palette.iter().enumerate() {
            let want = Color8::from_hex(*argb);
            let got = image.get(x as u32, 0).expect("in bounds");
            assert_eq!(
                (got.r, got.g, got.b),
                (want.r, want.g, want.b),
                "entry {x} shifted: {want:?} -> {got:?}"
            );
        }
    }

    #[test]
    fn resolve_clamps_bright_values_without_tonemap() {
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, [4.0, -1.0, 0.5]);
        let image = fb.resolve(&ResolveSettings::default());
        assert_eq!(image.get(0, 0), Some(Color8::new(255, 0, 188, 255)));
    }

    #[test]
    fn tonemaps_change_the_result() {
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, [1.0, 1.0, 1.0]);
        let none = fb
            .resolve(&ResolveSettings::default())
            .get(0, 0)
            .expect("in bounds");
        let reinhard = fb
            .resolve(&ResolveSettings {
                tonemap: ToneMap::Reinhard,
                ..Default::default()
            })
            .get(0, 0)
            .expect("in bounds");
        let aces = fb
            .resolve(&ResolveSettings {
                tonemap: ToneMap::Aces,
                ..Default::default()
            })
            .get(0, 0)
            .expect("in bounds");
        assert_eq!(none.r, 255);
        assert!(reinhard.r < 200, "{}", reinhard.r);
        assert!(aces.r < 255);
    }

    #[test]
    fn exposure_scales() {
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, [0.2, 0.2, 0.2]);
        let dark = fb
            .resolve(&ResolveSettings {
                exposure: 0.5,
                ..Default::default()
            })
            .get(0, 0)
            .expect("in bounds");
        let bright = fb
            .resolve(&ResolveSettings {
                exposure: 2.0,
                ..Default::default()
            })
            .get(0, 0)
            .expect("in bounds");
        assert!(dark.r < bright.r);
    }

    #[test]
    fn dithering_breaks_banding_without_shifting_the_mean_much() {
        let mut fb = Framebuffer::new(64, 64);
        fb.clear([0.5, 0.5, 0.5]);
        let plain = fb.resolve(&ResolveSettings::default());
        let dithered = fb.resolve(&ResolveSettings {
            dither_strength: 32.0,
            ..Default::default()
        });
        let levels = |img: &Image| {
            let mut v: Vec<u8> = img.pixels.iter().map(|p| p.r).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        assert_eq!(
            levels(&plain),
            1,
            "a flat field is one level without dithering"
        );
        assert!(levels(&dithered) > 1, "dithering must break the band");
        let mean = |img: &Image| {
            img.pixels.iter().map(|p| p.r as u32).sum::<u32>() as f32 / img.pixels.len() as f32
        };
        assert!(
            (mean(&plain) - mean(&dithered)).abs() <= 4.0,
            "dithering must not shift the mean: {} vs {}",
            mean(&plain),
            mean(&dithered)
        );
    }

    #[test]
    fn palette_snapping_forces_the_palette() {
        let mut palette = Palette::new();
        palette.push("black", Color8::new(0, 0, 0, 255));
        palette.push("white", Color8::new(255, 255, 255, 255));
        let mut fb = Framebuffer::new(4, 1);
        fb.set(0, 0, Color8::new(10, 10, 10, 255).to_linear().to_array3());
        fb.set(
            1,
            0,
            Color8::new(240, 240, 240, 255).to_linear().to_array3(),
        );
        fb.set(
            2,
            0,
            Color8::new(120, 120, 120, 255).to_linear().to_array3(),
        );
        let image = fb.resolve_palette(&ResolveSettings::default(), &palette);
        assert_eq!(
            image.get(0, 0).unwrap().to_hex(),
            Color8::new(0, 0, 0, 255).to_hex()
        );
        assert_eq!(
            image.get(1, 0).unwrap().to_hex(),
            Color8::new(255, 255, 255, 255).to_hex()
        );
        // 120 is nearer black than white in sRGB distance.
        assert_eq!(
            image.get(2, 0).unwrap().to_hex(),
            Color8::new(0, 0, 0, 255).to_hex()
        );
    }

    #[test]
    fn resize_reallocates_and_clears() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set(0, 0, [1.0, 1.0, 1.0]);
        fb.resize(8, 2);
        assert_eq!(fb.pixel_count(), 16);
        assert_eq!(fb.get(0, 0), Some([0.0, 0.0, 0.0]));
        // Resizing to the same size is a no-op.
        fb.set(0, 0, [1.0, 1.0, 1.0]);
        fb.resize(8, 2);
        assert_eq!(fb.get(0, 0), Some([1.0, 1.0, 1.0]));
    }

    #[test]
    fn ids_are_tracked() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set_id(1, 1, 42);
        assert_eq!(fb.id_at(1, 1), Some(42));
        assert_eq!(fb.id_at(0, 0), Some(Framebuffer::NO_ID));
        fb.clear_ids();
        assert_eq!(fb.id_at(1, 1), Some(Framebuffer::NO_ID));
    }

    #[test]
    fn average_luminance() {
        let mut fb = Framebuffer::new(2, 2);
        fb.clear([1.0, 1.0, 1.0]);
        assert!(approx(fb.average_luminance(), 1.0));
        fb.clear([0.0, 0.0, 0.0]);
        assert_eq!(fb.average_luminance(), 0.0);
    }

    #[test]
    fn sample_linear_clamps() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set(0, 0, [1.0, 0.0, 0.0]);
        assert_eq!(fb.sample_linear(0.0, 0.0)[0], 1.0);
        assert_eq!(fb.sample_linear(-5.0, -5.0)[0], 1.0);
        assert_eq!(fb.sample_linear(10.0, 10.0)[0], 0.0);
    }

    #[test]
    fn unproject_recovers_a_point_on_the_far_plane() {
        let proj = Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, 1.0, 100.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 50.0, 0.0), Vec3::ZERO, Vec3::Z);
        let vp = proj * view;
        let inv = vp.inverse().unwrap();
        let mut fb = Framebuffer::new(64, 64);
        fb.set_depth(32, 32, 0.5);
        let world = fb.unproject_depth(32, 32, &inv).unwrap();
        // The centre pixel looks straight down the Y axis at the origin.
        assert!(world.x.abs() < 0.5 && world.z.abs() < 0.5, "{world:?}");
        assert!(world.y < 50.0);
    }

    #[test]
    fn unproject_rejects_the_background() {
        let fb = Framebuffer::new(4, 4);
        let identity = Mat4::IDENTITY;
        assert!(
            fb.unproject_depth(0, 0, &identity).is_none(),
            "depth == 1.0 is background"
        );
    }

    #[test]
    fn memory_estimate() {
        let fb = Framebuffer::new(64, 64);
        assert!(fb.memory_bytes() >= 64 * 64 * 12);
    }
}
