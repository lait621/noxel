//! Widget identity, interaction state, and the frame context.
//!
//! # Why identity is explicit
//!
//! An immediate-mode UI redraws every widget every frame, so it has no place to
//! keep "which button am I". It keeps that in one slot — the *active* id — and a
//! widget is the active one only if it hashes to the same [`Id`] it had last
//! frame.
//!
//! That makes the id load-bearing in a way that is easy to get wrong: two buttons
//! that hash to the same id are the *same button* as far as this system is
//! concerned, and pressing one lights up the other. Ids are therefore built from
//! a dotted path ([`Id::new`]) plus [`Id::with`] for a repeated row, so
//! `Id::new("shop").with("wheat")` and `Id::new("shop").with("corn")` differ,
//! while a loop over two shops cannot collide by accident.
//!
//! # Why the pointer is captured, not routed
//!
//! There is no input router. A widget that the pointer is over sets a flag, and
//! the game asks [`UiState::pointer_over_ui`] before it acts on a world click.
//! That is the whole mechanism, and it is enough: the alternative — a routing
//! tree with focus and bubbling — is a subsystem this engine does not need and
//! would have to keep in sync with the layout it mirrors.

use core::fmt;

use noxel_core::math::Color8;

use crate::font::FontSet;
use crate::geom::UiRect;
use crate::input::UiInput;
use crate::painter::Painter;
use crate::theme::Theme;

/// A widget's identity, stable across frames.
///
/// A 64-bit FNV-1a hash of a name. Collisions are theoretically possible and
/// practically irrelevant at the scale of one screen of widgets; the failure
/// mode is two widgets sharing hover state, not a crash or memory unsafety.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    /// No widget.
    pub const NONE: Self = Self(0);

    /// Hashes a stable name.
    ///
    /// Use a dotted path (`"shop.buy.0"`) rather than a bare label: a label is
    /// player-visible text and changes when the text does, which silently
    /// reassigns every widget's identity.
    #[must_use]
    pub fn new(name: &str) -> Self {
        // FNV-1a, 64-bit. Chosen because it is four lines, has no dependency, and
        // distributes short dotted strings well.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        Self(hash.max(1))
    }

    /// Derives a child id, for a widget inside a repeated container.
    #[must_use]
    pub fn with(self, name: &str) -> Self {
        let mut hash = self.0;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        Self(hash.max(1))
    }

    /// Derives a child id from an index, for a loop.
    #[must_use]
    pub fn index(self, index: usize) -> Self {
        self.with(&index.to_string())
    }

    /// Wraps a value that is already unique.
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Whether this is [`Id::NONE`].
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:#018x})", self.0)
    }
}

/// What happened to one widget this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    /// The widget's id.
    pub id: Id,
    /// Where the widget was drawn.
    pub rect: UiRect,
    /// The pointer is over it and nothing is blocking.
    pub hovered: bool,
    /// The primary button went down on it this frame.
    pub pressed: bool,
    /// The primary button is down and this widget is the one that took it.
    pub held: bool,
    /// The primary button came up over the widget that took the press.
    pub clicked: bool,
    /// A toggle or field changed value this frame.
    pub changed: bool,
    /// Whether keyboard focus is on this widget.
    pub focused: bool,
}

impl Response {
    /// A response for a widget nothing happened to.
    #[must_use]
    pub fn idle(id: Id, rect: UiRect) -> Self {
        Self {
            id,
            rect,
            ..Self::default()
        }
    }

    /// Whether the widget should draw its hover face.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.hovered || self.held
    }
}

/// A queued tooltip, drawn after every other widget so nothing covers it.
#[derive(Clone, Debug)]
pub struct Tooltip {
    /// The text lines, top to bottom.
    pub lines: Vec<String>,
    /// The rectangle the tooltip points at.
    pub anchor: UiRect,
    /// Where the pointer was, used to keep the box on screen.
    pub pointer: (i32, i32),
}

/// What one frame of interaction remembers.
///
/// Lives in the game, not in a global, so two UIs (a HUD and an editor overlay)
/// can coexist without stealing each other's hover state.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    hot: Id,
    active: Id,
    focus: Id,
    /// Set by any widget the pointer was over this frame.
    pointer_over_ui: bool,
    /// Set by the game when a modal owns the whole screen.
    modal: bool,
    /// Clicks consumed this frame, for a game that wants to know a click landed
    /// on chrome rather than on the world.
    consumed_click: bool,
    tooltips: Vec<Tooltip>,
    scroll_used: f32,
    time: f32,
    last_dt: f32,
    /// Whether the previous frame ended with the primary button coming up.
    ///
    /// A click spans two frames — down in one, up in the next — so the capture
    /// cannot be dropped the frame it is taken or `clicked` never fires. It also
    /// cannot be kept forever or a widget stays active after the button is
    /// released outside it. Dropping it the frame *after* the release is the only
    /// ordering that satisfies both.
    released_last_frame: bool,
}

impl UiState {
    /// A state with nothing hovered, focused or active.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the per-frame state. Call once at the top of a frame.
    ///
    /// Takes the input because a mouse capture outlives the frame it was taken
    /// in; see [`UiState::released_last_frame`].
    pub fn begin_frame(&mut self, dt: f32, input: &UiInput) {
        if self.released_last_frame {
            self.active = Id::NONE;
        }
        self.released_last_frame = input.primary_released;
        self.hot = Id::NONE;
        self.pointer_over_ui = false;
        self.consumed_click = false;
        self.tooltips.clear();
        self.scroll_used = 0.0;
        self.last_dt = dt.clamp(0.0, 0.25);
        self.time += self.last_dt;
    }

    /// Ends the frame. Call once at the bottom of one.
    ///
    /// Deliberately does **not** release the active widget: a press and its
    /// release happen in different frames, and clearing the capture here would
    /// mean `Response::clicked` could never be true. The capture ends in
    /// [`UiState::begin_frame`], the frame after the button comes up.
    pub fn end_frame(&mut self) {}

    /// Seconds since the UI started, advanced by the frame delta.
    #[must_use]
    pub fn time(&self) -> f32 {
        self.time
    }

    /// The last frame's delta, clamped.
    #[must_use]
    pub fn delta(&self) -> f32 {
        self.last_dt
    }

    /// The widget that currently owns the pointer.
    #[must_use]
    pub fn active(&self) -> Id {
        self.active
    }

    /// The widget the pointer is over.
    #[must_use]
    pub fn hot(&self) -> Id {
        self.hot
    }

    /// The widget with keyboard focus.
    #[must_use]
    pub fn focus(&self) -> Id {
        self.focus
    }

    /// Gives keyboard focus to a widget, or clears it with [`Id::NONE`].
    pub fn set_focus(&mut self, id: Id) {
        self.focus = id;
    }

    /// Declares that a modal owns the screen, so nothing underneath reacts.
    pub fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
    }

    /// Whether a modal is owning the screen.
    #[must_use]
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// Whether the pointer was over any widget this frame.
    ///
    /// The game checks this before acting on a world click, which is how a click
    /// on an inventory slot tills nothing.
    #[must_use]
    pub fn pointer_over_ui(&self) -> bool {
        self.pointer_over_ui || self.modal
    }

    /// Whether a click was consumed by a widget this frame.
    #[must_use]
    pub fn click_consumed(&self) -> bool {
        self.consumed_click
    }

    /// Queues a tooltip for the end of the frame.
    pub fn queue_tooltip(&mut self, tooltip: Tooltip) {
        self.tooltips.push(tooltip);
    }

    /// The queued tooltips.
    #[must_use]
    pub fn tooltips(&self) -> &[Tooltip] {
        &self.tooltips
    }

    /// Scroll left over after the widgets that wanted it took their share.
    #[must_use]
    pub fn scroll_remaining(&self) -> f32 {
        self.scroll_used
    }

    /// Marks `amount` of scroll as consumed.
    pub fn consume_scroll(&mut self, amount: f32) {
        self.scroll_used += amount;
    }

    /// Runs the hover/press/click state machine for one widget.
    ///
    /// The order is what makes a click feel right: a press claims the widget even
    /// if the pointer leaves it, and the click only fires if the pointer came
    /// back before the release. A button that fired on release anywhere would
    /// activate when the player pressed it, changed their mind, and let go over
    /// something else.
    pub fn interact(&mut self, id: Id, rect: UiRect, input: &UiInput) -> Response {
        let mut response = Response::idle(id, rect);
        response.focused = self.focus == id && !id.is_none();

        // A modal eats the pointer: the widget underneath sees no hover, so it
        // draws its idle face and cannot be clicked.
        let hovered = !self.modal && input.pointer_over(rect);
        if hovered {
            self.hot = id;
            self.pointer_over_ui = true;
        }

        if hovered && input.primary_pressed {
            self.active = id;
            self.pointer_over_ui = true;
            self.consumed_click = true;
        }

        response.hovered = hovered;
        response.pressed = hovered && input.primary_pressed;
        response.held = self.active == id && input.primary_down;
        response.clicked = self.active == id && hovered && input.primary_released;
        if response.clicked {
            self.consumed_click = true;
        }
        response
    }

    /// Registers a widget as hovered without running the click machine.
    ///
    /// For a purely decorative element that still has to stop a world click from
    /// passing through — a HUD panel with no interactive parts.
    pub fn block_pointer(&mut self, rect: UiRect, input: &UiInput) {
        if input.pointer_over(rect) {
            self.pointer_over_ui = true;
        }
    }
}

/// A font, a theme and one frame of interaction, together.
///
/// Optional. Everything in this crate works on [`Painter`], [`FontSet`] and
/// [`Theme`] directly, and a game that already owns those does not need a `Ui` at
/// all. It exists because most games do want one object to hand around.
pub struct Ui {
    /// The font used for every label.
    pub font: FontSet,
    /// The colours and styles.
    pub theme: Theme,
    /// The interaction state.
    pub state: UiState,
    /// The UI texture that nine-slice frames are cut from.
    pub texture: noxel_asset::image::Image,
}

impl Ui {
    /// Bundles a font, a theme and a UI texture.
    #[must_use]
    pub fn new(font: FontSet, theme: Theme, texture: noxel_asset::image::Image) -> Self {
        Self {
            font,
            theme,
            state: UiState::new(),
            texture,
        }
    }

    /// A UI with no art: the default theme draws flat boxes.
    #[must_use]
    pub fn flat(font: FontSet) -> Self {
        Self::new(
            font,
            Theme::default(),
            noxel_asset::image::Image::transparent(1, 1),
        )
    }

    /// Starts a frame.
    pub fn begin(&mut self, dt: f32, input: &UiInput) {
        self.state.begin_frame(dt, input);
    }

    /// Ends a frame.
    pub fn end(&mut self) {
        self.state.end_frame();
    }

    /// A painter clipped to the whole framebuffer.
    ///
    /// The returned painter borrows the **framebuffer**, not the `Ui`, which is
    /// what lets a caller hold one while calling `ui.button(...)` on the same
    /// line. A signature that tied the painter to `&mut self` would make every
    /// widget call a borrow error.
    pub fn painter<'a>(
        &self,
        target: &'a mut noxel_render::framebuffer::Framebuffer,
    ) -> Painter<'a> {
        Painter::full(target)
    }

    /// Draws a scrim over the whole screen and returns the painter, for a modal.
    ///
    /// The scrim is what makes a modal read as modal: without the darkening pass
    /// the world behind stays as bright as the dialog and the player keeps
    /// looking at the farm instead of at the menu. It also marks the frame as
    /// modal, so nothing underneath reacts to the pointer.
    pub fn modal_scrim<'a>(
        &mut self,
        target: &'a mut noxel_render::framebuffer::Framebuffer,
    ) -> Painter<'a> {
        self.state.set_modal(true);
        let scrim = self.theme.palette.scrim;
        let mut painter = Painter::full(target);
        painter.fill(painter.screen(), scrim);
        painter
    }

    /// Draws every tooltip queued this frame, on top of everything else.
    pub fn draw_tooltips(&self, painter: &mut Painter<'_>, screen: UiRect) {
        let metrics = self.theme.metrics;
        for tooltip in self.state.tooltips() {
            let (width, height) = tooltip_size(&self.font, tooltip, &self.theme);
            // Prefer below-right of the pointer, flip when it would leave the
            // screen. A tooltip that runs off the edge is unreadable, and the
            // near edges are exactly where a hotbar and a shop panel live.
            let mut x = tooltip.pointer.0 + metrics.gap_large;
            let mut y = tooltip.pointer.1 + metrics.gap_large;
            if x + width as i32 > screen.right() {
                x = tooltip.pointer.0 - metrics.gap_large - width as i32;
            }
            if y + height as i32 > screen.bottom() {
                y = tooltip.pointer.1 - metrics.gap_large - height as i32;
            }
            x = x.clamp(screen.x, (screen.right() - width as i32).max(screen.x));
            y = y.clamp(screen.y, (screen.bottom() - height as i32).max(screen.y));
            let box_rect = UiRect::new(x, y, width, height);

            painter.frame(&self.texture, &self.theme.tooltip, box_rect);
            let inner = box_rect.inset(metrics.padding);
            let style = crate::font::TextStyle::new(self.theme.tooltip_text);
            self.font
                .draw_text(painter, inner, &tooltip.lines.join("\n"), &style);
        }
    }
}

/// Measures a tooltip box, including its padding.
#[must_use]
pub fn tooltip_size(font: &FontSet, tooltip: &Tooltip, theme: &Theme) -> (u32, u32) {
    let padding = theme.metrics.padding;
    let style = crate::font::TextStyle::new(theme.tooltip_text);
    let mut width = 0;
    let mut height = 0i64;
    for line in &tooltip.lines {
        let (w, h) = font.measure(line, &style);
        width = width.max(w);
        height += i64::from(h) + i64::from(theme.metrics.gap);
    }
    if height > 0 {
        height -= i64::from(theme.metrics.gap);
    }
    (
        width + padding.horizontal().max(0) as u32,
        height.max(0) as u32 + padding.vertical().max(0) as u32,
    )
}

/// A themed colour, resolved from the palette.
#[must_use]
pub fn text_color(theme: &Theme, role: TextRole) -> Color8 {
    match role {
        TextRole::Body => theme.palette.text,
        TextRole::Dim => theme.palette.text_dim,
        TextRole::Strong => theme.palette.text_strong,
        TextRole::Good => theme.palette.good,
        TextRole::Bad => theme.palette.danger,
        TextRole::Accent => theme.palette.accent,
    }
}

/// The semantic role of a run of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// Ordinary text.
    Body,
    /// Secondary text.
    Dim,
    /// Emphasised text.
    Strong,
    /// A gain.
    Good,
    /// A loss or a warning.
    Bad,
    /// The interface's accent colour.
    Accent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::UiInputBuilder;

    const RECT: UiRect = UiRect {
        x: 0,
        y: 0,
        w: 40,
        h: 20,
    };

    #[test]
    fn distinct_names_give_distinct_ids() {
        assert_ne!(Id::new("shop"), Id::new("shop2"));
        // The dotted path is the point: two rows of one list must not collide.
        assert_ne!(Id::new("shop").with("wheat"), Id::new("shop").with("corn"));
        assert_ne!(Id::new("shop").index(0), Id::new("shop").index(1));
    }

    #[test]
    fn ids_are_stable_across_calls() {
        // Stability is the whole contract: a widget whose id changed per frame
        // would never stay hovered.
        assert_eq!(Id::new("hotbar.slot.3"), Id::new("hotbar.slot.3"));
        assert_eq!(Id::new("").0, 0xcbf2_9ce4_8422_2325, "the FNV offset basis");
    }

    #[test]
    fn a_click_fires_only_when_the_release_lands_on_the_presser() {
        let id = Id::new("button");
        let mut state = UiState::new();

        // Press inside.
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(5.0, 5.0).click().build();
        let response = state.interact(id, RECT, &input);
        assert!(response.pressed);
        assert!(!response.clicked);

        // Drag out and release: no click, because the release is elsewhere.
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(500.0, 500.0).release().build();
        let response = state.interact(id, RECT, &input);
        assert!(
            !response.clicked,
            "releasing away from the button must not activate it"
        );

        // Press and release inside: a click.
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(5.0, 5.0).click().build();
        state.interact(id, RECT, &input);
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(5.0, 5.0).release().build();
        let response = state.interact(id, RECT, &input);
        assert!(response.clicked);
    }

    #[test]
    fn a_modal_stops_the_widget_underneath_from_being_hovered() {
        let id = Id::new("under");
        let mut state = UiState::new();
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        state.set_modal(true);
        let input = UiInputBuilder::new().at(5.0, 5.0).click().build();
        let response = state.interact(id, RECT, &input);
        assert!(!response.hovered);
        assert!(!response.pressed);
    }

    #[test]
    fn hovering_a_widget_captures_the_pointer_for_the_game() {
        // This is what stops a click on a HUD panel from also swinging a hoe.
        let mut state = UiState::new();
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        assert!(!state.pointer_over_ui());
        let input = UiInputBuilder::new().at(5.0, 5.0).build();
        state.interact(Id::new("panel"), RECT, &input);
        assert!(state.pointer_over_ui());
    }

    #[test]
    fn a_modal_captures_the_pointer_even_where_no_widget_is() {
        let mut state = UiState::new();
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        state.set_modal(true);
        assert!(
            state.pointer_over_ui(),
            "a modal owns the whole screen, not just its box"
        );
    }

    #[test]
    fn a_capture_survives_the_frame_it_was_taken_in_and_ends_after_the_release() {
        // Three frames, because a click is press-then-release-then-nothing. An
        // earlier version cleared the capture in `end_frame`, which made
        // `Response::clicked` unreachable from a real frame loop while the unit
        // tests still passed — they never called `end_frame` between `interact`s.
        let id = Id::new("slider");
        let mut state = UiState::new();
        state.set_focus(id);

        // Frame 1: press.
        let press = UiInputBuilder::new().at(5.0, 5.0).click().build();
        state.begin_frame(1.0 / 60.0, &press);
        state.interact(id, RECT, &press);
        state.end_frame();
        assert_eq!(state.active(), id, "the capture must survive the frame");

        // Frame 2: release over the same widget — this is the click.
        let release = UiInputBuilder::new().at(5.0, 5.0).release().build();
        state.begin_frame(1.0 / 60.0, &release);
        let response = state.interact(id, RECT, &release);
        state.end_frame();
        assert!(
            response.clicked,
            "press then release over the same widget is a click"
        );
        assert_eq!(
            state.active(),
            id,
            "the release frame still needs the capture"
        );

        // Frame 3: nothing held. The capture is gone.
        let idle = UiInputBuilder::new().at(5.0, 5.0).build();
        state.begin_frame(1.0 / 60.0, &idle);
        assert_eq!(
            state.active(),
            Id::NONE,
            "a released press must not stay active"
        );
        assert_eq!(
            state.focus(),
            id,
            "focus is keyboard state and outlives a mouse-up"
        );
    }

    #[test]
    fn the_hot_widget_is_cleared_every_frame() {
        // Without this, a widget keeps its hover face after the pointer leaves,
        // which reads as a stuck button.
        let mut state = UiState::new();
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        state.interact(
            Id::new("a"),
            RECT,
            &UiInputBuilder::new().at(5.0, 5.0).build(),
        );
        assert!(!state.hot().is_none());
        state.begin_frame(1.0 / 60.0, &UiInput::new());
        assert!(state.hot().is_none());
    }

    #[test]
    fn time_advances_by_the_clamped_delta() {
        let mut state = UiState::new();
        state.begin_frame(0.1, &UiInput::new());
        assert!((state.time() - 0.1).abs() < 1e-6);
        // A long stall must not teleport an animation.
        state.begin_frame(30.0, &UiInput::new());
        assert!(
            (state.time() - 0.35).abs() < 1e-6,
            "dt is clamped to a quarter second"
        );
    }
}
