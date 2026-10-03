//! Colours, spacing and frame styles.
//!
//! A theme is a plain value, not a global. That is deliberate: a game that wants
//! a night-time UI, a shop screen in a different palette, or a high-contrast mode
//! makes a second `Theme` and passes it, rather than mutating a singleton that
//! every existing widget is holding a reference to.
//!
//! Every style works **without an atlas**. A [`FrameStyle`] with an empty source
//! rectangle draws a flat filled box with a border; the same style with a source
//! rectangle draws a nine-slice from the UI texture instead. That means a game
//! gets a usable UI on the first frame, before any art exists, and upgrades to
//! authored art by filling in rectangles — which is exactly the order a game is
//! actually built in.

use noxel_core::math::Color8;

use crate::geom::{Insets, UiRect};

/// The semantic colours a UI needs.
///
/// Named by role rather than by appearance (`text_dim`, not `grey`), so swapping
/// the palette does not leave a field called `grey` holding something blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Body text.
    pub text: Color8,
    /// Secondary text: units, hints, disabled labels.
    pub text_dim: Color8,
    /// Emphasised text: a title, the selected row.
    pub text_strong: Color8,
    /// Text drawn on top of a filled accent.
    pub text_on_accent: Color8,
    /// The one colour that marks "this is interactive".
    pub accent: Color8,
    /// A warning or a loss.
    pub danger: Color8,
    /// A gain or a success.
    pub good: Color8,
    /// A drop shadow under text and panels.
    pub shadow: Color8,
    /// The darkening pass drawn behind a modal.
    pub scrim: Color8,
    /// A panel's background where no atlas is loaded.
    pub surface: Color8,
    /// A recessed area: a slot, an input, a progress bar's trough.
    pub surface_sunken: Color8,
    /// A raised area: a button's idle face.
    pub surface_raised: Color8,
    /// The border of a panel.
    pub border: Color8,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            text: Color8::new(238, 232, 213, 255),
            text_dim: Color8::new(160, 152, 136, 255),
            text_strong: Color8::new(255, 246, 219, 255),
            text_on_accent: Color8::new(38, 30, 20, 255),
            accent: Color8::new(240, 186, 84, 255),
            danger: Color8::new(214, 84, 74, 255),
            good: Color8::new(126, 190, 96, 255),
            shadow: Color8::new(12, 10, 16, 160),
            scrim: Color8::new(8, 6, 14, 170),
            surface: Color8::new(58, 46, 74, 240),
            surface_sunken: Color8::new(34, 27, 45, 240),
            surface_raised: Color8::new(92, 74, 112, 255),
            border: Color8::new(30, 22, 40, 255),
        }
    }
}

/// The spacing scale.
///
/// All integers, and all small. A UI at 480x270 internal pixels has no room for
/// an 8px gap to be "close enough" to a 9px one; picking from a fixed scale is
/// what keeps two panels authored on different days aligning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    /// The smallest separation between two elements.
    pub gap: i32,
    /// Space between groups of elements.
    pub gap_large: i32,
    /// Inner padding of a panel.
    pub padding: Insets,
    /// The height of one row in a list.
    pub row_height: u32,
    /// The height of a standard button.
    pub button_height: u32,
    /// The side of a square inventory slot.
    pub slot_size: u32,
    /// The thickness of a panel border.
    pub border: u32,
    /// The height of a progress or energy bar.
    pub bar_height: u32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            gap: 2,
            gap_large: 6,
            padding: Insets::symmetric(4, 3),
            row_height: 12,
            button_height: 14,
            slot_size: 20,
            border: 1,
            bar_height: 7,
        }
    }
}

/// How a box is drawn: a nine-slice when art is available, a flat fill when not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStyle {
    /// The region of the UI texture to slice. Empty means "draw flat".
    pub source: UiRect,
    /// The fixed border widths inside `source`, in texels.
    pub insets: Insets,
    /// A colour multiplier over the sliced art. [`Color8::WHITE`] draws it as
    /// authored.
    pub tint: Color8,
    /// The fill used when `source` is empty.
    pub fill: Color8,
    /// The border used when `source` is empty.
    pub border: Color8,
    /// Border thickness for the flat fallback.
    pub border_width: u32,
    /// A drop shadow, drawn this far down and right of the box.
    pub shadow: Color8,
    /// Shadow displacement. `(0, 0)` disables it.
    pub shadow_offset: (i32, i32),
}

impl Default for FrameStyle {
    fn default() -> Self {
        Self {
            source: UiRect::ZERO,
            insets: Insets::all(3),
            tint: Color8::WHITE,
            fill: Palette::default().surface,
            border: Palette::default().border,
            border_width: 1,
            shadow: Color8::TRANSPARENT,
            shadow_offset: (0, 0),
        }
    }
}

impl FrameStyle {
    /// A flat style: a filled box with a border and no art.
    #[must_use]
    pub fn flat(fill: Color8, border: Color8) -> Self {
        Self {
            fill,
            border,
            ..Self::default()
        }
    }

    /// A flat style with no border at all, for a scrim or a divider.
    #[must_use]
    pub fn solid(fill: Color8) -> Self {
        Self {
            fill,
            border: Color8::TRANSPARENT,
            border_width: 0,
            ..Self::default()
        }
    }

    /// A nine-slice style cut from `source`.
    #[must_use]
    pub fn sliced(source: UiRect, insets: Insets) -> Self {
        Self {
            source,
            insets,
            ..Self::default()
        }
    }

    /// Recolours the style.
    #[must_use]
    pub fn with_tint(mut self, tint: Color8) -> Self {
        self.tint = tint;
        self
    }

    /// Adds a drop shadow.
    #[must_use]
    pub fn with_shadow(mut self, color: Color8, offset: (i32, i32)) -> Self {
        self.shadow = color;
        self.shadow_offset = offset;
        self
    }

    /// Whether this style needs a texture to draw.
    #[must_use]
    pub fn is_sliced(&self) -> bool {
        !self.source.is_empty()
    }
}

/// The three faces of a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonStyle {
    /// Not hovered, not pressed.
    pub idle: FrameStyle,
    /// Hovered.
    pub hover: FrameStyle,
    /// Held down.
    pub pressed: FrameStyle,
    /// Drawn when the button cannot be used.
    pub disabled: FrameStyle,
    /// The label colour in each state.
    pub text: Color8,
    /// The label colour while hovered.
    pub text_hover: Color8,
    /// The label colour while disabled.
    pub text_disabled: Color8,
    /// Label padding inside the button.
    pub padding: Insets,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        let palette = Palette::default();
        Self {
            idle: FrameStyle::flat(palette.surface_raised, palette.border),
            hover: FrameStyle::flat(lighten(palette.surface_raised, 22), palette.accent),
            pressed: FrameStyle::flat(darken(palette.surface_raised, 26), palette.accent),
            disabled: FrameStyle::flat(
                darken(palette.surface_raised, 40),
                darken(palette.border, 10),
            ),
            text: palette.text,
            text_hover: palette.text_strong,
            text_disabled: palette.text_dim,
            padding: Insets::symmetric(4, 2),
        }
    }
}

/// The faces of a progress bar.
///
/// `PartialEq` but not `Eq`: the low-value threshold is an `f32`, and pretending
/// otherwise to satisfy a derive would be a lie the compiler is right to refuse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarStyle {
    /// The empty trough.
    pub trough: FrameStyle,
    /// The filled portion.
    pub fill: Color8,
    /// The fill colour when the value is low enough to be a problem.
    pub fill_low: Color8,
    /// Below this fraction, `fill_low` is used.
    pub low_threshold: f32,
    /// A one-pixel highlight along the top of the fill.
    pub highlight: Color8,
}

impl Default for BarStyle {
    fn default() -> Self {
        let palette = Palette::default();
        Self {
            trough: FrameStyle::flat(palette.surface_sunken, palette.border),
            fill: palette.good,
            fill_low: palette.danger,
            low_threshold: 0.25,
            highlight: Color8::new(255, 255, 255, 40),
        }
    }
}

/// Everything the widgets need to draw themselves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Semantic colours.
    pub palette: Palette,
    /// Spacing.
    pub metrics: Metrics,
    /// A raised container: a HUD panel, a dialog.
    pub panel: FrameStyle,
    /// A recessed container: an inventory grid, a text field.
    pub sunken: FrameStyle,
    /// A tooltip.
    pub tooltip: FrameStyle,
    /// A button.
    pub button: ButtonStyle,
    /// A progress bar.
    pub bar: BarStyle,
    /// A tooltip's text colour.
    pub tooltip_text: Color8,
}

impl Default for Theme {
    fn default() -> Self {
        let palette = Palette::default();
        Self {
            palette,
            metrics: Metrics::default(),
            panel: FrameStyle::flat(palette.surface, palette.border)
                .with_shadow(palette.shadow, (2, 2)),
            sunken: FrameStyle::flat(palette.surface_sunken, palette.border),
            tooltip: FrameStyle::flat(Color8::new(24, 18, 32, 245), palette.accent)
                .with_shadow(palette.shadow, (2, 2)),
            button: ButtonStyle::default(),
            bar: BarStyle::default(),
            tooltip_text: palette.text,
        }
    }
}

/// Moves a colour towards white.
///
/// In sRGB bytes, not linear radiance: this is authoring, not lighting, and a
/// designer expects "22 lighter" to look like 22 steps lighter on the swatch.
#[must_use]
pub fn lighten(color: Color8, amount: u8) -> Color8 {
    Color8::new(
        color.r.saturating_add(amount),
        color.g.saturating_add(amount),
        color.b.saturating_add(amount),
        color.a,
    )
}

/// Moves a colour towards black.
#[must_use]
pub fn darken(color: Color8, amount: u8) -> Color8 {
    Color8::new(
        color.r.saturating_sub(amount),
        color.g.saturating_sub(amount),
        color.b.saturating_sub(amount),
        color.a,
    )
}

/// Blends two colours in sRGB bytes.
#[must_use]
pub fn mix(a: Color8, b: Color8, t: f32) -> Color8 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color8::new(
        channel(a.r, b.r),
        channel(a.g, b.g),
        channel(a.b, b.b),
        channel(a.a, b.a),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighten_and_darken_saturate_instead_of_wrapping() {
        assert_eq!(lighten(Color8::new(250, 250, 250, 255), 40), Color8::WHITE);
        assert_eq!(
            darken(Color8::new(10, 10, 10, 255), 40),
            Color8::new(0, 0, 0, 255)
        );
        // Alpha is a property of the surface, not of its brightness.
        assert_eq!(lighten(Color8::new(0, 0, 0, 128), 10).a, 128);
    }

    #[test]
    fn mixing_hits_both_ends_exactly() {
        let a = Color8::new(10, 20, 30, 40);
        let b = Color8::new(200, 210, 220, 230);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        // Out-of-range t clamps rather than extrapolating past the endpoints.
        assert_eq!(mix(a, b, -5.0), a);
        assert_eq!(mix(a, b, 5.0), b);
    }

    #[test]
    fn a_default_theme_draws_with_no_atlas_loaded() {
        // The first frame of a new game has no art; every style must still be
        // usable or the UI cannot be built before the art is.
        let theme = Theme::default();
        for style in [theme.panel, theme.sunken, theme.tooltip, theme.button.idle] {
            assert!(
                !style.is_sliced(),
                "the default theme must not require a texture"
            );
            assert_ne!(style.fill.a, 0, "a default box must be visible");
        }
    }

    #[test]
    fn a_sliced_style_reports_that_it_needs_a_texture() {
        let style = FrameStyle::sliced(UiRect::new(0, 0, 12, 12), Insets::all(3));
        assert!(style.is_sliced());
        // A solid style has a border width but a transparent border, so it draws
        // as a plain rectangle rather than as a one-pixel outline of nothing.
        let solid = FrameStyle::solid(Color8::WHITE);
        assert_eq!(solid.border_width, 0);
        assert_eq!(solid.border.a, 0);
    }
}
