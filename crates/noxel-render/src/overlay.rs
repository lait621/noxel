//! The debug overlay: wireframes, rays, and a small built-in font.
//!
//! Noxel has **no UI system** and no plans for one — see
//! `docs/adr/0009-no-ui.md`. What it does have is this: a way for an engine or
//! game developer to draw the shapes that explain what the visibility, physics
//! and NPC systems are doing. A culling bug is close to unguessable from a
//! screenshot; it is obvious the moment you can see the occluder boxes and the
//! camera-to-player ray.
//!
//! Everything here writes into the **linear** framebuffer using the exact
//! linear value of the requested sRGB colour, so an overlay drawn in
//! `#FF0000` resolves back to exactly `#FF0000`.
//!
//! ```
//! use noxel_core::math::{Color8, Vec3};
//! use noxel_render::{Framebuffer, Overlay, overlay::Font};
//!
//! let mut fb = Framebuffer::new(64, 32);
//! let overlay = Overlay::new();
//! overlay.text(&mut fb, 2, 2, "NOXEL 60 FPS", Color8::WHITE);
//! // 12 characters, 3 px wide, 1 px apart: 12 * 4 - 1.
//! assert_eq!(Font::text_width("NOXEL 60 FPS", 1), 47);
//! ```

use noxel_core::math::{Aabb, Color8, Mat4, Vec3};

use crate::framebuffer::Framebuffer;
use crate::renderer::CameraView;

/// A fixed-width bitmap font, 3 pixels wide and 5 tall, with one column of
/// spacing between glyphs.
///
/// The glyphs are stored as **editable ASCII art** rather than packed bits: a
/// future contributor (human or AI) can add a character by drawing it. The
/// packing happens once, lazily, on first use.
#[derive(Clone, Debug)]
pub struct Font {
    glyphs: Vec<(u8, [[bool; Font::WIDTH]; Font::HEIGHT])>,
}

impl Font {
    /// Glyph width in pixels.
    pub const WIDTH: usize = 3;
    /// Glyph height in pixels.
    pub const HEIGHT: usize = 5;
    /// Horizontal spacing between glyphs, in pixels.
    pub const SPACING: usize = 1;

    /// The glyph art.
    ///
    /// Entries are separated by `|`; each entry is `KEY;row0/row1/row2/row3/row4`
    /// where `#` is an ink pixel and `.` is empty. Only the first byte of `KEY`
    /// is used, so `\` and `|` can be written literally and the space glyph
    /// still works.
    ///
    /// The font is uppercase-only: lowercase input is folded to uppercase, which
    /// halves the table and is all a debug overlay needs. A character with no
    /// glyph is skipped but still advances the pen, so columns never collapse.
    const ART: &'static str = concat!(
        " ;.../.../.../.../...|",
        "0;###/#.#/#.#/#.#/###|",
        "1;.#./##./.#./.#./###|",
        "2;##./..#/.#./#../###|",
        "3;##./..#/.#./..#/##.|",
        "4;#.#/#.#/###/..#/..#|",
        "5;###/#../##./..#/##.|",
        "6;.##/#../###/#.#/###|",
        "7;###/..#/.#./#../#..|",
        "8;###/#.#/###/#.#/###|",
        "9;###/#.#/###/..#/##.|",
        "A;.#./#.#/###/#.#/#.#|",
        "B;##./#.#/##./#.#/##.|",
        "C;.##/#../#../#../.##|",
        "D;##./#.#/#.#/#.#/##.|",
        "E;###/#../##./#../###|",
        "F;###/#../##./#../#..|",
        "G;.##/#../#.#/#.#/.##|",
        "H;#.#/#.#/###/#.#/#.#|",
        "I;###/.#./.#./.#./###|",
        "J;..#/..#/..#/#.#/.#.|",
        "K;#.#/#.#/##./#.#/#.#|",
        "L;#../#../#../#../###|",
        "M;#.#/###/###/#.#/#.#|",
        "N;##./#.#/#.#/#.#/#.#|",
        "O;.#./#.#/#.#/#.#/.#.|",
        "P;##./#.#/##./#../#..|",
        "Q;.#./#.#/#.#/###/..#|",
        "R;##./#.#/##./#.#/#.#|",
        "S;.##/#../.#./..#/##.|",
        "T;###/.#./.#./.#./.#.|",
        "U;#.#/#.#/#.#/#.#/.##|",
        "V;#.#/#.#/#.#/#.#/.#.|",
        "W;#.#/#.#/###/###/#.#|",
        "X;#.#/#.#/.#./#.#/#.#|",
        "Y;#.#/#.#/.#./.#./.#.|",
        "Z;###/..#/.#./#../###|",
        ".;.../.../.../.../.#.|",
        ",;.../.../.../.#./#..|",
        ":;.../.#./.../.#./...|",
        ";;.../.#./.../.#./#..|",
        "!;.#./.#./.#./.../.#.|",
        "?;##./..#/.#./.../.#.|",
        "-;.../.../###/.../...|",
        "+;.../.#./###/.#./...|",
        "=;.../###/.../###/...|",
        "/;..#/..#/.#./#../#..|",
        "\\;#../#../.#./..#/..#|",
        "(;.#./#../#../#../.#.|",
        ");.#./..#/..#/..#/.#.|",
        "[;##./#../#../#../##.|",
        "];.##/..#/..#/..#/.##|",
        "<;..#/.#./#../.#./..#|",
        ">;#../.#./..#/.#./#..|",
        "*;.../#.#/.#./#.#/...|",
        "%;#.#/..#/.#./#../#.#|",
        "#;.#./###/.#./###/.#.|",
        "&;##./##./#.#/##./#.#|",
        "';.#./.#./.../.../...|",
        "\";#.#/#.#/.../.../...|",
        "_;.../.../.../.../###|",
        "|;.#./.#./.#./.#./.#.|",
        "~;.../.#./###/.#./...|",
        "@;###/#.#/###/#../##.|",
        "$;.#./###/#.#/###/.#.|",
        "^;.#./#.#/.../.../...|",
        "`;#../.#./.../.../...|",
        "{;.##/.#./##./.#./.##|",
        "};##./.#./.##/.#./##.|",
    );

    /// Builds the font by parsing the glyph table in `Font::ART`.
    #[must_use]
    pub fn new() -> Self {
        let mut glyphs = Vec::new();
        for entry in Self::ART.split('|') {
            let Some((key, art)) = entry.split_once(';') else {
                continue;
            };
            let rows: Vec<&str> = art.split('/').collect();
            if rows.len() != Self::HEIGHT {
                continue;
            }
            let mut bitmap = [[false; Self::WIDTH]; Self::HEIGHT];
            for (y, row) in rows.iter().enumerate() {
                for (x, ch) in row.chars().enumerate().take(Self::WIDTH) {
                    bitmap[y][x] = ch == '#';
                }
            }
            // Multi-character keys are aliases; take the first byte only.
            let byte = key.as_bytes().first().copied().unwrap_or(b'?');
            glyphs.push((byte, bitmap));
        }
        Self { glyphs }
    }

    /// The bitmap for a character, or `None` when it is not in the font.
    #[must_use]
    pub fn glyph(&self, ch: char) -> Option<&[[bool; Self::WIDTH]; Self::HEIGHT]> {
        // Reject anything outside ASCII first: `ch as u8` truncates a codepoint,
        // so U+4E2D would silently render as '-'.
        if !ch.is_ascii() {
            return None;
        }
        let mut b = ch as u8;
        if ch.is_ascii_lowercase() {
            b = ch.to_ascii_uppercase() as u8;
        }
        self.glyphs.iter().find(|(k, _)| *k == b).map(|(_, g)| g)
    }

    /// True when the character has a glyph.
    #[must_use]
    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    /// The pixel width of a string at the given scale.
    ///
    /// A string of `n` characters occupies `n * (WIDTH + SPACING) - SPACING`
    /// pixels, so the trailing gap is not counted.
    #[must_use]
    pub fn text_width(text: &str, scale: u32) -> u32 {
        let n = text.chars().count() as u32;
        let scale = scale.max(1);
        if n == 0 {
            0
        } else {
            n * (Self::WIDTH as u32 + Self::SPACING as u32) * scale - Self::SPACING as u32 * scale
        }
    }

    /// The pixel height of a line at the given scale.
    #[must_use]
    pub fn text_height(scale: u32) -> u32 {
        Self::HEIGHT as u32 * scale.max(1)
    }

    /// Number of glyphs loaded.
    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }
}

impl Default for Font {
    fn default() -> Self {
        Self::new()
    }
}

/// Draws debug primitives into a framebuffer.
#[derive(Clone, Debug)]
pub struct Overlay {
    font: Font,
    /// When true, 3D primitives are hidden behind geometry.
    depth_test: bool,
    /// Global scale for text.
    text_scale: u32,
}

impl Overlay {
    /// Creates an overlay with a depth test enabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            font: Font::new(),
            depth_test: true,
            text_scale: 1,
        }
    }

    /// Enables or disables the depth test for 3D primitives.
    ///
    /// A culling overlay is almost always more useful with the depth test off:
    /// the whole point is to see the boxes that are *inside* the world.
    pub fn set_depth_test(&mut self, on: bool) {
        self.depth_test = on;
    }

    /// True when 3D primitives are depth-tested.
    #[must_use]
    pub fn depth_test(&self) -> bool {
        self.depth_test
    }

    /// Sets the text scale.
    pub fn set_text_scale(&mut self, scale: u32) {
        self.text_scale = scale.max(1);
    }

    /// The font in use.
    #[must_use]
    pub fn font(&self) -> &Font {
        &self.font
    }

    // -------------------------------------------------------------- 3D

    /// Projects a world point to screen space, or `None` when it is behind the
    /// camera or outside the depth range.
    #[must_use]
    pub fn project(
        camera: &CameraView,
        target: &Framebuffer,
        point: Vec3,
    ) -> Option<(f32, f32, f32)> {
        let clip = camera.view_projection.transform_point4(point.extend(1.0));
        let ndc = clip.perspective_divide()?;
        if !(0.0..=1.0).contains(&ndc.z) {
            return None;
        }
        Some((
            (ndc.x * 0.5 + 0.5) * target.width() as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * target.height() as f32,
            ndc.z,
        ))
    }

    /// Draws a line between two world points.
    pub fn line(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        a: Vec3,
        b: Vec3,
        color: Color8,
    ) {
        let Some((ax, ay, az)) = Self::project(camera, target, a) else {
            return;
        };
        let Some((bx, by, bz)) = Self::project(camera, target, b) else {
            return;
        };
        self.screen_line_dz(target, ax, ay, az, bx, by, bz, color);
    }

    /// Draws the twelve edges of a world-space box.
    pub fn aabb(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        bounds: &Aabb,
        color: Color8,
    ) {
        let c = bounds.corners();
        // Corner order is (x varies fastest, then y, then z): edges connect
        // corners differing in exactly one bit.
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        for (i, j) in EDGES {
            self.line(target, camera, c[i], c[j], color);
        }
    }

    /// Draws a ray from `origin` along `direction` for `length` metres.
    pub fn ray(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        origin: Vec3,
        direction: Vec3,
        length: f32,
        color: Color8,
    ) {
        let end = origin + direction.normalize_or_zero() * length;
        self.line(target, camera, origin, end, color);
    }

    /// Draws a small cross at a world point.
    pub fn cross(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        centre: Vec3,
        size: f32,
        color: Color8,
    ) {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.line(
                target,
                camera,
                centre - axis * size,
                centre + axis * size,
                color,
            );
        }
    }

    // ---------------------------------------------------------- 2D

    /// Draws an arbitrary screen-space line.
    pub fn screen_line(
        &self,
        target: &mut Framebuffer,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: Color8,
    ) {
        self.screen_line_dz(target, x0, y0, 0.0, x1, y1, 0.0, color);
    }

    /// Draws a screen line with interpolated depth.
    #[allow(clippy::too_many_arguments)]
    fn screen_line_dz(
        &self,
        target: &mut Framebuffer,
        x0: f32,
        y0: f32,
        z0: f32,
        x1: f32,
        y1: f32,
        z1: f32,
        color: Color8,
    ) {
        // Cohen–Sutherland style rejection of lines wholly off-screen.
        let (w, h) = (target.width() as f32, target.height() as f32);
        if (x0 < 0.0 && x1 < 0.0)
            || (x0 >= w && x1 >= w)
            || (y0 < 0.0 && y1 < 0.0)
            || (y0 >= h && y1 >= h)
        {
            return;
        }
        let dx = x1 - x0;
        let dy = y1 - y0;
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as u32;
        let rgb = linear_of(color);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = x0 + dx * t;
            let y = y0 + dy * t;
            if x < 0.0 || y < 0.0 {
                continue;
            }
            let (ix, iy) = (x as u32, y as u32);
            if ix >= target.width() || iy >= target.height() {
                continue;
            }
            if self.depth_test && z0 > 0.0 {
                let z = z0 + (z1 - z0) * t;
                if target.depth_at(ix, iy).is_some_and(|d| z > d + 1e-4) {
                    continue;
                }
            }
            target.set(ix, iy, rgb);
        }
    }

    /// Draws an outlined or filled screen-space rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn screen_rect(
        &self,
        target: &mut Framebuffer,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: Color8,
        filled: bool,
    ) {
        let rgb = linear_of(color);
        let x1 = (x + width).min(target.width());
        let y1 = (y + height).min(target.height());
        if filled {
            for py in y..y1 {
                for px in x..x1 {
                    target.blend(px, py, rgb, color.a as f32 / 255.0);
                }
            }
            return;
        }
        for px in x..x1 {
            target.blend(px, y, rgb, 1.0);
            if y1 > y {
                target.blend(px, y1 - 1, rgb, 1.0);
            }
        }
        for py in y..y1 {
            target.blend(x, py, rgb, 1.0);
            if x1 > x {
                target.blend(x1 - 1, py, rgb, 1.0);
            }
        }
    }

    /// Draws a string at a screen position.
    pub fn text(&self, target: &mut Framebuffer, x: u32, y: u32, text: &str, color: Color8) {
        self.text_scaled(target, x, y, text, color, self.text_scale);
    }

    /// Draws a string at an explicit scale.
    pub fn text_scaled(
        &self,
        target: &mut Framebuffer,
        x: u32,
        y: u32,
        text: &str,
        color: Color8,
        scale: u32,
    ) {
        let scale = scale.max(1);
        let rgb = linear_of(color);
        let mut pen_x = x;
        for ch in text.chars() {
            if ch == '\n' {
                continue;
            }
            if let Some(glyph) = self.font.glyph(ch) {
                for (gy, row) in glyph.iter().enumerate() {
                    for (gx, ink) in row.iter().enumerate() {
                        if !*ink {
                            continue;
                        }
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = pen_x + gx as u32 * scale + sx;
                                let py = y + gy as u32 * scale + sy;
                                target.blend(px, py, rgb, color.a as f32 / 255.0);
                            }
                        }
                    }
                }
            }
            pen_x += (Font::WIDTH as u32 + Font::SPACING as u32) * scale;
        }
    }

    /// Draws a filled panel of text lines with a border.
    pub fn panel(
        &self,
        target: &mut Framebuffer,
        x: u32,
        y: u32,
        lines: &[String],
        foreground: Color8,
        background: Color8,
    ) {
        let scale = self.text_scale;
        let line_height = Font::text_height(scale) + 2 * scale;
        let widest = lines
            .iter()
            .map(|l| Font::text_width(l, scale))
            .max()
            .unwrap_or(0);
        let width = widest + 6 * scale;
        let height = line_height * lines.len() as u32 + 4 * scale;
        self.screen_rect(target, x, y, width, height, background, true);
        self.screen_rect(target, x, y, width, height, foreground, false);
        for (i, line) in lines.iter().enumerate() {
            let ly = y + 3 * scale + i as u32 * line_height;
            self.text_scaled(target, x + 3 * scale, ly, line, foreground, scale);
        }
    }

    /// Draws a crosshair centred on a screen position.
    pub fn crosshair(&self, target: &mut Framebuffer, x: u32, y: u32, size: u32, color: Color8) {
        for i in 0..size {
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let px = x as i64 + dx * i as i64;
                let py = y as i64 + dy * i as i64;
                if px >= 0
                    && py >= 0
                    && (px as u32) < target.width()
                    && (py as u32) < target.height()
                {
                    target.set(px as u32, py as u32, linear_of(color));
                }
            }
        }
    }

    /// Renders the standard statistics block.
    pub fn stats_panel(&self, target: &mut Framebuffer, lines: &[String]) {
        let foreground = Color8::new(220, 230, 240, 255);
        let background = Color8::new(10, 12, 18, 200);
        self.panel(target, 2, 2, lines, foreground, background);
    }

    /// Fills an axis-aligned rectangle of the screen from a world-space box,
    /// used by the occlusion visualiser.
    pub fn highlight_bounds(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        bounds: &Aabb,
        color: Color8,
    ) {
        self.aabb(target, camera, bounds, color);
    }

    /// Draws a line between two screen points using the camera's projection of
    /// two world points, clipping the near plane.
    pub fn segment(
        &self,
        target: &mut Framebuffer,
        camera: &CameraView,
        a: Vec3,
        b: Vec3,
        color: Color8,
    ) {
        // Clip against the near plane in view space so a segment crossing behind
        // the camera still draws its visible part.
        let view = camera.view;
        let va = view.transform_point3(a);
        let vb = view.transform_point3(b);
        let near = camera.near.max(0.01);
        let (a, b) = if va.z > -near && vb.z > -near {
            return;
        } else if va.z > -near {
            let t = (-near - va.z) / (vb.z - va.z);
            (va.lerp(vb, t), vb)
        } else if vb.z > -near {
            let t = (-near - vb.z) / (va.z - vb.z);
            (va, vb.lerp(va, t))
        } else {
            (va, vb)
        };
        let project = |p: Vec3| -> Option<(f32, f32)> {
            let clip = camera.projection.transform_point4(p.extend(1.0));
            let ndc = clip.perspective_divide()?;
            Some((
                (ndc.x * 0.5 + 0.5) * target.width() as f32,
                (1.0 - (ndc.y * 0.5 + 0.5)) * target.height() as f32,
            ))
        };
        let (Some((ax, ay)), Some((bx, by))) = (project(a), project(b)) else {
            return;
        };
        self.screen_line(target, ax, ay, bx, by, color);
    }

    /// The inverse view-projection of a camera, for picking.
    #[must_use]
    pub fn inverse_view_projection(camera: &CameraView) -> Mat4 {
        camera.inverse_view_projection()
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

/// The linear RGB of an sRGB colour, which is what the framebuffer stores.
#[inline]
fn linear_of(color: Color8) -> [f32; 3] {
    color.to_linear().to_array3()
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Vec2;

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            12.0,
            1.0,
            1.0,
            100.0,
        )
    }

    #[test]
    fn font_loads_glyphs() {
        let f = Font::new();
        assert!(f.glyph_count() >= 40, "{}", f.glyph_count());
        assert!(f.has_glyph('A'));
        assert!(f.has_glyph('0'));
        assert!(f.has_glyph(' '));
        assert!(f.has_glyph('.'));
    }

    #[test]
    fn font_is_case_insensitive() {
        let f = Font::new();
        assert_eq!(f.glyph('a'), f.glyph('A'));
        assert!(f.has_glyph('z'));
    }

    #[test]
    fn font_missing_glyph_is_none() {
        let f = Font::new();
        assert!(f.glyph('\u{4e2d}').is_none());
    }

    #[test]
    fn text_width_math() {
        assert_eq!(Font::text_width("", 1), 0);
        assert_eq!(Font::text_width("A", 1), 3);
        assert_eq!(Font::text_width("AB", 1), 7);
        assert_eq!(Font::text_width("AB", 2), 14);
    }

    #[test]
    fn text_draws_pixels() {
        let mut fb = Framebuffer::new(64, 32);
        fb.clear([0.0, 0.0, 0.0]);
        let o = Overlay::new();
        o.text(&mut fb, 2, 2, "HI", Color8::WHITE);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 5, "the glyphs must put ink on the screen: {lit}");
    }

    #[test]
    fn text_is_exactly_the_requested_colour() {
        let mut fb = Framebuffer::new(32, 16);
        let o = Overlay::new();
        let color = Color8::new(255, 0, 0, 255);
        o.text(&mut fb, 0, 0, "A", color);
        let image = fb.resolve(&crate::framebuffer::ResolveSettings::default());
        let mut found = false;
        for y in 0..16 {
            for x in 0..32 {
                let p = image.get(x, y).unwrap_or(Color8::TRANSPARENT);
                if p.r > 0 {
                    assert_eq!((p.r, p.g, p.b), (255, 0, 0));
                    found = true;
                }
            }
        }
        assert!(found, "the glyph must be drawn");
    }

    #[test]
    fn text_scaled_is_bigger() {
        let mut small = Framebuffer::new(64, 32);
        let mut big = Framebuffer::new(64, 32);
        let o = Overlay::new();
        o.text_scaled(&mut small, 0, 0, "A", Color8::WHITE, 1);
        o.text_scaled(&mut big, 0, 0, "A", Color8::WHITE, 3);
        let count = |fb: &Framebuffer| {
            fb.color_slice()
                .chunks_exact(3)
                .filter(|c| c[0] > 0.5)
                .count()
        };
        assert!(count(&big) > count(&small));
    }

    #[test]
    fn text_advances_even_for_unknown_glyphs() {
        let mut fb = Framebuffer::new(64, 32);
        let o = Overlay::new();
        o.text(&mut fb, 0, 0, "\u{4e2d}A", Color8::WHITE);
        // The 'A' must be drawn one cell to the right of the origin.
        let lit_at_origin = (0..4).any(|x| (0..6).any(|y| fb.get(x, y).unwrap()[0] > 0.5));
        assert!(
            !lit_at_origin,
            "the unknown glyph must leave its cell blank"
        );
        let lit_later = (4..8).any(|x| (0..6).any(|y| fb.get(x, y).unwrap()[0] > 0.5));
        assert!(lit_later, "the A must still be drawn");
    }

    #[test]
    fn project_maps_the_origin_to_the_screen_centre() {
        let fb = Framebuffer::new(64, 64);
        let cam = camera();
        let (x, y, z) = Overlay::project(&cam, &fb, Vec3::ZERO).unwrap();
        assert!(
            (x - 32.0).abs() < 0.5 && (y - 32.0).abs() < 0.5,
            "({x},{y})"
        );
        assert!(z > 0.0 && z < 1.0, "{z}");
    }

    #[test]
    fn project_rejects_points_outside_the_depth_range() {
        let fb = Framebuffer::new(64, 64);
        let mut cam = camera();
        cam.far = 30.0;
        cam.view_projection = cam.projection * cam.view;
        assert!(Overlay::project(&cam, &fb, Vec3::new(0.0, 500.0, 0.0)).is_none());
    }

    #[test]
    fn line_draws_a_stroke() {
        let mut fb = Framebuffer::new(64, 64);
        let o = Overlay::new();
        o.line(
            &mut fb,
            &camera(),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 20, "{lit}");
    }

    #[test]
    fn aabb_draws_twelve_edges() {
        let mut fb = Framebuffer::new(128, 128);
        let o = Overlay::new();
        let cam = CameraView::orthographic(
            Vec3::new(20.0, 20.0, 20.0),
            Vec3::ZERO,
            Vec3::Y,
            30.0,
            1.0,
            1.0,
            200.0,
        );
        o.aabb(
            &mut fb,
            &cam,
            &Aabb::new(Vec3::splat(-2.0), Vec3::splat(2.0)),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 100, "a wireframe cube needs a lot of ink: {lit}");
    }

    #[test]
    fn offscreen_primitives_are_skipped() {
        let mut fb = Framebuffer::new(32, 32);
        let before = fb.color_slice().to_vec();
        let o = Overlay::new();
        o.screen_line(&mut fb, -100.0, -100.0, -50.0, -50.0, Color8::WHITE);
        assert_eq!(fb.color_slice(), before.as_slice());
    }

    #[test]
    fn screen_rect_outline_and_fill() {
        let mut outline = Framebuffer::new(32, 32);
        let mut filled = Framebuffer::new(32, 32);
        let o = Overlay::new();
        o.screen_rect(&mut outline, 4, 4, 10, 10, Color8::WHITE, false);
        o.screen_rect(&mut filled, 4, 4, 10, 10, Color8::WHITE, true);
        let count = |fb: &Framebuffer| {
            fb.color_slice()
                .chunks_exact(3)
                .filter(|c| c[0] > 0.5)
                .count()
        };
        assert!(count(&filled) > count(&outline));
        // The interior of the outline must be empty.
        assert_eq!(outline.get(8, 8).unwrap()[0], 0.0);
        assert!(filled.get(8, 8).unwrap()[0] > 0.5);
    }

    #[test]
    fn screen_rect_clamps_to_the_target() {
        let mut fb = Framebuffer::new(16, 16);
        let o = Overlay::new();
        o.screen_rect(&mut fb, 8, 8, 100, 100, Color8::WHITE, true);
        assert!(fb.get(15, 15).unwrap()[0] > 0.5);
    }

    #[test]
    fn ray_draws_from_the_origin() {
        let mut fb = Framebuffer::new(64, 64);
        let o = Overlay::new();
        o.ray(&mut fb, &camera(), Vec3::ZERO, Vec3::X, 5.0, Color8::WHITE);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 5);
    }

    #[test]
    fn cross_marks_a_point() {
        let mut fb = Framebuffer::new(64, 64);
        let o = Overlay::new();
        o.cross(&mut fb, &camera(), Vec3::ZERO, 1.0, Color8::WHITE);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 5);
    }

    #[test]
    fn crosshair_marks_the_centre() {
        let mut fb = Framebuffer::new(32, 32);
        let o = Overlay::new();
        o.crosshair(&mut fb, 16, 16, 3, Color8::WHITE);
        assert!(fb.get(16, 16).unwrap()[0] > 0.5);
        assert!(fb.get(18, 16).unwrap()[0] > 0.5);
        assert_eq!(fb.get(0, 0).unwrap()[0], 0.0);
    }

    #[test]
    fn panel_sizes_to_its_content() {
        let mut fb = Framebuffer::new(128, 64);
        let o = Overlay::new();
        let lines = vec!["NOXEL".to_string(), "FPS 60".to_string()];
        o.panel(
            &mut fb,
            0,
            0,
            &lines,
            Color8::WHITE,
            Color8::new(0, 0, 0, 255),
        );
        // The background covers the expected area.
        assert!(fb.get(2, 2).unwrap()[0] < 0.1, "inner background");
        assert_eq!(fb.get(120, 60).unwrap()[0], 0.0, "outside the panel");
    }

    #[test]
    fn stats_panel_renders() {
        let mut fb = Framebuffer::new(160, 60);
        let o = Overlay::new();
        o.stats_panel(&mut fb, &["FPS 60".to_string(), "TRIS 1234".to_string()]);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.3)
            .count();
        assert!(lit > 20, "{lit}");
    }

    #[test]
    fn depth_test_hides_lines_behind_geometry() {
        let mut fb = Framebuffer::new(64, 64);
        // Pretend something very close was already drawn everywhere.
        for y in 0..64 {
            for x in 0..64 {
                fb.set_depth(x, y, 0.01);
            }
        }
        let mut o = Overlay::new();
        o.set_depth_test(true);
        o.line(
            &mut fb,
            &camera(),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert_eq!(lit, 0, "the depth test must hide the line");

        o.set_depth_test(false);
        o.line(
            &mut fb,
            &camera(),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 10, "with the test off it must appear");
    }

    #[test]
    fn segment_clips_at_the_near_plane() {
        let mut fb = Framebuffer::new(64, 64);
        let o = Overlay::new();
        let cam = CameraView::perspective(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            1.0,
            100.0,
        );
        // One endpoint is behind the camera.
        o.segment(
            &mut fb,
            &cam,
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 5.0),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 0, "the visible part must still be drawn");
    }

    #[test]
    fn highlight_bounds_is_an_aabb() {
        let mut fb = Framebuffer::new(64, 64);
        let o = Overlay::new();
        o.highlight_bounds(
            &mut fb,
            &camera(),
            &Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)),
            Color8::new(0, 255, 0, 255),
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[1] > 0.5)
            .count();
        assert!(lit > 0);
    }

    #[test]
    fn inverse_view_projection_is_available() {
        let cam = camera();
        let inv = Overlay::inverse_view_projection(&cam);
        let p = Vec3::new(1.0, 0.0, 1.0);
        let clip = cam.view_projection.transform_point4(p.extend(1.0));
        let back = inv.transform_point4(clip).perspective_divide().unwrap();
        assert!(back.approx_eq(p, 1e-3));
    }

    #[test]
    fn overlay_handles_an_empty_string() {
        let mut fb = Framebuffer::new(8, 8);
        let o = Overlay::new();
        o.text(&mut fb, 0, 0, "", Color8::WHITE);
        assert_eq!(fb.get(0, 0).unwrap()[0], 0.0);
    }

    #[test]
    fn text_at_the_edge_does_not_panic() {
        let mut fb = Framebuffer::new(8, 8);
        let o = Overlay::new();
        o.text(&mut fb, 7, 7, "EDGE", Color8::WHITE);
        o.text(&mut fb, 100, 100, "OFFSCREEN", Color8::WHITE);
    }

    #[test]
    fn text_scale_can_be_set_globally() {
        let mut o = Overlay::new();
        assert_eq!(o.text_scale, 1);
        o.set_text_scale(0);
        assert_eq!(o.text_scale, 1, "scale is clamped to at least 1");
        o.set_text_scale(4);
        assert_eq!(o.text_scale, 4);
    }

    #[test]
    fn default_overlay_depth_tests() {
        assert!(Overlay::new().depth_test());
        assert!(Overlay::default().depth_test());
    }

    #[test]
    fn project_handles_a_point_behind_an_ortho_camera() {
        let fb = Framebuffer::new(32, 32);
        let cam = camera();
        // Far above the camera, outside the ortho volume's near plane.
        assert!(Overlay::project(&cam, &fb, Vec3::new(0.0, 25.0, 0.0)).is_none());
    }

    #[test]
    fn vec2_import_is_used() {
        let v = Vec2::new(1.0, 2.0);
        assert_eq!(v.x, 1.0);
    }
}
