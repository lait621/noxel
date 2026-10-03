//! Post-processing on the linear framebuffer.
//!
//! Everything here runs on the HDR buffer *before* the resolve to 8-bit sRGB, so
//! an effect can push values above 1.0 and let the tone curve bring them back.
//! Dithering is the exception: it belongs to quantisation and therefore lives in
//! [`crate::Framebuffer::resolve`] via
//! [`crate::ResolveSettings::dither_strength`].
//!
//! All effects are written to be **allocation-light**: each one borrows scratch
//! storage owned by the [`PostProcess`] instance rather than allocating per
//! frame.

use noxel_core::math::Color;
use noxel_core::time::Stopwatch;

use crate::framebuffer::Framebuffer;

/// Configuration for the post chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostSettings {
    /// Darken the corners. `0.0` disables.
    pub vignette: f32,
    /// Where the vignette starts to bite, as a fraction of the half-diagonal.
    pub vignette_radius: f32,
    /// Bright-pass threshold for bloom. `0.0` disables bloom.
    pub bloom_threshold: f32,
    /// How much blurred bright-pass is added back.
    pub bloom_intensity: f32,
    /// Blur radius in pixels for the bloom ping-pong.
    pub bloom_radius: u32,
    /// Multiply the whole image (before the resolve exposure).
    pub gain: f32,
    /// Add to the whole image.
    pub lift: f32,
    /// Desaturate towards luminance, `0` = untouched, `1` = greyscale.
    ///
    /// A tiny amount (0.05–0.15) is a cheap way to make a busy pixel-art scene
    /// read as one image without hand-editing the art.
    pub desaturate: f32,
    /// Apply a subtle horizontal scanline darkening, one line in `n` (`0` off).
    ///
    /// Off by default: it is a CRT pastiche, not a pixel-art requirement. Kept
    /// because the demo's "retro" preset uses it.
    pub scanline_period: u32,
    /// Amount the scanline darkens by, `0..=1`.
    pub scanline_strength: f32,
}

impl Default for PostSettings {
    fn default() -> Self {
        // Everything off: a pixel-art engine should not tint an artist's
        // palette unless asked to.
        Self {
            vignette: 0.0,
            vignette_radius: 0.6,
            bloom_threshold: 0.0,
            bloom_intensity: 0.0,
            bloom_radius: 4,
            gain: 1.0,
            lift: 0.0,
            desaturate: 0.0,
            scanline_period: 0,
            scanline_strength: 0.0,
        }
    }
}

impl PostSettings {
    /// A gentle preset for a lit 3D-looking scene.
    #[must_use]
    pub fn cinematic() -> Self {
        Self {
            vignette: 0.25,
            bloom_threshold: 1.0,
            bloom_intensity: 0.35,
            bloom_radius: 6,
            desaturate: 0.08,
            ..Self::default()
        }
    }

    /// A CRT pastiche: strong vignette, scanlines, slight bloom.
    #[must_use]
    pub fn retro() -> Self {
        Self {
            vignette: 0.4,
            vignette_radius: 0.5,
            bloom_threshold: 0.8,
            bloom_intensity: 0.25,
            bloom_radius: 4,
            scanline_period: 2,
            scanline_strength: 0.12,
            ..Self::default()
        }
    }

    /// True when the chain would do anything at all.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        (self.vignette - 0.0).abs() < 1e-6
            && (self.bloom_intensity - 0.0).abs() < 1e-6
            && (self.gain - 1.0).abs() < 1e-6
            && (self.lift - 0.0).abs() < 1e-6
            && (self.desaturate - 0.0).abs() < 1e-6
            && self.scanline_period == 0
    }
}

/// The post chain, holding the scratch buffers the bloom pass needs.
#[derive(Debug, Default)]
pub struct PostProcess {
    /// Bloom scratch, `width * height * 3` linear values.
    bright: Vec<f32>,
    /// Ping-pong scratch of the same size.
    blur: Vec<f32>,
    width: u32,
    height: u32,
}

impl PostProcess {
    /// Creates an empty post chain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensures the scratch buffers match the framebuffer.
    fn prepare(&mut self, fb: &Framebuffer) {
        if self.width == fb.width() && self.height == fb.height() {
            return;
        }
        self.width = fb.width();
        self.height = fb.height();
        let len = fb.pixel_count() * 3;
        self.bright.clear();
        self.bright.resize(len, 0.0);
        self.blur.clear();
        self.blur.resize(len, 0.0);
    }

    /// Applies the whole chain. Returns the milliseconds spent.
    pub fn apply(&mut self, fb: &mut Framebuffer, settings: &PostSettings) -> f32 {
        let watch = Stopwatch::start();
        if settings.is_identity() {
            return 0.0;
        }
        self.prepare(fb);
        self.apply_gain_lift(fb, settings);
        if settings.desaturate > 0.0 {
            self.apply_desaturate(fb, settings.desaturate);
        }
        if settings.bloom_intensity > 0.0 && settings.bloom_threshold >= 0.0 {
            self.apply_bloom(fb, settings);
        }
        if settings.vignette > 0.0 {
            self.apply_vignette(fb, settings.vignette, settings.vignette_radius);
        }
        if settings.scanline_period > 0 && settings.scanline_strength > 0.0 {
            self.apply_scanlines(fb, settings.scanline_period, settings.scanline_strength);
        }
        watch.elapsed_ms() as f32
    }

    /// `rgb = rgb * gain + lift`, clamped at zero.
    pub fn apply_gain_lift(&self, fb: &mut Framebuffer, settings: &PostSettings) {
        if (settings.gain - 1.0).abs() < 1e-6 && settings.lift.abs() < 1e-6 {
            return;
        }
        for c in fb.color_slice_mut() {
            *c = (*c * settings.gain + settings.lift).max(0.0);
        }
    }

    /// Blends every pixel towards its luminance.
    pub fn apply_desaturate(&self, fb: &mut Framebuffer, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        for chunk in fb.color_slice_mut().chunks_exact_mut(3) {
            let luma = 0.2126 * chunk[0] + 0.7152 * chunk[1] + 0.0722 * chunk[2];
            chunk[0] += (luma - chunk[0]) * amount;
            chunk[1] += (luma - chunk[1]) * amount;
            chunk[2] += (luma - chunk[2]) * amount;
        }
    }

    /// Darkens the corners with a smooth radial ramp.
    pub fn apply_vignette(&self, fb: &mut Framebuffer, strength: f32, radius: f32) {
        let (w, h) = (fb.width() as f32, fb.height() as f32);
        let (cx, cy) = (w * 0.5, h * 0.5);
        let max_distance = (cx * cx + cy * cy).sqrt();
        let radius = radius.clamp(0.0, 1.0);
        for y in 0..fb.height() {
            let dy = y as f32 + 0.5 - cy;
            for x in 0..fb.width() {
                let dx = x as f32 + 0.5 - cx;
                let d = (dx * dx + dy * dy).sqrt() / max_distance;
                if d <= radius {
                    continue;
                }
                let t = ((d - radius) / (1.0 - radius).max(1e-4)).clamp(0.0, 1.0);
                // Squared falloff reads as a lens rather than a linear ramp.
                // Clamped at zero: a strength above 1 must darken to black, not
                // produce negative radiance.
                let factor = (1.0 - strength * t * t).max(0.0);
                fb.multiply(x, y, factor);
            }
        }
    }

    /// Darkens every `period`th row.
    pub fn apply_scanlines(&self, fb: &mut Framebuffer, period: u32, strength: f32) {
        let period = period.max(2);
        let strength = strength.clamp(0.0, 1.0);
        let factor = 1.0 - strength;
        for y in (0..fb.height()).step_by(period as usize) {
            for x in 0..fb.width() {
                fb.multiply(x, y, factor);
            }
        }
    }

    /// Thresholds the bright parts of the image, blurs them and adds them back.
    pub fn apply_bloom(&mut self, fb: &mut Framebuffer, settings: &PostSettings) {
        // Size the scratch buffers here rather than relying on `apply`, so the
        // effect is safe to call on its own (which the tests do).
        self.prepare(fb);
        let width = fb.width();
        let height = fb.height();
        let color = fb.color_slice();
        self.bright.clear();
        self.bright.resize(color.len(), 0.0);
        let threshold = settings.bloom_threshold;
        // Bright pass: keep only what is above the threshold, scaled by how far
        // above it is, so the bloom is continuous rather than popping on.
        for (dst, src) in self.bright.chunks_exact_mut(3).zip(color.chunks_exact(3)) {
            for c in 0..3 {
                let v = src[c];
                dst[c] = if v > threshold { v - threshold } else { 0.0 };
            }
        }
        let radius = settings.bloom_radius.max(1) as i32;
        // Separable box blur, twice: a good enough approximation of a Gaussian
        // for a glow, and O(1) per pixel with a running sum.
        box_blur_h(&self.bright, &mut self.blur, width, height, radius);
        box_blur_v(&self.blur, &mut self.bright, width, height, radius);
        box_blur_h(&self.bright, &mut self.blur, width, height, radius * 2);
        box_blur_v(&self.blur, &mut self.bright, width, height, radius * 2);

        let intensity = settings.bloom_intensity;
        for (dst, glow) in fb
            .color_slice_mut()
            .chunks_exact_mut(3)
            .zip(self.bright.chunks_exact(3))
        {
            dst[0] += glow[0] * intensity;
            dst[1] += glow[1] * intensity;
            dst[2] += glow[2] * intensity;
        }
    }

    /// Fills the image with a solid colour, ignoring existing content.
    ///
    /// Used by tests and by the "solid backdrop" mode in the demo.
    pub fn fill(fb: &mut Framebuffer, color: Color) {
        fb.clear([color.r, color.g, color.b]);
    }

    /// Bytes of scratch memory held.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.bright.capacity() * 4 + self.blur.capacity() * 4
    }
}

/// Horizontal box blur of a 3-channel `f32` image.
fn box_blur_h(src: &[f32], dst: &mut [f32], width: u32, height: u32, radius: i32) {
    let w = width as i32;
    let r = radius.max(1);
    let window = (r * 2 + 1) as f32;
    for y in 0..height as i32 {
        let row = (y * w) as usize * 3;
        for c in 0..3 {
            // Prime the running sum with the clamped left edge.
            let mut sum = 0.0f32;
            for k in -r..=r {
                let x = k.clamp(0, w - 1);
                sum += src[row + x as usize * 3 + c];
            }
            for x in 0..w {
                dst[row + x as usize * 3 + c] = sum / window;
                let out = (x - r).clamp(0, w - 1);
                let incoming = (x + r + 1).clamp(0, w - 1);
                sum += src[row + incoming as usize * 3 + c] - src[row + out as usize * 3 + c];
            }
        }
    }
}

/// Vertical box blur of a 3-channel `f32` image.
fn box_blur_v(src: &[f32], dst: &mut [f32], width: u32, height: u32, radius: i32) {
    let w = width as i32;
    let h = height as i32;
    let r = radius.max(1);
    let window = (r * 2 + 1) as f32;
    for x in 0..w {
        let col = x as usize * 3;
        for c in 0..3 {
            let mut sum = 0.0f32;
            for k in -r..=r {
                let y = k.clamp(0, h - 1);
                sum += src[(y * w) as usize * 3 + col + c];
            }
            for y in 0..h {
                dst[(y * w) as usize * 3 + col + c] = sum / window;
                let out = (y - r).clamp(0, h - 1);
                let incoming = (y + r + 1).clamp(0, h - 1);
                sum += src[(incoming * w) as usize * 3 + col + c]
                    - src[(out * w) as usize * 3 + col + c];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [f32; 3]) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        fb.clear(rgb);
        fb
    }

    #[test]
    fn identity_settings_do_nothing() {
        let mut fb = solid(8, 8, [0.5, 0.25, 0.75]);
        let before = fb.color_slice().to_vec();
        let mut post = PostProcess::new();
        let ms = post.apply(&mut fb, &PostSettings::default());
        assert_eq!(fb.color_slice(), before.as_slice());
        assert_eq!(ms, 0.0);
        assert!(PostSettings::default().is_identity());
    }

    #[test]
    fn gain_and_lift() {
        let mut fb = solid(2, 2, [0.5, 0.5, 0.5]);
        let post = PostProcess::new();
        post.apply_gain_lift(
            &mut fb,
            &PostSettings {
                gain: 2.0,
                lift: 0.1,
                ..Default::default()
            },
        );
        assert!((fb.get(0, 0).unwrap()[0] - 1.1).abs() < 1e-5);
    }

    #[test]
    fn lift_never_goes_negative() {
        let mut fb = solid(2, 2, [0.0, 0.0, 0.0]);
        let post = PostProcess::new();
        post.apply_gain_lift(
            &mut fb,
            &PostSettings {
                gain: 1.0,
                lift: -5.0,
                ..Default::default()
            },
        );
        assert_eq!(fb.get(0, 0).unwrap(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn desaturate_reaches_luminance() {
        let mut fb = solid(2, 2, [1.0, 0.0, 0.0]);
        let post = PostProcess::new();
        post.apply_desaturate(&mut fb, 1.0);
        let c = fb.get(0, 0).unwrap();
        assert!(
            (c[0] - c[1]).abs() < 1e-5 && (c[1] - c[2]).abs() < 1e-5,
            "{c:?}"
        );
        assert!((c[0] - 0.2126).abs() < 1e-4);
    }

    #[test]
    fn desaturate_zero_is_a_noop() {
        let mut fb = solid(2, 2, [0.2, 0.4, 0.6]);
        let before = fb.color_slice().to_vec();
        PostProcess::new().apply_desaturate(&mut fb, 0.0);
        assert_eq!(fb.color_slice(), before.as_slice());
    }

    #[test]
    fn vignette_darkens_corners_more_than_the_centre() {
        let mut fb = solid(32, 32, [1.0, 1.0, 1.0]);
        let post = PostProcess::new();
        post.apply_vignette(&mut fb, 0.5, 0.5);
        let centre = fb.get(16, 16).unwrap()[0];
        let corner = fb.get(0, 0).unwrap()[0];
        assert!(centre > corner, "centre {centre} corner {corner}");
        assert_eq!(centre, 1.0, "inside the radius nothing changes");
    }

    #[test]
    fn vignette_never_goes_negative() {
        let mut fb = solid(16, 16, [1.0, 1.0, 1.0]);
        PostProcess::new().apply_vignette(&mut fb, 4.0, 0.0);
        assert!(fb.color_slice().iter().all(|c| *c >= 0.0));
    }

    #[test]
    fn scanlines_darken_every_other_row() {
        let mut fb = solid(4, 4, [1.0, 1.0, 1.0]);
        PostProcess::new().apply_scanlines(&mut fb, 2, 0.5);
        assert!((fb.get(0, 0).unwrap()[0] - 0.5).abs() < 1e-6);
        assert_eq!(fb.get(0, 1).unwrap()[0], 1.0);
        assert!((fb.get(0, 2).unwrap()[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn bloom_ignores_dark_pixels() {
        let mut fb = solid(16, 16, [0.2, 0.2, 0.2]);
        let before = fb.color_slice().to_vec();
        let mut post = PostProcess::new();
        post.apply_bloom(
            &mut fb,
            &PostSettings {
                bloom_threshold: 0.8,
                bloom_intensity: 1.0,
                bloom_radius: 2,
                ..Default::default()
            },
        );
        assert_eq!(
            fb.color_slice(),
            before.as_slice(),
            "nothing is above the threshold"
        );
    }

    #[test]
    fn bloom_spreads_bright_pixels() {
        let mut fb = Framebuffer::new(32, 32);
        fb.clear([0.0, 0.0, 0.0]);
        // A single bright dot.
        fb.set(16, 16, [4.0, 4.0, 4.0]);
        let mut post = PostProcess::new();
        post.apply_bloom(
            &mut fb,
            &PostSettings {
                bloom_threshold: 1.0,
                bloom_intensity: 1.0,
                bloom_radius: 3,
                ..Default::default()
            },
        );
        // Its neighbours must now be lit.
        assert!(fb.get(14, 16).unwrap()[0] > 0.0, "bloom must spread");
        assert!(fb.get(16, 16).unwrap()[0] > 4.0, "the source gets brighter");
        // And something far away must stay dark.
        assert_eq!(fb.get(0, 0).unwrap(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn bloom_is_energy_positive_but_bounded() {
        let mut fb = solid(16, 16, [2.0, 2.0, 2.0]);
        let before: f32 = fb.color_slice().iter().sum();
        let mut post = PostProcess::new();
        post.apply_bloom(
            &mut fb,
            &PostSettings {
                bloom_threshold: 1.0,
                bloom_intensity: 0.5,
                bloom_radius: 2,
                ..Default::default()
            },
        );
        let after: f32 = fb.color_slice().iter().sum();
        assert!(after > before, "bloom adds light");
        assert!(after < before * 3.0, "but not unboundedly");
    }

    #[test]
    fn box_blur_preserves_a_uniform_field() {
        let src = vec![0.5f32; 16 * 16 * 3];
        let mut dst = vec![0.0f32; 16 * 16 * 3];
        box_blur_h(&src, &mut dst, 16, 16, 3);
        assert!(
            dst.iter().all(|v| (v - 0.5).abs() < 1e-4),
            "{:?}",
            &dst[..6]
        );
    }

    #[test]
    fn box_blur_spreads_and_conserves_energy_approximately() {
        let mut src = vec![0.0f32; 16 * 16 * 3];
        src[(8 * 16 + 8) * 3] = 1.0;
        src[(8 * 16 + 8) * 3 + 1] = 1.0;
        src[(8 * 16 + 8) * 3 + 2] = 1.0;
        let mut dst = vec![0.0f32; 16 * 16 * 3];
        box_blur_h(&src, &mut dst, 16, 16, 2);
        let sum_in: f32 = src.iter().sum();
        let sum_out: f32 = dst.iter().sum();
        assert!(sum_out > 0.0);
        // Edge clamping loses or gains a little energy; stay within 25%.
        assert!(
            (sum_out - sum_in).abs() / sum_in < 0.25,
            "{sum_in} vs {sum_out}"
        );
        assert!(dst[(8 * 16 + 6) * 3] > 0.0, "the blur must reach sideways");
    }

    #[test]
    fn full_chain_runs_and_is_bounded() {
        let mut fb = solid(24, 24, [0.5, 0.5, 0.5]);
        let mut post = PostProcess::new();
        let ms = post.apply(&mut fb, &PostSettings::cinematic());
        assert!(ms >= 0.0);
        assert!(fb.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn retro_preset_applies_scanlines() {
        let mut fb = solid(8, 8, [1.0, 1.0, 1.0]);
        let mut post = PostProcess::new();
        post.apply(&mut fb, &PostSettings::retro());
        assert!(fb.get(0, 0).unwrap()[0] < 1.0);
        assert!(fb.get(0, 1).unwrap()[0] >= fb.get(0, 0).unwrap()[0]);
    }

    #[test]
    fn scratch_buffers_are_reused_across_sizes() {
        let mut post = PostProcess::new();
        let mut a = solid(8, 8, [2.0, 2.0, 2.0]);
        post.apply(&mut a, &PostSettings::cinematic());
        let mut b = solid(32, 16, [2.0, 2.0, 2.0]);
        post.apply(&mut b, &PostSettings::cinematic());
        assert_eq!(post.memory_bytes(), 32 * 16 * 3 * 4 * 2);
    }

    #[test]
    fn fill_replaces_everything() {
        let mut fb = solid(4, 4, [0.0, 0.0, 0.0]);
        PostProcess::fill(&mut fb, Color::rgb(0.25, 0.5, 0.75));
        assert_eq!(fb.get(3, 3).unwrap(), [0.25, 0.5, 0.75]);
    }
}
