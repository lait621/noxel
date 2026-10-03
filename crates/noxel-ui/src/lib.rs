//! Noxel's UI layer: crisp bitmap text, nine-slice frames, layout and widgets.
//!
//! # Why this crate exists
//!
//! [ADR 0009](../../../docs/adr/0009-no-ui.md) decided that Noxel would ship no
//! UI toolkit, on the grounds that the original requirement was documentation
//! rather than a game. That reasoning was sound and its premise has since
//! changed: the engine now has a game built on it, and that game needs a clock, a
//! hotbar, an inventory, a shop and a dialogue box. Writing those against
//! `noxel_render::overlay` — a 3x5 uppercase font and a rectangle — is how a
//! project ends up with six incompatible half-UIs.
//!
//! So this crate exists, and [ADR 0012](../../../docs/adr/0012-ui-layer.md)
//! records the reversal. What ADR 0009 got right, and what this crate keeps, is
//! the boundary: **this is a drawing and hit-testing layer, not an application
//! framework.** There is no screen stack, no retained widget tree, no theming
//! language and no scripting. A game owns its own state and calls these
//! functions to draw it.
//!
//! # The shape of it
//!
//! ```text
//!   FontSet  ─┐
//!   Theme    ─┼─►  Ui  ──(painter)──►  Framebuffer  ──► resolve() ──► window
//!   UiState  ─┘
//! ```
//!
//! | Type | What it is |
//! |---|---|
//! | [`FontSet`] | atlas-backed text: faces, fallback, wrapping, one baseline |
//! | [`Theme`] | colours, spacing and frame styles — a value, not a global |
//! | [`UiState`] | one frame of hover/press/click state and queued tooltips |
//! | [`Painter`] | clipped, integer drawing into the linear framebuffer |
//! | [`Ui`] | the three above bundled, plus the widgets |
//! | [`UiRect`] | integer screen geometry |
//!
//! # The three rules
//!
//! Everything here follows from three decisions, and a change that breaks one is
//! a bug rather than a preference:
//!
//! 1. **Integer pixels.** No `f32` position reaches the framebuffer. A widget at
//!    `x = 10.5` would be resampled by the window's whole-number upscale and come
//!    out two pixels wide and half-bright.
//! 2. **Clipping is structural.** A child cannot paint outside its parent, no
//!    matter what rectangle it asks for. This is what makes a scroll view, a
//!    tooltip near an edge, and a long name in a narrow slot all safe by
//!    construction rather than by each caller remembering.
//! 3. **Works with no art.** Every style has a flat fallback, so a game can build
//!    its whole interface before a single texture exists, then upgrade to
//!    nine-slice art by filling in rectangles.
//!
//! # A frame
//!
//! ```no_run
//! use noxel_ui::{Id, TextRole, TextStyle, Ui, UiInput, UiRect};
//! use noxel_core::math::Color8;
//! use noxel_render::framebuffer::Framebuffer;
//! # fn load() -> Ui { unimplemented!() }
//! # let mut ui = load();
//! # let mut framebuffer = Framebuffer::new(480, 270);
//! # let input = UiInput::new();
//! ui.begin(1.0 / 60.0, &input);
//!
//! let screen = UiRect::screen(480, 270);
//! let panel = screen.inset(noxel_ui::Insets::all(4)).with_size(120, 40);
//! {
//!     let mut painter = ui.painter(&mut framebuffer);
//!     let panel_style = ui.theme.panel;
//!     ui.panel(&mut painter, panel, &panel_style);
//!     let inner = panel.inset(ui.theme.metrics.padding);
//!     ui.label(&mut painter, inner, "Gold: 1,240", &TextStyle::new(Color8::WHITE));
//!     if ui.button(&mut painter, &input, inner, Id::new("sell"), "Sell", true).clicked {
//!         // the player sold something
//!     }
//! }
//!
//! // Nothing underneath the panel should also react to the click.
//! let pointer_reached_the_world = !ui.state.pointer_over_ui();
//! ui.end();
//! ```
//!
//! # What is deliberately not here
//!
//! * **A layout engine.** Layout is [`UiRect`] arithmetic — `columns`, `rows`,
//!   `cut_top`, `inset`, `place`. A constraint solver would be a second thing to
//!   debug when a panel is one pixel off, and this arithmetic is readable at the
//!   call site.
//! * **Animation.** A widget takes the value it should draw. Easing belongs to
//!   the game, which knows what is animating.
//! * **Input routing.** [`UiState::pointer_over_ui`] is the whole mechanism; the
//!   game asks it before acting on the world.
//! * **Text editing.** A text field needs selection, a caret, an IME and a
//!   clipboard. A game that wants one builds it against [`UiState::set_focus`].
//! * **Bidirectional text.** Each glyph is drawn after the last; see
//!   [`font`] for the exact limitation.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod context;
pub mod font;
pub mod geom;
pub mod input;
pub mod painter;
pub mod theme;
pub mod widget;

/// A font that exists only in memory, for a downstream game's tests.
///
/// Behind the `test-support` feature, so a shipped build does not carry it. See
/// the module documentation for why a game wants this rather than the real bake.
#[cfg(any(test, feature = "test-support"))]
pub mod testfont;

pub use context::{Id, Response, TextRole, Tooltip, Ui, UiState, text_color};
pub use font::{
    Face, FontError, FontSet, Glyph, LineMetrics, MISSING_ADVANCE, TextAlign, TextStyle, Wrap,
};
pub use geom::{Anchor, Axis, Insets, UiRect};
pub use input::{KeyCode, UiInput, UiInputBuilder};
pub use painter::{Painter, srgb_of};
pub use theme::{BarStyle, ButtonStyle, FrameStyle, Metrics, Palette, Theme, darken, lighten, mix};
pub use widget::{SlotState, Sprite};

/// The types a game reaches for on every frame.
pub mod prelude {
    pub use crate::context::{Id, Response, TextRole, Ui, UiState};
    pub use crate::font::{TextAlign, TextStyle, Wrap};
    pub use crate::geom::{Anchor, Insets, UiRect};
    pub use crate::input::{UiInput, UiInputBuilder};
    pub use crate::painter::Painter;
    pub use crate::theme::{FrameStyle, Theme};
    pub use crate::widget::{SlotState, Sprite};
}
