//! Clipped, integer-space drawing into the linear framebuffer.
//!
//! Every primitive here goes through three rules, and they are the whole reason
//! the UI looks crisp:
//!
//! 1. **Integer coordinates.** Nothing takes or produces an `f32`. A rectangle at
//!    a fractional position would be resampled by the window's whole-number
//!    upscale and come out two pixels wide and half-bright.
//! 2. **Clipping, not overflow.** A widget draws inside its own rectangle even
//!    when a child is bigger than its parent — a scroll view, a tooltip near the
//!    screen edge, a long name in a narrow slot. Without a clip those bleed
//!    across the whole UI, which is the single most common way an immediate-mode
//!    UI looks broken.
//! 3. **Linear colour.** The framebuffer holds linear radiance, so a colour is
//!    converted once with [`Color8::to_linear`] and blended there. A pixel drawn
//!    opaquely in `#FF0000` resolves back to exactly `#FF0000`.

use noxel_asset::image::Image;
use noxel_core::math::{Color, Color8};
use noxel_render::framebuffer::Framebuffer;

use crate::geom::{Insets, UiRect};
use crate::theme::FrameStyle;

/// The linear RGB of an sRGB colour, which is what the framebuffer stores.
#[inline]
fn linear_of(color: Color8) -> [f32; 3] {
    color.to_linear().to_array3()
}

/// The linear colour a texel contributes under a tint.
///
/// [`Color8::WHITE`] means "draw the texel as authored", which is the common
/// case and is why it is special-cased rather than left as a multiply. The
/// alternative — treating white as a multiply — is also correct arithmetic and
/// was the bug here: a white tint multiplied against an atlas of white
/// coverage glyphs is fine, but against *coloured* art it draws the whole
/// nine-slice white. The panel frames came out blank and the glyphs did not,
/// which is exactly the shape of a bug that only shows up in the stretched
/// pieces.
#[inline]
fn color_of(texel: Color8, tint: Color8) -> [f32; 3] {
    if tint == Color8::WHITE {
        linear_of(texel)
    } else {
        linear_of(tint)
    }
}

/// A clipped drawing surface over a framebuffer.
///
/// A painter is cheap to make and is meant to be made often: one per layer, one
/// per clipping region, one for a nested panel. It holds a mutable borrow of the
/// framebuffer, so there is only ever one at a time — which is what stops the
/// "two layers, interleaved writes, wrong z-order" bug by construction.
pub struct Painter<'a> {
    target: &'a mut Framebuffer,
    clip: UiRect,
    opacity: f32,
}

impl<'a> Painter<'a> {
    /// A painter clipped to `clip`.
    #[must_use]
    pub fn new(target: &'a mut Framebuffer, clip: UiRect) -> Self {
        let screen = UiRect::screen(target.width(), target.height());
        Self {
            target,
            clip: clip.intersect(screen),
            opacity: 1.0,
        }
    }

    /// A painter clipped to the whole framebuffer.
    #[must_use]
    pub fn full(target: &'a mut Framebuffer) -> Self {
        let clip = UiRect::screen(target.width(), target.height());
        Self {
            target,
            clip,
            opacity: 1.0,
        }
    }

    /// The framebuffer being drawn into.
    #[must_use]
    pub fn target(&self) -> &Framebuffer {
        self.target
    }

    /// The framebuffer's size.
    #[must_use]
    pub fn screen(&self) -> UiRect {
        UiRect::screen(self.target.width(), self.target.height())
    }

    /// The region everything is currently clipped to.
    #[must_use]
    pub fn clip(&self) -> UiRect {
        self.clip
    }

    /// Narrows the clip to `rect` for the duration of `draw`, then restores it.
    ///
    /// This is the API that makes nested layout safe: a child cannot paint
    /// outside its parent no matter what rectangle it asks for.
    pub fn clipped<T>(&mut self, rect: UiRect, draw: impl FnOnce(&mut Painter<'_>) -> T) -> T {
        let previous = self.clip;
        self.clip = self.clip.intersect(rect);
        let result = draw(self);
        self.clip = previous;
        result
    }

    /// Scales the alpha of everything drawn by `draw`, then restores it.
    ///
    /// Used for a fade: a whole panel can be drawn at 40% while it animates in,
    /// without every widget knowing about the animation.
    ///
    /// A closure rather than a guard object, and deliberately so. A guard would
    /// have to hold a borrow of the painter to restore the value on drop, and a
    /// borrowed painter cannot be drawn into — `let _g = p.fade(0.5); p.fill(..)`
    /// does not compile. Returning the value through a closure keeps the
    /// restoration infallible and the painter usable.
    pub fn with_opacity<T>(&mut self, opacity: f32, draw: impl FnOnce(&mut Painter<'_>) -> T) -> T {
        let previous = self.opacity;
        self.opacity = (previous * opacity).clamp(0.0, 1.0);
        let result = draw(self);
        self.opacity = previous;
        result
    }

    /// Sets the alpha multiplier directly.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// The alpha multiplier currently in effect.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Whether a rectangle is worth drawing at all.
    #[must_use]
    pub fn visible(&self, rect: UiRect) -> bool {
        !rect.intersect(self.clip).is_empty() && self.opacity > 0.0
    }

    /// Writes one pixel, respecting the clip. Values outside are dropped.
    pub fn pixel(&mut self, x: i32, y: i32, color: Color8) {
        if !self.clip.contains(x, y) || self.opacity <= 0.0 {
            return;
        }
        let alpha = (color.a as f32 / 255.0) * self.opacity;
        if alpha <= 0.0 {
            return;
        }
        let Some(px) = u32::try_from(x).ok() else {
            return;
        };
        let Some(py) = u32::try_from(y).ok() else {
            return;
        };
        self.target.blend(px, py, linear_of(color), alpha);
    }

    /// Fills a rectangle.
    ///
    /// Opaque fills take a fast path that skips the blend, which matters because
    /// the background of a full-screen panel is the largest single fill in a
    /// frame.
    pub fn fill(&mut self, rect: UiRect, color: Color8) {
        let area = rect.intersect(self.clip);
        if area.is_empty() || self.opacity <= 0.0 || color.a == 0 {
            return;
        }
        let alpha = (color.a as f32 / 255.0) * self.opacity;
        if alpha <= 0.0 {
            return;
        }
        let rgb = linear_of(color);
        // An opaque fill is the common case (panel backgrounds, bars), so it
        // takes the branch that skips the blend arithmetic entirely.
        if alpha >= 1.0 {
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) {
                        self.target.set(px, py, rgb);
                    }
                }
            }
            return;
        }
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) {
                    self.target.blend(px, py, rgb, alpha);
                }
            }
        }
    }

    /// Draws a rectangle's border, drawn inside its own bounds.
    ///
    /// Inside rather than centred: a border that straddles the edge adds a pixel
    /// to the widget's footprint, so two adjacent widgets draw a two-pixel seam
    /// where the layout said there was one.
    pub fn outline(&mut self, rect: UiRect, color: Color8, thickness: u32) {
        let thickness = thickness.min(rect.w.min(rect.h));
        if thickness == 0 {
            return;
        }
        let thickness = thickness as i32;
        self.fill(UiRect::new(rect.x, rect.y, rect.w, thickness as u32), color);
        self.fill(
            UiRect::new(rect.x, rect.bottom() - thickness, rect.w, thickness as u32),
            color,
        );
        self.fill(
            UiRect::new(
                rect.x,
                rect.y + thickness,
                thickness as u32,
                rect.h - 2 * thickness as u32,
            ),
            color,
        );
        self.fill(
            UiRect::new(
                rect.right() - thickness,
                rect.y + thickness,
                thickness as u32,
                rect.h - 2 * thickness as u32,
            ),
            color,
        );
    }

    /// Fills a rectangle with a vertical gradient from `top` to `bottom`.
    ///
    /// The gradient is quantised to whole rows and interpolated in **sRGB**
    /// space, not linear: a linear ramp between two colours reads as a much
    /// darker band in the middle, which is a real effect and the wrong one for a
    /// UI panel.
    pub fn gradient_v(&mut self, rect: UiRect, top: Color8, bottom: Color8) {
        let area = rect.intersect(self.clip);
        if area.is_empty() || area.h == 0 {
            return;
        }
        for y in area.y..area.bottom() {
            let t = if area.h <= 1 {
                0.0
            } else {
                (y - area.y) as f32 / (area.h - 1) as f32
            };
            let color = Color8::new(
                lerp_u8(top.r, bottom.r, t),
                lerp_u8(top.g, bottom.g, t),
                lerp_u8(top.b, bottom.b, t),
                lerp_u8(top.a, bottom.a, t),
            );
            self.fill(UiRect::new(area.x, y, area.w, 1), color);
        }
    }

    /// Draws a stroked line, one pixel thick, Bresenham-style.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color8) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let doubled = 2 * error;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// Copies a region of an image, one texel per pixel.
    ///
    /// `tint` replaces the source's colour and keeps its alpha, so a white glyph
    /// in an atlas can be drawn in any colour without a second texture. Passing
    /// `Color8::WHITE` draws the source unchanged.
    pub fn blit(&mut self, source: &Image, src: UiRect, x: i32, y: i32, tint: Color8) {
        self.blit_scaled(source, src, x, y, 1, tint);
    }

    /// Copies a region of an image at an integer scale.
    ///
    /// Only whole-number scales: a 1.5x blit has to either drop or duplicate a
    /// row, and both look like a rendering bug next to art that does neither.
    pub fn blit_scaled(
        &mut self,
        source: &Image,
        src: UiRect,
        x: i32,
        y: i32,
        scale: u32,
        tint: Color8,
    ) {
        let scale = scale.max(1) as i32;
        if src.is_empty() || self.opacity <= 0.0 || tint.a == 0 {
            return;
        }
        let tint_alpha = tint.a as f32 / 255.0;
        let (sw, sh) = (source.width(), source.height());

        for sy in 0..src.h {
            for sx in 0..src.w {
                let ux = src.x + sx as i32;
                let uy = src.y + sy as i32;
                if ux < 0 || uy < 0 || ux as u32 >= sw || uy as u32 >= sh {
                    continue;
                }
                let texel = source
                    .get(ux as u32, uy as u32)
                    .unwrap_or(Color8::TRANSPARENT);
                if texel.a == 0 {
                    continue;
                }
                let alpha = tint_alpha * (texel.a as f32 / 255.0) * self.opacity;
                if alpha <= 0.0 {
                    continue;
                }
                let rgb = color_of(texel, tint);
                let px0 = x + sx as i32 * scale;
                let py0 = y + sy as i32 * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = px0 + dx;
                        let py = py0 + dy;
                        if !self.clip.contains(px, py) {
                            continue;
                        }
                        if let (Ok(ux), Ok(uy)) = (u32::try_from(px), u32::try_from(py)) {
                            self.target.blend(ux, uy, rgb, alpha);
                        }
                    }
                }
            }
        }
    }

    /// Draws a nine-slice frame: fixed corners, stretched edges, stretched centre.
    ///
    /// The destination rectangle may be any size, which is the point — one 12x12
    /// authored button in the atlas becomes a 40x14 button and a 200x90 dialog
    /// without a second asset, and the corners never distort.
    ///
    /// When the destination is too small for both borders the insets are shrunk
    /// proportionally rather than clamped independently, so the frame stays
    /// symmetric instead of losing its right edge first.
    pub fn nine_slice(
        &mut self,
        source: &Image,
        src: UiRect,
        dst: UiRect,
        insets: Insets,
        tint: Color8,
    ) {
        if src.is_empty() || dst.is_empty() {
            return;
        }
        let left = insets.left.max(0).min(src.w as i32);
        let right = insets.right.max(0).min(src.w as i32 - left);
        let top = insets.top.max(0).min(src.h as i32);
        let bottom = insets.bottom.max(0).min(src.h as i32 - top);

        // Fit the borders into the destination without letting them cross.
        let (dl, dr, dt, db) = fit_borders(dst, left, right, top, bottom);

        let src_mid_w = (src.w as i32 - left - right).max(0) as u32;
        let src_mid_h = (src.h as i32 - top - bottom).max(0) as u32;
        let dst_mid_w = (dst.w as i32 - dl - dr).max(0) as u32;
        let dst_mid_h = (dst.h as i32 - dt - db).max(0) as u32;

        let src_x = [src.x, src.x + left, src.x + left + src_mid_w as i32];
        let src_y = [src.y, src.y + top, src.y + top + src_mid_h as i32];
        let src_w = [left as u32, src_mid_w, right as u32];
        let src_h = [top as u32, src_mid_h, bottom as u32];

        let dst_x = [dst.x, dst.x + dl, dst.right() - dr];
        let dst_y = [dst.y, dst.y + dt, dst.bottom() - db];
        let dst_w = [dl as u32, dst_mid_w, dr as u32];
        let dst_h = [dt as u32, dst_mid_h, db as u32];

        // Every piece goes through one nearest-sample stretch. When a source
        // dimension already matches its destination — which is the case for
        // every corner in the common path — the sampling is exactly 1:1 and the
        // result is identical to a blit, so the case analysis that used to live
        // here bought nothing and got the centre of the frame wrong: the middle
        // piece has to stretch in *both* axes, and the two single-axis helpers
        // each covered only one.
        for row in 0..3 {
            for column in 0..3 {
                let piece_src = UiRect::new(src_x[column], src_y[row], src_w[column], src_h[row]);
                let piece_dst = UiRect::new(dst_x[column], dst_y[row], dst_w[column], dst_h[row]);
                if piece_src.is_empty() || piece_dst.is_empty() {
                    continue;
                }
                self.stretch_piece(source, piece_src, piece_dst, tint);
            }
        }
    }

    /// Copies `src` into `dst`, sampling nearest-neighbour on both axes.
    ///
    /// Nearest rather than bilinear: a stretched pixel-art edge keeps its texel
    /// boundaries, and a bilinear sample would introduce colours the artist
    /// never drew along every seam of every panel.
    fn stretch_piece(&mut self, source: &Image, src: UiRect, dst: UiRect, tint: Color8) {
        let (sw, sh) = (source.width(), source.height());
        let alpha_scale = tint.a as f32 / 255.0 * self.opacity;
        if alpha_scale <= 0.0 {
            return;
        }
        for dy in 0..dst.h {
            let sy = src.y + (dy as u64 * src.h as u64 / dst.h.max(1) as u64) as i32;
            if sy < 0 || sy as u32 >= sh {
                continue;
            }
            let py = dst.y + dy as i32;
            if py < self.clip.y || py >= self.clip.bottom() {
                continue;
            }
            for dx in 0..dst.w {
                let sx = src.x + (dx as u64 * src.w as u64 / dst.w.max(1) as u64) as i32;
                if sx < 0 || sx as u32 >= sw {
                    continue;
                }
                let px = dst.x + dx as i32;
                if px < self.clip.x || px >= self.clip.right() {
                    continue;
                }
                let texel = source
                    .get(sx as u32, sy as u32)
                    .unwrap_or(Color8::TRANSPARENT);
                if texel.a == 0 {
                    continue;
                }
                let alpha = alpha_scale * (texel.a as f32 / 255.0);
                if alpha <= 0.0 {
                    continue;
                }
                if let (Ok(px), Ok(py)) = (u32::try_from(px), u32::try_from(py)) {
                    self.target.blend(px, py, color_of(texel, tint), alpha);
                }
            }
        }
    }

    /// Draws a themed box: a nine-slice when the style carries art, a flat fill
    /// with a border when it does not.
    ///
    /// The fallback is not a placeholder — a flat box is what a UI panel with a
    /// two-colour design actually wants, and it is what lets a game build its
    /// interface before the art exists.
    pub fn frame(&mut self, source: &Image, style: &FrameStyle, dst: UiRect) {
        if style.shadow_offset != (0, 0) && style.shadow.a != 0 {
            let shadow = UiRect::new(
                dst.x + style.shadow_offset.0,
                dst.y + style.shadow_offset.1,
                dst.w,
                dst.h,
            );
            if style.is_sliced() {
                self.nine_slice(source, style.source, shadow, style.insets, style.shadow);
            } else {
                self.fill(shadow, style.shadow);
            }
        }
        if style.is_sliced() {
            self.nine_slice(source, style.source, dst, style.insets, style.tint);
            return;
        }
        if style.fill.a != 0 {
            self.fill(dst, style.fill);
        }
        if style.border_width > 0 && style.border.a != 0 {
            self.outline(dst, style.border, style.border_width);
        }
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Shrinks border insets until the left and right (or top and bottom) fit.
fn fit_borders(dst: UiRect, left: i32, right: i32, top: i32, bottom: i32) -> (i32, i32, i32, i32) {
    let (mut left, mut right) = (left, right);
    let horizontal = left + right;
    if horizontal > dst.w as i32 && horizontal > 0 {
        // Scale both sides by the same factor so the frame stays symmetric.
        let scale = dst.w as f32 / horizontal as f32;
        left = (left as f32 * scale).floor() as i32;
        right = dst.w as i32 - left;
    }
    let (mut top, mut bottom) = (top, bottom);
    let vertical = top + bottom;
    if vertical > dst.h as i32 && vertical > 0 {
        let scale = dst.h as f32 / vertical as f32;
        top = (top as f32 * scale).floor() as i32;
        bottom = dst.h as i32 - top;
    }
    (left.max(0), right.max(0), top.max(0), bottom.max(0))
}

/// Converts a linear colour back to the sRGB byte a designer wrote.
///
/// Exposed because a test or a tool that samples the framebuffer needs to undo
/// the resolve to compare against authored art.
#[must_use]
pub fn srgb_of(linear: [f32; 3]) -> Color8 {
    Color::from_array3(linear).to_srgb8()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painter(fb: &mut Framebuffer) -> Painter<'_> {
        Painter::full(fb)
    }

    #[test]
    fn an_opaque_fill_round_trips_to_the_authored_byte() {
        // The framebuffer is linear; resolving must give back exactly what was
        // asked for, or every colour in the UI is subtly wrong.
        let mut fb = Framebuffer::new(8, 8);
        let color = Color8::new(199, 84, 41, 255);
        painter(&mut fb).fill(UiRect::new(1, 1, 4, 4), color);
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(2, 2), Some(color));
        // Outside the rectangle nothing was written.
        assert_eq!(image.get(0, 0), Some(Color8::new(0, 0, 0, 255)));
    }

    #[test]
    fn clipping_drops_everything_outside_the_region() {
        let mut fb = Framebuffer::new(16, 16);
        {
            let mut p = painter(&mut fb);
            p.clipped(UiRect::new(4, 4, 4, 4), |inner| {
                // Ask for far more than the clip allows; none of it may land.
                inner.fill(UiRect::new(0, 0, 16, 16), Color8::WHITE);
            });
        }
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(5, 5), Some(Color8::WHITE));
        assert_eq!(image.get(3, 3), Some(Color8::BLACK));
        assert_eq!(image.get(9, 9), Some(Color8::BLACK));
    }

    #[test]
    fn clipping_is_restored_when_the_closure_returns() {
        let mut fb = Framebuffer::new(16, 16);
        let mut p = painter(&mut fb);
        let outer = p.clip();
        p.clipped(UiRect::new(2, 2, 2, 2), |inner| {
            assert_eq!(inner.clip(), UiRect::new(2, 2, 2, 2));
        });
        assert_eq!(p.clip(), outer, "the clip must not leak out of `clipped`");
    }

    #[test]
    fn a_clip_outside_the_framebuffer_is_intersected_away() {
        let mut fb = Framebuffer::new(8, 8);
        let p = Painter::new(&mut fb, UiRect::new(-100, -100, 400, 400));
        assert_eq!(p.clip(), UiRect::screen(8, 8));
    }

    #[test]
    fn an_unknown_tint_leaves_an_alpha_only_glyph_at_its_coverage() {
        // The atlas is white coverage; tinting must replace the colour and keep
        // the shape, which is what makes one atlas draw every text colour.
        let mut source = Image::transparent(4, 4);
        source.set(0, 0, Color8::new(255, 255, 255, 255));
        let mut fb = Framebuffer::new(4, 4);
        painter(&mut fb).blit(
            &source,
            UiRect::new(0, 0, 4, 4),
            0,
            0,
            Color8::new(255, 0, 0, 255),
        );
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(0, 0), Some(Color8::new(255, 0, 0, 255)));
        assert_eq!(
            image.get(1, 1),
            Some(Color8::BLACK),
            "transparent texels draw nothing"
        );
    }

    #[test]
    fn a_scoped_fade_scales_what_is_drawn_and_restores_afterwards() {
        let mut fb = Framebuffer::new(4, 4);
        let mut p = painter(&mut fb);
        assert!((p.opacity() - 1.0).abs() < 1e-6);
        p.with_opacity(0.5, |inner| {
            // Half white over black, in linear space.
            inner.fill(UiRect::new(0, 0, 2, 2), Color8::WHITE);
        });
        assert!(
            (p.opacity() - 1.0).abs() < 1e-6,
            "the fade must not leak out of the closure"
        );
        // The second half is drawn at full strength, so the two halves differ.
        p.fill(UiRect::new(2, 0, 2, 2), Color8::WHITE);

        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        let faded = image.get(0, 0).unwrap().r;
        let full = image.get(3, 0).unwrap().r;
        assert!(
            (186..=190).contains(&faded),
            "half white should resolve near 188, got {faded}"
        );
        assert_eq!(full, 255);
    }

    #[test]
    fn half_alpha_over_black_is_half_in_linear_space() {
        // Blending happens in linear radiance, not in sRGB bytes: a 50% white
        // over black must resolve to sRGB 188, not to 128.
        let mut fb = Framebuffer::new(2, 2);
        painter(&mut fb).fill(UiRect::new(0, 0, 2, 2), Color8::new(255, 255, 255, 128));
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        let pixel = image.get(0, 0).unwrap();
        assert!(
            (186..=190).contains(&pixel.r),
            "linear half white should resolve near 188, got {}",
            pixel.r
        );
    }

    #[test]
    fn a_border_is_drawn_inside_its_own_rectangle() {
        // A straddling border would add a pixel to the widget's footprint and
        // break adjacency in a grid.
        let mut fb = Framebuffer::new(10, 10);
        painter(&mut fb).outline(UiRect::new(2, 2, 6, 6), Color8::WHITE, 1);
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(2, 2), Some(Color8::WHITE));
        assert_eq!(image.get(7, 7), Some(Color8::WHITE));
        assert_eq!(
            image.get(1, 1),
            Some(Color8::BLACK),
            "the border must not spill left"
        );
        assert_eq!(
            image.get(8, 8),
            Some(Color8::BLACK),
            "the border must not spill right"
        );
        assert_eq!(
            image.get(4, 4),
            Some(Color8::BLACK),
            "the interior stays empty"
        );
    }

    #[test]
    fn a_nine_slice_stretches_the_source_colour_not_the_tint() {
        // The stretched edges and centre once drew pure white regardless of the
        // atlas, because they used the tint as the colour instead of the texel.
        // A white-tinted frame therefore came out blank while the corners — which
        // go through `blit` — stayed correct, so the panel looked like a white
        // box with a coloured border.
        let mut source = Image::new(4, 4, Color8::new(120, 80, 40, 255));
        source.set(0, 0, Color8::new(200, 30, 30, 255));
        let middle = Color8::new(120, 80, 40, 255);

        let mut fb = Framebuffer::new(20, 20);
        painter(&mut fb).nine_slice(
            &source,
            UiRect::new(0, 0, 4, 4),
            UiRect::new(0, 0, 20, 20),
            Insets::all(1),
            Color8::WHITE,
        );
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        // The stretched centre, the stretched top edge and the stretched left
        // edge all have to keep the source colour.
        assert_eq!(
            image.get(10, 10),
            Some(middle),
            "the centre was not the source colour"
        );
        assert_eq!(
            image.get(10, 0),
            Some(middle),
            "the top edge was not the source colour"
        );
        assert_eq!(
            image.get(0, 10),
            Some(middle),
            "the left edge was not the source colour"
        );
        // And the corner is still authored art, not the fill.
        assert_eq!(image.get(0, 0), Some(Color8::new(200, 30, 30, 255)));
    }

    #[test]
    fn a_tinted_nine_slice_replaces_the_colour_everywhere() {
        // The other half of the same contract: an explicit tint must recolour
        // the corners and the stretched pieces alike, or a hover state would
        // light up the frame but not its edges.
        let mut source = Image::new(4, 4, Color8::new(120, 80, 40, 255));
        source.set(0, 0, Color8::new(200, 30, 30, 255));
        let tint = Color8::new(0, 200, 0, 255);

        let mut fb = Framebuffer::new(20, 20);
        painter(&mut fb).nine_slice(
            &source,
            UiRect::new(0, 0, 4, 4),
            UiRect::new(0, 0, 20, 20),
            Insets::all(1),
            tint,
        );
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(0, 0), Some(tint), "the corner ignored the tint");
        assert_eq!(image.get(10, 10), Some(tint), "the centre ignored the tint");
    }

    #[test]
    fn border_insets_scale_symmetrically_when_the_box_is_too_small() {
        // A 3px-wide box cannot hold 4px borders; losing the right edge first is
        // the bug this guards, so both sides must give equally.
        let (left, right, _, _) = fit_borders(UiRect::new(0, 0, 3, 3), 4, 4, 1, 1);
        assert_eq!(left + right, 3);
        assert!(left >= 1 && right >= 1, "neither edge may vanish entirely");
    }

    #[test]
    fn nine_slice_keeps_the_corners_at_their_authored_size() {
        // A 4x4 source with 1px borders: the corner texels must survive a blit
        // into a much larger box untouched.
        let mut source = Image::new(4, 4, Color8::new(10, 10, 10, 255));
        source.set(0, 0, Color8::new(1, 2, 3, 255));
        source.set(3, 0, Color8::new(4, 5, 6, 255));
        source.set(0, 3, Color8::new(7, 8, 9, 255));
        source.set(3, 3, Color8::new(11, 12, 13, 255));

        let mut fb = Framebuffer::new(20, 20);
        painter(&mut fb).nine_slice(
            &source,
            UiRect::new(0, 0, 4, 4),
            UiRect::new(0, 0, 20, 20),
            Insets::all(1),
            Color8::WHITE,
        );
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(
            image.get(0, 0),
            Some(Color8::new(1, 2, 3, 255)),
            "top-left corner moved"
        );
        assert_eq!(
            image.get(19, 0),
            Some(Color8::new(4, 5, 6, 255)),
            "top-right corner moved"
        );
        assert_eq!(
            image.get(0, 19),
            Some(Color8::new(7, 8, 9, 255)),
            "bottom-left corner moved"
        );
        assert_eq!(
            image.get(19, 19),
            Some(Color8::new(11, 12, 13, 255)),
            "bottom-right corner moved"
        );
    }
}
