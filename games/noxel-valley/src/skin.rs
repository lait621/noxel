//! The game's look: a theme built from the UI atlas.
//!
//! The engine's [`Theme`] works with no art at all, which is what lets the game
//! run before `noxel-gen farm` has ever been executed. This module is the
//! upgrade path: it takes the flat default theme and replaces each frame with a
//! nine-slice cut from `farm/ui.png`, leaving anything the atlas does not have
//! alone.
//!
//! Writing it as an upgrade rather than a replacement is the point. A missing
//! region degrades one widget to a flat box; it does not blank the UI, and it
//! does not need a second code path to test.

use noxel_asset::atlas::Atlas;
use noxel_core::math::Color8;
use noxel_ui::{
    BarStyle, ButtonStyle, FrameStyle, Insets, Metrics, Palette, Theme, UiRect, lighten, mix,
};

/// The nine-slice border width every frame in the UI atlas is drawn with.
///
/// Four pixels, and it is the same four for every frame, so a `FrameStyle` can
/// be built from a region name without also carrying a per-frame inset table
/// that could disagree with the art.
pub const FRAME_INSET: i32 = 4;

/// Builds the game's theme.
#[must_use]
pub fn build(ui: Option<&Atlas>) -> Theme {
    let mut theme = Theme::default();

    // A farming game is read for hours; the palette leans warm and keeps text
    // near-white so it stays legible over a green field.
    theme.palette = Palette {
        text: Color8::new(244, 238, 222, 255),
        text_dim: Color8::new(166, 158, 140, 255),
        text_strong: Color8::new(255, 250, 232, 255),
        text_on_accent: Color8::new(48, 34, 18, 255),
        accent: Color8::new(244, 190, 88, 255),
        danger: Color8::new(220, 92, 78, 255),
        good: Color8::new(132, 198, 104, 255),
        shadow: Color8::new(14, 11, 18, 170),
        scrim: Color8::new(10, 8, 16, 165),
        surface: Color8::new(66, 52, 44, 242),
        surface_sunken: Color8::new(38, 29, 26, 242),
        surface_raised: Color8::new(104, 82, 60, 255),
        border: Color8::new(32, 24, 20, 255),
    };
    theme.metrics = Metrics {
        gap: 2,
        gap_large: 6,
        padding: Insets::symmetric(5, 4),
        row_height: 13,
        button_height: 15,
        slot_size: 20,
        border: 1,
        bar_height: 8,
    };
    // The tooltip frame in the atlas is a light parchment: text on it has to be
    // dark, or the tooltip is a blank cream box. This is the one place the
    // palette's "body text is light" rule inverts, so it is stated here rather
    // than derived.
    theme.tooltip_text = Color8::new(54, 38, 24, 255);

    // Every frame whose region exists becomes a nine-slice; a missing one falls
    // back to a flat box in the **game's** palette. Reading `theme.panel.fill`
    // here instead would use the fill `Theme::default()` computed from the
    // *default* palette, and every panel would draw in the engine's colours
    // rather than the game's — which is exactly the bug this line fixes.
    theme.panel = sliced(ui, "panel", theme.palette.surface, theme.palette.border)
        .with_shadow(theme.palette.shadow, (2, 2));
    theme.sunken = sliced(
        ui,
        "slot",
        theme.palette.surface_sunken,
        theme.palette.border,
    );
    theme.tooltip = sliced(
        ui,
        "tooltip",
        Color8::new(32, 24, 22, 246),
        theme.palette.accent,
    )
    .with_shadow(theme.palette.shadow, (2, 2));

    theme.button = ButtonStyle {
        idle: sliced(
            ui,
            "button",
            theme.palette.surface_raised,
            theme.palette.border,
        ),
        hover: sliced(
            ui,
            "button_hover",
            lighten(theme.palette.surface_raised, 20),
            theme.palette.accent,
        ),
        pressed: sliced(
            ui,
            "button_press",
            mix(theme.palette.surface_raised, Color8::BLACK, 0.25),
            theme.palette.accent,
        ),
        disabled: FrameStyle::flat(
            mix(theme.palette.surface_raised, Color8::BLACK, 0.45),
            mix(theme.palette.border, Color8::BLACK, 0.2),
        ),
        text: theme.palette.text,
        text_hover: theme.palette.text_strong,
        text_disabled: theme.palette.text_dim,
        padding: Insets::symmetric(5, 3),
    };

    theme.bar = BarStyle {
        trough: FrameStyle::flat(theme.palette.surface_sunken, theme.palette.border),
        fill: theme.palette.good,
        fill_low: theme.palette.danger,
        low_threshold: 0.25,
        highlight: Color8::new(255, 255, 255, 36),
    };

    theme
}

/// A nine-slice style cut from a named region, or a flat one if it is missing.
fn sliced(ui: Option<&Atlas>, name: &str, fill: Color8, border: Color8) -> FrameStyle {
    match ui.and_then(|atlas| atlas.region(name)) {
        Some(region) => {
            let rect = region.pixel_rect;
            FrameStyle::sliced(
                UiRect::new(
                    rect.min.x as i32,
                    rect.min.y as i32,
                    rect.width().max(0.0) as u32,
                    rect.height().max(0.0) as u32,
                ),
                Insets::all(FRAME_INSET),
            )
        }
        // No art: a flat box in the same colours, so the layout is identical and
        // only the decoration is missing.
        None => FrameStyle::flat(fill, border),
    }
}

/// The colour of a ripe crop in the field, for a marker the player can see.
#[must_use]
pub fn ripe_marker() -> Color8 {
    Color8::new(255, 240, 150, 255)
}

/// Moves a colour towards grey, for a seed packet that should read as "not the
/// vegetable".
#[must_use]
pub fn desaturate(color: Color8, amount: f32) -> Color8 {
    let grey = (u32::from(color.r) * 30 + u32::from(color.g) * 59 + u32::from(color.b) * 11) / 100;
    mix(
        color,
        Color8::new(grey as u8, grey as u8, grey as u8, color.a),
        amount,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_text_is_readable_on_the_tooltip_frame() {
        // The tooltip is the one surface whose text is dark on light; a
        // regression here shows up as an empty cream box with the words
        // invisible inside it.
        let theme = build(None);
        let luminance =
            |c: Color8| 0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b);
        assert!(
            luminance(theme.tooltip_text) < 110.0,
            "tooltip text is too light for a parchment frame"
        );
    }

    #[test]
    fn the_flat_fallback_uses_the_game_palette_not_the_engine_default() {
        // These are different colours, and using the wrong one is invisible in a
        // unit test that only checks "something was drawn".
        let theme = build(None);
        let engine_default = Palette::default().surface;
        assert_ne!(
            theme.palette.surface, engine_default,
            "the test is vacuous otherwise"
        );
        assert_eq!(theme.panel.fill, theme.palette.surface);
        assert_eq!(theme.sunken.fill, theme.palette.surface_sunken);
        assert_eq!(theme.button.idle.fill, theme.palette.surface_raised);
        assert_eq!(theme.button.idle.border, theme.palette.border);
    }

    #[test]
    fn the_theme_works_with_no_atlas_at_all() {
        // This is the path every test and every first run takes.
        let theme = build(None);
        assert!(!theme.panel.is_sliced());
        assert!(!theme.button.idle.is_sliced());
        assert_ne!(
            theme.panel.fill.a, 0,
            "a panel with no art must still be visible"
        );
        assert_ne!(theme.button.idle.fill.a, 0);
    }

    #[test]
    fn desaturating_reaches_grey_and_keeps_alpha() {
        let red = Color8::new(200, 40, 40, 128);
        let grey = desaturate(red, 1.0);
        assert_eq!(grey.r, grey.g, "fully desaturated is grey");
        assert_eq!(grey.g, grey.b);
        assert_eq!(
            grey.a, 128,
            "alpha is a property of the surface, not its hue"
        );
        assert_eq!(desaturate(red, 0.0), red);
    }

    #[test]
    fn the_game_palette_is_legible_against_its_own_surfaces() {
        // Text on a panel is the thing the player reads most; if the contrast is
        // not there the whole UI is decoration.
        let theme = build(None);
        let luminance =
            |c: Color8| 0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b);
        let text = luminance(theme.palette.text);
        let surface = luminance(theme.palette.surface);
        assert!(
            (text - surface).abs() > 90.0,
            "body text ({text:.0}) is too close to the panel ({surface:.0})"
        );
    }
}
