//! Bitmap text: an atlas, a face per script, and layout on one baseline.
//!
//! # Why a font atlas instead of a built-in font
//!
//! `noxel_render::overlay` carries a 3x5 uppercase font. That is the right
//! trade for a debug readout and the wrong one for a game: it has no lowercase,
//! it cannot draw a Chinese character, and 3x5 is not enough to tell `5` from
//! `S` at a glance in a menu.
//!
//! A [`FontSet`] is loaded from an atlas baked by `tools/fontgen`. Two ideas
//! make it more than a bitmap lookup:
//!
//! * **Faces fall back.** A glyph is looked up in each face in priority order,
//!   so one `FontSet` holds a hand-drawn Latin face and a rasterised Chinese
//!   face, and a string mixing the two draws correctly without the caller
//!   knowing which characters belong to which.
//! * **The line box is derived, not declared.** A mixed string sets its baseline
//!   to the deepest face it used and its line height to the tallest, so Latin
//!   and Chinese sit on one baseline instead of two. A caller that fixes the
//!   line height up front has to know the content first, which is exactly what
//!   an immediate-mode UI cannot do.
//!
//! # Crispness
//!
//! Glyphs are drawn as opaque or transparent — there is no coverage blending of
//! the glyph shape itself — at integer positions and integer scales. The atlas
//! is sampled nearest-neighbour. Those three together are what keep upscaled
//! text sharp; an antialiased glyph edge at 320x180 upscaled 6x is a grey blur
//! six pixels wide.
//!
//! # Not supported
//!
//! Bidirectional text, shaping and ligatures. Each glyph is drawn where the
//! previous one ended, which is correct for Latin, Greek, Cyrillic and CJK and
//! wrong for Arabic and Devanagari. A game that needs those brings a shaped
//! glyph list, not a font.

use std::collections::HashMap;

use noxel_asset::image::Image;
use noxel_asset::json::{JsonValue, parse_str};
use noxel_core::math::Color8;

use crate::geom::UiRect;
use crate::painter::Painter;

/// Why a font failed to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontError {
    /// The atlas JSON could not be parsed.
    Json(String),
    /// The JSON parsed but does not describe a font.
    Shape(String),
    /// The atlas image or its metrics could not be read from disk.
    Io(String),
}

impl core::fmt::Display for FontError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Json(message) => write!(f, "font atlas JSON: {message}"),
            Self::Shape(message) => write!(f, "font atlas is malformed: {message}"),
            Self::Io(message) => write!(f, "font atlas file: {message}"),
        }
    }
}

impl std::error::Error for FontError {}

/// One glyph's place in the atlas and how far the pen moves after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// The glyph's ink box inside the atlas texture.
    pub rect: UiRect,
    /// Where to put the ink box, relative to the pen at the **top of the
    /// face's own line box**.
    ///
    /// Measured from the face's line box and not from the shared baseline,
    /// because [`crate::font::FontSet::draw_line`] shifts a glyph by
    /// `line_baseline - face_baseline` to put every face on one baseline. A
    /// bake that folded the face's baseline into this offset would have that
    /// shift applied twice, and Latin text would sit below the Chinese it is
    /// supposed to align with.
    pub offset: (i32, i32),
    /// Horizontal pen movement after drawing, in unscaled pixels.
    pub advance: i32,
}

/// A set of glyphs sharing one size and one baseline.
#[derive(Clone, Debug)]
pub struct Face {
    /// The face's name, as the atlas named it.
    pub name: String,
    /// The face's own line box height.
    pub line_height: u32,
    /// Distance from the top of that line box down to the baseline.
    pub baseline: u32,
    glyphs: HashMap<char, Glyph>,
}

impl Face {
    /// The glyph for `ch`, if this face has one.
    #[must_use]
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.get(&ch)
    }

    /// How many glyphs the face holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Whether the face holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// The box a line of text occupies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineMetrics {
    /// Total pen advance.
    pub width: u32,
    /// Height of the line box.
    pub line_height: u32,
    /// Distance from the top of the line box to the baseline.
    pub baseline: u32,
}

/// How a run of text is aligned inside the rectangle it is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// Against the left edge.
    #[default]
    Left,
    /// Centred in the rectangle.
    Center,
    /// Against the right edge.
    Right,
}

/// How a run of text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    /// The ink colour. The alpha is the ink's alpha.
    pub color: Color8,
    /// Integer pixel scale. `1` is the atlas size; `2` doubles every pixel.
    pub scale: u32,
    /// Horizontal alignment inside the target rectangle.
    pub align: TextAlign,
    /// Where the text wraps.
    pub wrap: Wrap,
    /// Extra pixels between lines.
    pub line_spacing: i32,
    /// Draw a one-pixel drop shadow in this colour before the ink.
    ///
    /// A shadow is not decoration here: UI text sits on top of a world whose
    /// brightness changes with the time of day, and dark text on a dark field is
    /// unreadable without one.
    pub shadow: Option<Color8>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: Color8::WHITE,
            scale: 1,
            align: TextAlign::Left,
            wrap: Wrap::None,
            line_spacing: 1,
            shadow: None,
        }
    }
}

impl TextStyle {
    /// A style with a colour.
    #[must_use]
    pub fn new(color: Color8) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }

    /// Sets the integer scale.
    #[must_use]
    pub fn with_scale(mut self, scale: u32) -> Self {
        self.scale = scale.max(1);
        self
    }

    /// Sets the alignment.
    #[must_use]
    pub fn with_align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// Sets the wrap mode.
    #[must_use]
    pub fn with_wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = wrap;
        self
    }

    /// Adds a one-pixel drop shadow.
    #[must_use]
    pub fn with_shadow(mut self, color: Color8) -> Self {
        self.shadow = Some(color);
        self
    }
}

/// Where a run of text is allowed to break.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wrap {
    /// Never break; a line is as wide as it needs to be.
    #[default]
    None,
    /// Break at or before this width in pixels.
    Width(u32),
}

/// The pen advance for a character no face can draw, in unscaled pixels.
///
/// Measurement and drawing must agree on this. If the layout counted a missing
/// character as zero width and the renderer advanced the pen by three, every
/// line containing one would be measured short and drawn long — the same shape
/// of bug as a font that counts bytes instead of characters, and just as hard to
/// see until two strings overlap.
pub const MISSING_ADVANCE: i32 = 3;

/// A font: an atlas texture plus an ordered list of faces to look glyphs up in.
#[derive(Clone, Debug)]
pub struct FontSet {
    texture: Image,
    faces: Vec<Face>,
    fallback: Vec<usize>,
}

impl FontSet {
    /// Loads a font from an atlas image and its metrics JSON.
    ///
    /// # Errors
    /// Returns [`FontError::Json`] if the JSON does not parse, and
    /// [`FontError::Shape`] if it parses but is not a font atlas.
    pub fn from_json(texture: Image, json: &JsonValue) -> Result<Self, FontError> {
        let declared = json
            .get("atlas")
            .ok_or_else(|| FontError::Shape("no `atlas` object".into()))?;
        let declared_width = declared.get_u32("width", 0);
        let declared_height = declared.get_u32("height", 0);
        if declared_width != texture.width() || declared_height != texture.height() {
            return Err(FontError::Shape(format!(
                "atlas is {}x{} but the metrics say {}x{}",
                texture.width(),
                texture.height(),
                declared_width,
                declared_height
            )));
        }

        let face_list = json
            .get_array("faces")
            .ok_or_else(|| FontError::Shape("no `faces` array".into()))?;
        if face_list.is_empty() {
            return Err(FontError::Shape("`faces` is empty".into()));
        }

        let mut faces = Vec::with_capacity(face_list.len());
        for entry in face_list {
            let name = entry.get_str("name").unwrap_or("unnamed").to_string();
            let line_height = entry.get_u32("line_height", 0);
            let baseline = entry.get_u32("baseline", 0);
            if line_height == 0 {
                return Err(FontError::Shape(format!(
                    "face {name:?} has no line height"
                )));
            }
            if baseline > line_height {
                return Err(FontError::Shape(format!(
                    "face {name:?} has baseline {baseline} below its line height {line_height}"
                )));
            }
            let table = entry
                .get("glyphs")
                .and_then(|g| g.as_object())
                .ok_or_else(|| FontError::Shape(format!("face {name:?} has no `glyphs`")))?;

            let mut glyphs = HashMap::with_capacity(table.len());
            for (key, value) in table {
                let Some(ch) = key.chars().next() else {
                    continue;
                };
                let Some(rect) = value.get("rect").and_then(|r| r.as_array()) else {
                    continue;
                };
                let Some(offset) = value.get("offset").and_then(|r| r.as_array()) else {
                    continue;
                };
                if rect.len() < 4 || offset.len() < 2 {
                    return Err(FontError::Shape(format!(
                        "glyph {ch:?} in {name:?} is short"
                    )));
                }
                let number = |value: &JsonValue| value.as_i64().unwrap_or(0) as i32;
                glyphs.insert(
                    ch,
                    Glyph {
                        rect: UiRect::new(
                            number(&rect[0]),
                            number(&rect[1]),
                            number(&rect[2]).max(0) as u32,
                            number(&rect[3]).max(0) as u32,
                        ),
                        offset: (number(&offset[0]), number(&offset[1])),
                        advance: value.get_i32("advance", 0).max(0),
                    },
                );
            }
            faces.push(Face {
                name,
                line_height,
                baseline,
                glyphs,
            });
        }

        // The fallback order is by face *name* in the JSON, because an array of
        // names survives a human editing the file more gracefully than indices.
        let fallback = match json.get_array("fallback") {
            Some(names) => {
                let mut order = Vec::with_capacity(names.len());
                for name in names {
                    let Some(name) = name.as_str() else { continue };
                    if let Some(index) = faces.iter().position(|f| f.name == name) {
                        order.push(index);
                    }
                }
                order
            }
            None => Vec::new(),
        };
        let fallback = if fallback.is_empty() {
            (0..faces.len()).collect()
        } else {
            fallback
        };

        Ok(Self {
            texture,
            faces,
            fallback,
        })
    }

    /// A font with no glyphs at all.
    ///
    /// What a game uses when its font bake is missing. Text then measures and
    /// lays out correctly and draws nothing, which is a conspicuously empty
    /// interface rather than a crash — and the game is expected to say loudly in
    /// its log *why* it is empty. A silent blank screen with no explanation is
    /// the failure mode this exists to avoid.
    #[must_use]
    pub fn blank() -> Self {
        Self {
            texture: Image::transparent(1, 1),
            faces: vec![Face {
                name: "blank".to_string(),
                line_height: 8,
                baseline: 6,
                glyphs: HashMap::new(),
            }],
            fallback: vec![0],
        }
    }

    /// Loads a font from an atlas PNG and a metrics JSON on disk.
    ///
    /// # Errors
    /// Returns [`FontError::Io`] for a missing or unreadable file, and the same
    /// errors as [`FontSet::from_json`] for a malformed one.
    pub fn load(
        png: impl AsRef<std::path::Path>,
        json: impl AsRef<std::path::Path>,
    ) -> Result<Self, FontError> {
        let texture = Image::load_png(png).map_err(|e| FontError::Io(e.to_string()))?;
        let text = std::fs::read_to_string(json).map_err(|e| FontError::Io(e.to_string()))?;
        let parsed = parse_str(&text).map_err(|e| FontError::Json(e.to_string()))?;
        Self::from_json(texture, &parsed)
    }

    /// The atlas texture every glyph is cut from.
    #[must_use]
    pub fn texture(&self) -> &Image {
        &self.texture
    }

    /// The faces, in the order the atlas declared them.
    #[must_use]
    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    /// The face that will draw `ch`, and its glyph.
    ///
    /// `None` means no face has the character. A caller draws a fallback mark
    /// rather than silently skipping, because a missing glyph that renders as
    /// nothing looks like a layout bug and is actually a font bug.
    #[must_use]
    pub fn glyph(&self, ch: char) -> Option<(&Face, &Glyph)> {
        for index in &self.fallback {
            let face = self.faces.get(*index)?;
            if let Some(glyph) = face.glyph(ch) {
                return Some((face, glyph));
            }
        }
        None
    }

    /// Whether any face can draw `ch`.
    #[must_use]
    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    /// The box one line of text occupies.
    ///
    /// A `\n` is not special here: measurement is per line, and
    /// [`FontSet::layout`] is what splits a string into lines.
    #[must_use]
    pub fn line_metrics(&self, text: &str, scale: u32) -> LineMetrics {
        let scale = scale.max(1);
        // Only faces that actually drew something contribute, so a Latin-only
        // string does not inherit a Chinese face's taller line box.
        let mut deepest = 0u32;
        let mut tallest = 0u32;
        let mut width = 0i64;
        let mut any = false;
        for ch in text.chars() {
            match self.glyph(ch) {
                Some((face, glyph)) => {
                    any = true;
                    deepest = deepest.max(face.baseline);
                    tallest = tallest.max(face.line_height);
                    width += i64::from(glyph.advance);
                }
                // A missing glyph still takes space, and takes exactly as much
                // as `draw_line` will advance it by.
                None => width += i64::from(MISSING_ADVANCE),
            }
        }
        if !any {
            let face = &self.faces[0];
            return LineMetrics {
                width: 0,
                line_height: face.line_height * scale,
                baseline: face.baseline * scale,
            };
        }
        LineMetrics {
            width: (width.max(0) as u32) * scale,
            line_height: tallest * scale,
            baseline: deepest * scale,
        }
    }

    /// The size of a run of text, honouring newlines and wrapping.
    #[must_use]
    pub fn measure(&self, text: &str, style: &TextStyle) -> (u32, u32) {
        let lines = self.layout(text, style);
        let mut width = 0;
        let mut height = 0i64;
        for line in &lines {
            let metrics = self.line_metrics(line, style.scale);
            width = width.max(metrics.width);
            height += i64::from(metrics.line_height) + i64::from(style.line_spacing);
        }
        if height > 0 {
            height -= i64::from(style.line_spacing);
        }
        (width, height.max(0) as u32)
    }

    /// Splits `text` into the lines it will be drawn as.
    ///
    /// Breaks at `\n` always. With [`Wrap::Width`] it also breaks at spaces, and
    /// — this is the part a Latin-only word wrapper gets wrong — **between any
    /// two CJK characters**, because Chinese has no spaces and a wrapper that
    /// only looks for them will overflow the panel rather than wrap.
    #[must_use]
    pub fn layout<'a>(&self, text: &'a str, style: &TextStyle) -> Vec<&'a str> {
        let mut out = Vec::new();
        for paragraph in text.split('\n') {
            match style.wrap {
                Wrap::None => out.push(paragraph),
                Wrap::Width(limit) => self.wrap_paragraph(paragraph, limit, style.scale, &mut out),
            }
        }
        if out.is_empty() {
            out.push("");
        }
        out
    }

    fn wrap_paragraph<'a>(&self, text: &'a str, limit: u32, scale: u32, out: &mut Vec<&'a str>) {
        if text.is_empty() {
            out.push(text);
            return;
        }
        let limit = i64::from(limit.max(1));
        let scale = i64::from(scale.max(1));
        let advance = |ch: char| {
            i64::from(self.glyph(ch).map_or(MISSING_ADVANCE, |(_, g)| g.advance)) * scale
        };

        let mut line_start = 0usize;
        let mut width = 0i64;
        let mut previous_was_space = false;

        for (index, ch) in text.char_indices() {
            let step = advance(ch);
            // A break is allowed before a space-delimited word, and between any
            // two non-ASCII characters. That second rule is the one that matters
            // for Chinese: there are no spaces to break at, so a wrapper that
            // only looks for whitespace overflows the panel instead of wrapping.
            let breakable = index > line_start && (!ch.is_ascii() || previous_was_space);
            // A run with no break in it at all — one long URL, one long ASCII
            // word — still has to break once it has filled the line, or it
            // overflows. Character-level breaking is the fallback of last
            // resort, not the normal path.
            let must_break = breakable || width >= limit;

            if must_break && width + step > limit {
                let mut end = index;
                // A line never ends in the space it broke at…
                while end > line_start && text[..end].ends_with(' ') {
                    end -= 1;
                }
                out.push(&text[line_start..end]);
                // …and the next line never begins with one.
                let mut next = index;
                while text[next..].starts_with(' ') {
                    next += 1;
                }
                line_start = next;
                width = 0;
                if next > index {
                    // The character we broke at was a space, and has been
                    // consumed as the break rather than as the start of a line.
                    previous_was_space = false;
                    continue;
                }
            }
            width += step;
            previous_was_space = ch == ' ';
        }
        out.push(&text[line_start..]);
    }

    /// Draws text inside `rect`, wrapping and aligning to it.
    ///
    /// Returns the height actually used, so a caller can stack the next element
    /// under it without measuring twice.
    pub fn draw_text(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        text: &str,
        style: &TextStyle,
    ) -> u32 {
        let scale = style.scale.max(1);
        let lines = self.layout(text, style);
        let mut y = rect.y;
        for line in &lines {
            let metrics = self.line_metrics(line, scale);
            let x = match style.align {
                TextAlign::Left => rect.x,
                TextAlign::Center => rect.x + (rect.w as i32 - metrics.width as i32) / 2,
                TextAlign::Right => rect.right() - metrics.width as i32,
            };
            if let Some(shadow) = style.shadow {
                self.draw_line(
                    painter,
                    x + scale as i32,
                    y + scale as i32,
                    line,
                    shadow,
                    scale,
                );
            }
            self.draw_line(painter, x, y, line, style.color, scale);
            y += metrics.line_height as i32 + style.line_spacing;
        }
        (y - rect.y - style.line_spacing).max(0) as u32
    }

    /// Draws one line at a pen position, with no wrapping or alignment.
    ///
    /// `y` is the **top of the line box**, not the baseline. That is the choice
    /// that lets a caller stack lines by `line_height` without knowing which
    /// faces the text will reach for.
    pub fn draw_line(
        &self,
        painter: &mut Painter<'_>,
        x: i32,
        y: i32,
        text: &str,
        color: Color8,
        scale: u32,
    ) -> u32 {
        let scale = scale.max(1) as i32;
        let metrics = self.line_metrics(text, scale as u32);
        let pen_start = x;
        let mut pen = x;
        for ch in text.chars() {
            let Some((face, glyph)) = self.glyph(ch) else {
                // Keep the pen moving for an unknown glyph so a missing
                // character does not collapse the rest of the line.
                pen += 3 * scale;
                continue;
            };
            if !glyph.rect.is_empty() {
                // The glyph's offset is measured from the top of *its* face's
                // line box; shift it down so its baseline lands on the line's.
                let dy =
                    metrics.baseline as i32 - face.baseline as i32 * scale + glyph.offset.1 * scale;
                let dx = glyph.offset.0 * scale;
                painter.blit_scaled(
                    &self.texture,
                    glyph.rect,
                    pen + dx,
                    y + dy,
                    scale as u32,
                    color,
                );
            }
            pen += glyph.advance * scale;
        }
        (pen - pen_start).max(0) as u32
    }

    /// A one-line convenience that draws at a top-left corner.
    pub fn draw_simple(
        &self,
        painter: &mut Painter<'_>,
        x: i32,
        y: i32,
        text: &str,
        color: Color8,
        scale: u32,
    ) {
        self.draw_line(painter, x, y, text, color, scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testfont::test_font;

    #[test]
    fn line_metrics_draw_their_width_from_advances_not_glyph_boxes() {
        let font = test_font();
        // Every test glyph advances 5 and is 4 wide, so three characters are 15
        // wide with no space between them.
        let metrics = font.line_metrics("aaa", 1);
        assert_eq!(metrics.width, 15);
        assert_eq!(metrics.line_height, 7);
        assert_eq!(metrics.baseline, 5);
    }

    #[test]
    fn scaling_multiplies_every_metric() {
        let font = test_font();
        let one = font.line_metrics("aaa", 1);
        let three = font.line_metrics("aaa", 3);
        assert_eq!(three.width, one.width * 3);
        assert_eq!(three.line_height, one.line_height * 3);
        assert_eq!(three.baseline, one.baseline * 3);
    }

    #[test]
    fn a_missing_glyph_advances_the_pen_and_does_not_collapse_the_line() {
        let font = test_font();
        // The test font has no 'Ω'; the line after it must still be wider than
        // the line before it, or a missing character silently eats the rest.
        let without = font.line_metrics("aa", 1).width;
        let with = font.line_metrics("aΩa", 1).width;
        assert!(with > without, "{with} should exceed {without}");
    }

    #[test]
    fn wrapping_breaks_between_cjk_characters() {
        let font = test_font();
        // Chinese has no spaces, so a wrapper that only breaks on whitespace
        // returns one over-long line here instead of wrapping.
        let style = TextStyle::new(Color8::WHITE).with_wrap(Wrap::Width(24));
        let lines = font.layout("字字字字", &style);
        assert_eq!(lines, vec!["字字", "字字"]);
        assert_eq!(
            lines.concat(),
            "字字字字",
            "wrapping must not lose or duplicate text"
        );
    }

    #[test]
    fn a_word_with_no_break_in_it_still_wraps_rather_than_overflowing() {
        let font = test_font();
        let style = TextStyle::new(Color8::WHITE).with_wrap(Wrap::Width(10));
        let lines = font.layout("aaaaaa", &style);
        assert_eq!(lines, vec!["aa", "aa", "aa"]);
        for line in &lines {
            assert!(
                font.line_metrics(line, 1).width <= 10,
                "{line:?} overflows the limit"
            );
        }
    }

    #[test]
    fn wrapping_prefers_a_space_over_a_mid_word_break() {
        let font = test_font();
        let style = TextStyle::new(Color8::WHITE).with_wrap(Wrap::Width(16));
        // "aaa aaa" is 18 wide with the space, so it must break — and the break
        // belongs at the space, not after the fourth character.
        assert_eq!(font.layout("aaa aaa", &style), vec!["aaa", "aaa"]);
    }

    #[test]
    fn a_mixed_line_shares_one_baseline_and_the_taller_line_box() {
        let font = test_font();
        // Latin is 7 tall on a baseline of 5; the CJK face is 9 tall on 7. The
        // line has to take the deeper baseline and the taller box, or the
        // Chinese characters sit above the Latin instead of beside it.
        let mixed = font.line_metrics("a字", 1);
        assert_eq!(mixed.line_height, 9);
        assert_eq!(mixed.baseline, 7);
        assert_eq!(mixed.width, 5 + 12);
        // A Latin-only line does not inherit the Chinese face's taller box.
        assert_eq!(font.line_metrics("aa", 1).line_height, 7);
    }

    #[test]
    fn newlines_split_lines_and_empty_input_is_one_empty_line() {
        let font = test_font();
        let style = TextStyle::default();
        assert_eq!(font.layout("a\nb", &style), vec!["a", "b"]);
        assert_eq!(font.layout("", &style), vec![""]);
    }

    #[test]
    fn measure_stacks_line_heights_without_a_trailing_gap() {
        let font = test_font();
        let style = TextStyle::default().with_wrap(Wrap::None);
        let (_, height) = font.measure("a\na", &style);
        // Two 7px lines plus one 1px gap between them, and none after the last.
        assert_eq!(height, 7 + 1 + 7);
    }

    #[test]
    fn a_face_with_a_baseline_below_its_line_height_is_rejected() {
        let image = Image::new(8, 8, Color8::TRANSPARENT);
        let json = parse_str(
            r#"{"atlas":{"width":8,"height":8},"faces":[{"name":"bad","line_height":4,"baseline":9,"glyphs":{}}]}"#,
        )
        .unwrap();
        assert!(matches!(
            FontSet::from_json(image, &json),
            Err(FontError::Shape(_))
        ));
    }

    #[test]
    fn a_mismatched_atlas_size_is_rejected() {
        // The metrics claim a different texture size, which would silently
        // sample the wrong rectangle for every glyph.
        let image = Image::new(8, 8, Color8::TRANSPARENT);
        let json = parse_str(
            r#"{"atlas":{"width":64,"height":8},"faces":[{"name":"f","line_height":4,"baseline":2,"glyphs":{}}]}"#,
        )
        .unwrap();
        assert!(matches!(
            FontSet::from_json(image, &json),
            Err(FontError::Shape(_))
        ));
    }
}
