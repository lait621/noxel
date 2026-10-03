//! Colour types.
//!
//! Noxel keeps two representations and is strict about which is which:
//!
//! * [`Color`] — **linear** `f32` RGBA. All lighting, blending of lit surfaces
//!   and ray-tracing maths happens here. Linear is not negotiable: blending
//!   sRGB values directly is the single most common cause of "muddy" pixel-art
//!   lighting.
//! * [`Color8`] — **sRGB** `u8` RGBA, exactly one pixel in a framebuffer.
//!
//! Conversion happens at the renderer's boundary (when sampling a texture, and
//! once at the end of the frame after tone mapping). See
//! `docs/adr/0005-color-management.md`.

use super::scalar::{linear_to_srgb, srgb_to_linear};
use super::vec::Vec4;

/// A **linear-space** RGBA colour with `f32` components.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Color {
    /// Red, linear, nominally `[0, 1]` but unbounded above for HDR.
    pub r: f32,
    /// Green, linear.
    pub g: f32,
    /// Blue, linear.
    pub b: f32,
    /// Alpha, `[0, 1]`, *not* gamma-encoded.
    pub a: f32,
}

impl Color {
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    /// Fully transparent black; the identity for additive blending.
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    /// Opaque mid grey (linear 0.5).
    pub const GREY: Self = Self::rgb(0.5, 0.5, 0.5);
    /// Opaque red.
    pub const RED: Self = Self::rgb(1.0, 0.0, 0.0);
    /// Opaque green.
    pub const GREEN: Self = Self::rgb(0.0, 1.0, 0.0);
    /// Opaque blue.
    pub const BLUE: Self = Self::rgb(0.0, 0.0, 1.0);
    /// Opaque yellow.
    pub const YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);

    /// Fully opaque colour from linear RGB.
    #[inline]
    #[must_use]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Colour from linear RGBA.
    #[inline]
    #[must_use]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Every component set to `v` in linear space (grey).
    #[inline]
    #[must_use]
    pub const fn grey(v: f32) -> Self {
        Self::rgb(v, v, v)
    }

    /// Builds a linear colour from an 8-bit **sRGB** triple plus alpha.
    #[inline]
    #[must_use]
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::rgba(
            srgb_to_linear(r as f32 / 255.0),
            srgb_to_linear(g as f32 / 255.0),
            srgb_to_linear(b as f32 / 255.0),
            a as f32 / 255.0,
        )
    }

    /// Builds a linear colour from a packed `0xAARRGGBB` sRGB value.
    #[inline]
    #[must_use]
    pub fn from_hex(argb: u32) -> Self {
        Self::from_srgb8(
            ((argb >> 16) & 0xFF) as u8,
            ((argb >> 8) & 0xFF) as u8,
            (argb & 0xFF) as u8,
            ((argb >> 24) & 0xFF) as u8,
        )
    }

    /// Scales RGB, leaving alpha alone. Colour-modulating a sprite or light.
    #[inline]
    #[must_use]
    pub fn tint(self, factor: f32) -> Self {
        Self {
            r: self.r * factor,
            g: self.g * factor,
            b: self.b * factor,
            a: self.a,
        }
    }

    /// Multiplies two colours component-wise (the usual modulation operator).
    #[inline]
    #[must_use]
    pub fn modulate(self, other: Self) -> Self {
        Self {
            r: self.r * other.r,
            g: self.g * other.g,
            b: self.b * other.b,
            a: self.a * other.a,
        }
    }

    /// Horizontal linear interpolation toward `other`.
    #[inline]
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Relative luminance in linear space (Rec. 709 weights).
    #[inline]
    #[must_use]
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Clamps to `[0, 1]` and converts back to an 8-bit sRGB pixel.
    #[inline]
    #[must_use]
    pub fn to_srgb8(self) -> Color8 {
        Color8::new(
            (linear_to_srgb(self.r.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.g.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.b.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (self.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        )
    }

    /// Clamps every component (including alpha) into `[0, 1]`.
    #[inline]
    #[must_use]
    pub fn saturate(self) -> Self {
        Self {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
            a: self.a.clamp(0.0, 1.0),
        }
    }

    /// Packs to the `0xAARRGGBB` layout used by every pixel-art asset format.
    #[inline]
    #[must_use]
    pub fn to_hex(self) -> u32 {
        let c = self.to_srgb8();
        ((c.a as u32) << 24) | ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)
    }

    /// The linear RGB components as a plain array.
    ///
    /// The renderer stores its framebuffer as tightly packed `f32` triples, so
    /// this is the conversion used on that boundary.
    #[inline]
    #[must_use]
    pub const fn to_array3(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }

    /// Builds a colour from a plain linear RGB array (alpha becomes 1).
    #[inline]
    #[must_use]
    pub const fn from_array3(a: [f32; 3]) -> Self {
        Self::rgb(a[0], a[1], a[2])
    }

    /// Converts to a `Vec4` (`rgb`/`a`) for uniform upload.
    #[inline]
    #[must_use]
    pub const fn to_vec4(self) -> Vec4 {
        Vec4::new(self.r, self.g, self.b, self.a)
    }

    /// Builds from a `Vec4` interpreted as linear RGBA.
    #[inline]
    #[must_use]
    pub const fn from_vec4(v: Vec4) -> Self {
        Self {
            r: v.x,
            g: v.y,
            b: v.z,
            a: v.w,
        }
    }

    /// Source-over alpha compositing in linear space: `self` over `dst`.
    ///
    /// Both inputs and the result are premultiplied-free straight colours.
    #[inline]
    #[must_use]
    pub fn over(self, dst: Self) -> Self {
        let out_a = self.a + dst.a * (1.0 - self.a);
        if out_a <= 0.0 {
            return Self::TRANSPARENT;
        }
        let inv = 1.0 / out_a;
        Self {
            r: (self.r * self.a + dst.r * dst.a * (1.0 - self.a)) * inv,
            g: (self.g * self.a + dst.g * dst.a * (1.0 - self.a)) * inv,
            b: (self.b * self.a + dst.b * dst.a * (1.0 - self.a)) * inv,
            a: out_a,
        }
    }

    /// Returns the same linear colour with a replacement alpha value (fade-out
    /// used by the occlusion system when a roof or tree fades in front of the
    /// player).
    #[inline]
    #[must_use]
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

impl From<Color8> for Color {
    #[inline]
    fn from(c: Color8) -> Self {
        c.to_linear()
    }
}

/// A single **sRGB** 8-bit RGBA pixel, as stored in a framebuffer or texture.
///
/// Field order is `r, g, b, a`, matching the byte order of a PNG and of a wgpu
/// `Rgba8UnormSrgb` texture, so a framebuffer can be uploaded or written out
/// with no per-pixel shuffling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Color8 {
    /// Red, sRGB encoded.
    pub r: u8,
    /// Green, sRGB encoded.
    pub g: u8,
    /// Blue, sRGB encoded.
    pub b: u8,
    /// Alpha, linear (alpha is never gamma-encoded).
    pub a: u8,
}

impl Color8 {
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Self = Self::new(0, 0, 0, 255);
    /// Opaque white.
    pub const WHITE: Self = Self::new(255, 255, 255, 255);
    /// Opaque red.
    pub const RED: Self = Self::new(255, 0, 0, 255);
    /// Opaque green.
    pub const GREEN: Self = Self::new(0, 255, 0, 255);
    /// Opaque blue.
    pub const BLUE: Self = Self::new(0, 0, 255, 255);
    /// Opaque magenta: the conventional "missing texture" colour.
    pub const MAGENTA: Self = Self::new(255, 0, 255, 255);
    /// Opaque mid grey.
    pub const GREY: Self = Self::new(128, 128, 128, 255);

    /// Constructs a pixel.
    #[inline]
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Constructs an opaque pixel.
    #[inline]
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// Builds from a packed `0xAARRGGBB` value.
    #[inline]
    #[must_use]
    pub const fn from_hex(argb: u32) -> Self {
        Self::new(
            ((argb >> 16) & 0xFF) as u8,
            ((argb >> 8) & 0xFF) as u8,
            (argb & 0xFF) as u8,
            ((argb >> 24) & 0xFF) as u8,
        )
    }

    /// Converts to a packed `0xAARRGGBB` value.
    #[inline]
    #[must_use]
    pub const fn to_hex(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Converts to a linear-space [`Color`].
    #[inline]
    #[must_use]
    pub fn to_linear(self) -> Color {
        Color::from_srgb8(self.r, self.g, self.b, self.a)
    }

    /// Builds from a linear-space [`Color`], clamping and encoding.
    #[inline]
    #[must_use]
    pub fn from_linear(c: Color) -> Self {
        c.to_srgb8()
    }

    /// True when the pixel is fully transparent.
    #[inline]
    #[must_use]
    pub const fn is_transparent(self) -> bool {
        self.a == 0
    }

    /// The four bytes in memory order.
    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Builds from four bytes in memory order.
    #[inline]
    #[must_use]
    pub const fn from_array(a: [u8; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3])
    }

    /// Source-over compositing **in sRGB space**, for the rasterizer's 8-bit
    /// framebuffer.
    ///
    /// Blending 8-bit values without decoding is technically wrong, but it is
    /// what every pixel-art renderer does, and it is what preserves the crisp
    /// hand-picked palette of the source art. The engine's HDR path (see
    /// `noxel-render::post`) blends in linear space instead; pick one per
    /// material and stay consistent.
    #[inline]
    #[must_use]
    pub fn blend_over(self, dst: Self) -> Self {
        if self.a == 255 {
            return self;
        }
        if self.a == 0 {
            return dst;
        }
        let sa = self.a as u32;
        let da = dst.a as u32;
        let inv = 255 - sa;
        let out_a = sa + da * inv / 255;
        if out_a == 0 {
            return Self::TRANSPARENT;
        }
        let mix = |s: u8, d: u8| -> u8 {
            (((s as u32) * sa * 255 + (d as u32) * da * inv) / (out_a * 255)).min(255) as u8
        };
        Self::new(
            mix(self.r, dst.r),
            mix(self.g, dst.g),
            mix(self.b, dst.b),
            out_a as u8,
        )
    }

    /// Adds another pixel's RGB, saturating. Used by additive glow and light
    /// accumulation passes.
    #[inline]
    #[must_use]
    pub fn add_saturating(self, other: Self) -> Self {
        Self::new(
            self.r.saturating_add(other.r),
            self.g.saturating_add(other.g),
            self.b.saturating_add(other.b),
            self.a.max(other.a),
        )
    }

    /// Multiplies RGB by a `[0, 255]` scalar, keeping alpha.
    #[inline]
    #[must_use]
    pub fn scale_rgb(self, num: u32, den: u32) -> Self {
        let d = den.max(1);
        Self::new(
            ((self.r as u32 * num) / d).min(255) as u8,
            ((self.g as u32 * num) / d).min(255) as u8,
            ((self.b as u32 * num) / d).min(255) as u8,
            self.a,
        )
    }

    /// Linear interpolation between two pixels, component-wise.
    #[inline]
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let f = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t) as u8 };
        Self::new(
            f(self.r, other.r),
            f(self.g, other.g),
            f(self.b, other.b),
            f(self.a, other.a),
        )
    }
}

impl Default for Color8 {
    fn default() -> Self {
        Self::WHITE
    }
}

/// A named entry in a [`Palette`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    /// The colour.
    pub color: Color8,
    /// Human/AI-readable name, e.g. `"grass_dark"`. Never empty.
    pub name: &'static str,
}

/// A fixed-size indexed palette.
///
/// Pixel-art projects in Noxel author art against a named palette so the world
/// generator and the prefab tooling can refer to `"road_asphalt"` rather than a
/// magic RGB triple. The demo ships one in `assets/config/palette.json`; the
/// type here is the in-engine mirror that tools and tests share.
#[derive(Clone, Debug)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
}

impl Palette {
    /// An empty palette.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Adds an entry, returning its index.
    pub fn push(&mut self, name: &'static str, color: Color8) -> usize {
        self.entries.push(PaletteEntry { color, name });
        self.entries.len() - 1
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up by index.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Color8> {
        self.entries.get(index).map(|e| e.color)
    }

    /// Looks up by name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<Color8> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.color)
    }

    /// All entries.
    #[must_use]
    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    /// The nearest entry by squared sRGB distance.
    ///
    /// Used when an imported image (a hand-painted PNG) needs snapping back onto
    /// the project palette. Returns `None` for an empty palette.
    #[must_use]
    pub fn nearest(&self, color: Color8) -> Option<PaletteEntry> {
        self.entries.iter().copied().min_by_key(|e| {
            let dr = e.color.r as i32 - color.r as i32;
            let dg = e.color.g as i32 - color.g as i32;
            let db = e.color.b as i32 - color.b as i32;
            let da = e.color.a as i32 - color.a as i32;
            dr * dr + dg * dg + db * db + da * da
        })
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Color8::from_hex(0xFF11_2233);
        assert_eq!((c.r, c.g, c.b, c.a), (0x11, 0x22, 0x33, 0xFF));
        assert_eq!(c.to_hex(), 0xFF11_2233);
    }

    #[test]
    fn basic_colour_lookup() {
        let mut p = Palette::new();
        p.push("grass", Color8::rgb(34, 139, 34));
        p.push("road", Color8::rgb(60, 60, 64));
        assert_eq!(p.find("road"), Some(Color8::rgb(60, 60, 64)));
        assert_eq!(p.find("nope"), None);
        let near = p.nearest(Color8::rgb(62, 58, 66)).unwrap();
        assert_eq!(near.name, "road");
    }

    #[test]
    fn srgb8_and_linear_roundtrip() {
        for v in [0u8, 1, 64, 128, 200, 255] {
            let c = Color8::new(v, v, v, 255);
            let back = c.to_linear().to_srgb8();
            assert!((back.r as i32 - v as i32).abs() <= 1, "{} -> {}", v, back.r);
        }
    }

    #[test]
    fn source_over_is_identity_for_opaque() {
        let dst = Color::from_srgb8(10, 20, 30, 255);
        let out = Color::WHITE.over(dst);
        assert!((out.r - 1.0).abs() < 1e-5);
    }

    #[test]
    fn source_over_transparent_keeps_dst() {
        let dst = Color::from_srgb8(10, 20, 30, 255);
        let out = Color::TRANSPARENT.over(dst);
        assert!((out.r - dst.r).abs() < 1e-5 && (out.g - dst.g).abs() < 1e-5);
    }

    #[test]
    fn color8_blend_half_over_opaque() {
        let src = Color8::new(255, 255, 255, 128);
        let dst = Color8::new(0, 0, 0, 255);
        let out = src.blend_over(dst);
        assert!(out.r > 120 && out.r < 136, "{out:?}");
        assert_eq!(out.a, 255);
    }

    #[test]
    fn color8_blend_fully_transparent_is_noop() {
        let dst = Color8::new(9, 9, 9, 255);
        assert_eq!(Color8::TRANSPARENT.blend_over(dst), dst);
    }

    #[test]
    fn color8_blend_fully_opaque_replaces() {
        let src = Color8::new(9, 9, 9, 255);
        assert_eq!(src.blend_over(Color8::WHITE), src);
    }

    #[test]
    fn luminance_uses_709_weights() {
        assert!((Color::GREEN.luminance() - 0.7152).abs() < 1e-4);
    }

    #[test]
    fn palette_nearest_prefers_alpha_match_too() {
        let mut p = Palette::new();
        p.push("opaque", Color8::new(10, 10, 10, 255));
        p.push("ghost", Color8::new(10, 10, 10, 0));
        let n = p.nearest(Color8::new(11, 10, 10, 250)).unwrap();
        assert_eq!(n.name, "opaque");
    }
}
