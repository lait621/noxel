//! A font that exists only in memory, so widget tests need no asset files.
//!
//! The real font is a baked atlas on disk, and a test that loaded it would fail
//! on a fresh checkout before `tools/fontgen` had ever run. This builds the same
//! data structure in a few lines: two faces of known size, every ASCII character
//! drawn as a solid block, and one Chinese character on a taller face.
//!
//! The metrics are chosen so a test can assert exact numbers rather than
//! "greater than zero": a Latin glyph advances 5 and is 4 wide, the space
//! advances 3, the CJK glyph advances 12, and the two faces disagree about their
//! line box on purpose — that disagreement is what the mixed-baseline test is
//! about.

use noxel_asset::image::Image;
use noxel_asset::json::{JsonValue, parse_str};
use noxel_core::math::Color8;

use crate::font::FontSet;

/// Latin advance, in pixels.
pub const LATIN_ADVANCE: i32 = 5;
/// Latin cell width, in pixels.
pub const LATIN_WIDTH: u32 = 4;
/// Latin line box height.
pub const LATIN_LINE_HEIGHT: u32 = 7;
/// Latin distance from the line top to the baseline.
pub const LATIN_BASELINE: u32 = 5;
/// The space's advance, which is deliberately not the same as a letter's.
pub const SPACE_ADVANCE: i32 = 3;
/// CJK advance, in pixels.
pub const CJK_ADVANCE: i32 = 12;
/// CJK line box height, taller than the Latin face.
pub const CJK_LINE_HEIGHT: u32 = 9;
/// CJK distance from the line top to the baseline, deeper than the Latin face.
pub const CJK_BASELINE: u32 = 7;
/// The one Chinese character the test font knows.
pub const CJK_CHAR: char = '字';

/// Builds the in-memory font.
///
/// # Panics
/// Panics if the hand-built JSON is malformed, which would be a bug in this
/// function rather than in the code under test.
#[must_use]
pub fn test_font() -> FontSet {
    let atlas = Image::new(32, 16, Color8::WHITE);

    let mut latin = Vec::new();
    for code in 32u32..127 {
        let ch = char::from_u32(code).expect("printable ASCII is valid");
        // The space has no ink but still moves the pen, which is the one case
        // that separates "advance" from "width".
        let (rect, advance) = if ch == ' ' {
            (
                JsonValue::array([0, 0, 0, 0].map(JsonValue::from)),
                SPACE_ADVANCE,
            )
        } else {
            (
                JsonValue::array([
                    JsonValue::from(0),
                    JsonValue::from(0),
                    JsonValue::from(LATIN_WIDTH),
                    JsonValue::from(LATIN_LINE_HEIGHT),
                ]),
                LATIN_ADVANCE,
            )
        };
        latin.push((
            ch.to_string(),
            JsonValue::object([
                ("rect", rect),
                (
                    "offset",
                    JsonValue::array([JsonValue::from(0), JsonValue::from(0)]),
                ),
                ("advance", JsonValue::from(advance)),
            ]),
        ));
    }

    let cjk = vec![(
        CJK_CHAR.to_string(),
        JsonValue::object([
            (
                "rect",
                JsonValue::array([
                    JsonValue::from(8),
                    JsonValue::from(0),
                    JsonValue::from(CJK_ADVANCE as u32),
                    JsonValue::from(CJK_LINE_HEIGHT),
                ]),
            ),
            (
                "offset",
                JsonValue::array([JsonValue::from(0), JsonValue::from(0)]),
            ),
            ("advance", JsonValue::from(CJK_ADVANCE)),
        ]),
    )];

    let document = JsonValue::object([
        (
            "atlas",
            JsonValue::object([
                ("width", JsonValue::from(atlas.width())),
                ("height", JsonValue::from(atlas.height())),
            ]),
        ),
        (
            "faces",
            JsonValue::array([
                JsonValue::object([
                    ("name", JsonValue::from("pixel")),
                    ("line_height", JsonValue::from(LATIN_LINE_HEIGHT)),
                    ("baseline", JsonValue::from(LATIN_BASELINE)),
                    ("glyphs", JsonValue::Object(latin)),
                ]),
                JsonValue::object([
                    ("name", JsonValue::from("cjk")),
                    ("line_height", JsonValue::from(CJK_LINE_HEIGHT)),
                    ("baseline", JsonValue::from(CJK_BASELINE)),
                    ("glyphs", JsonValue::Object(cjk)),
                ]),
            ]),
        ),
        (
            "fallback",
            JsonValue::array([JsonValue::from("pixel"), JsonValue::from("cjk")]),
        ),
    ]);

    // Round-tripping through the parser rather than calling `from_json` on the
    // builder exercises the same path the game takes, so a test cannot pass on a
    // structure the real loader would reject.
    let text = document.to_string();
    let parsed = parse_str(&text).expect("the test font JSON must parse");
    FontSet::from_json(atlas, &parsed).expect("the test font must load")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::LineMetrics;

    #[test]
    fn the_test_font_has_both_faces() {
        let font = test_font();
        assert_eq!(font.faces().len(), 2);
        assert!(font.has_glyph('A'));
        assert!(font.has_glyph(CJK_CHAR));
        // A character neither face knows is what the missing-glyph tests use.
        assert!(!font.has_glyph('Ω'));
    }

    #[test]
    fn the_metrics_are_what_the_tests_claim() {
        let font = test_font();
        assert_eq!(
            font.line_metrics("a", 1),
            LineMetrics {
                width: LATIN_ADVANCE as u32,
                line_height: LATIN_LINE_HEIGHT,
                baseline: LATIN_BASELINE
            }
        );
        assert_eq!(font.line_metrics(" ", 1).width, SPACE_ADVANCE as u32);
        assert_eq!(
            font.line_metrics(&CJK_CHAR.to_string(), 1),
            LineMetrics {
                width: CJK_ADVANCE as u32,
                line_height: CJK_LINE_HEIGHT,
                baseline: CJK_BASELINE
            }
        );
    }
}
