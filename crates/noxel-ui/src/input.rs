//! One frame of input, in framebuffer pixels.
//!
//! # Coordinates
//!
//! The pointer is in **framebuffer pixels** — the same space the UI is laid out
//! in — not in window pixels. Something has to convert, and it belongs in the
//! windowing layer rather than here: only the presenter knows the integer scale
//! and the letterbox offset it used, and a UI that guessed would put every click
//! in the wrong place at every window size but one.
//!
//! [`WindowConfig::cursor_to_framebuffer`](https://docs.rs/noxel-window) is that
//! conversion; a host calls it and fills this struct.
//!
//! # Edges, not levels
//!
//! Both the level (`primary_down`) and the edges (`primary_pressed`,
//! `primary_released`) are carried. Widgets need the edges — a button acts on
//! release, a drag acts on the level — and deriving an edge from the level
//! requires the previous frame, which an immediate-mode UI deliberately does not
//! keep.

use noxel_core::math::Color8;

/// A key code, using the same numbering as `noxel_app::InputState`.
pub type KeyCode = u32;

/// One frame's worth of UI input.
#[derive(Clone, Debug, Default)]
pub struct UiInput {
    /// Pointer position in framebuffer pixels. May be outside the framebuffer.
    pub pointer: (f32, f32),
    /// Pointer movement since the previous frame.
    pub pointer_delta: (f32, f32),
    /// Whether the pointer is inside the window at all.
    ///
    /// A cursor that has left the window must not hover anything: without this,
    /// the last position inside the window stays hot and a button lights up
    /// while the player is clicking somewhere else entirely.
    pub pointer_inside: bool,
    /// Primary (left) button held.
    pub primary_down: bool,
    /// Primary button went down this frame.
    pub primary_pressed: bool,
    /// Primary button came up this frame.
    pub primary_released: bool,
    /// Secondary (right) button went down this frame.
    pub secondary_pressed: bool,
    /// Scroll wheel delta. Positive scrolls up.
    pub scroll: f32,
    /// Keys that went down this frame.
    pub keys_pressed: Vec<KeyCode>,
    /// Keys currently held.
    pub keys_held: Vec<KeyCode>,
    /// Shift held, for multi-select and bulk buy.
    pub shift: bool,
    /// Control held.
    pub control: bool,
    /// Alt held.
    pub alt: bool,
}

impl UiInput {
    /// An input with nothing happening.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pointer_inside: true,
            ..Self::default()
        }
    }

    /// The pointer as whole pixels, which is the space every hit test works in.
    #[must_use]
    pub fn point(&self) -> (i32, i32) {
        (self.pointer.0.round() as i32, self.pointer.1.round() as i32)
    }

    /// Whether a key went down this frame.
    #[must_use]
    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(&key)
    }

    /// Whether a key is held.
    #[must_use]
    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key)
    }

    /// Whether any of several keys went down this frame.
    #[must_use]
    pub fn any_key_pressed(&self, keys: &[KeyCode]) -> bool {
        keys.iter().any(|key| self.key_pressed(*key))
    }

    /// Whether the pointer is over a rectangle.
    ///
    /// False while the pointer is outside the window, which is what stops the
    /// last-inside position from keeping a widget hovered.
    #[must_use]
    pub fn pointer_over(&self, rect: crate::geom::UiRect) -> bool {
        if !self.pointer_inside {
            return false;
        }
        let (x, y) = self.point();
        rect.contains(x, y)
    }

    /// Clears the per-frame edges, keeping the levels.
    ///
    /// Called by the host after a frame. Forgetting it makes `pressed` fire on
    /// every frame the button is held, which turns a click into a repeat.
    pub fn end_frame(&mut self) {
        self.primary_pressed = false;
        self.primary_released = false;
        self.secondary_pressed = false;
        self.scroll = 0.0;
        self.pointer_delta = (0.0, 0.0);
        self.keys_pressed.clear();
    }
}

/// A convenience for building an input from a window event loop in tests.
///
/// Not the only way to fill a [`UiInput`]; it exists so a test can express "the
/// pointer was at (10, 6) and clicked" without constructing every field.
#[derive(Clone, Debug)]
pub struct UiInputBuilder {
    input: UiInput,
}

impl UiInputBuilder {
    /// Starts from an idle input with the pointer inside the window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            input: UiInput::new(),
        }
    }

    /// Places the pointer, in framebuffer pixels.
    #[must_use]
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.input.pointer = (x, y);
        self.input.pointer_inside = true;
        self
    }

    /// Presses the primary button this frame.
    #[must_use]
    pub fn click(mut self) -> Self {
        self.input.primary_down = true;
        self.input.primary_pressed = true;
        self
    }

    /// Releases the primary button this frame.
    #[must_use]
    pub fn release(mut self) -> Self {
        self.input.primary_down = false;
        self.input.primary_released = true;
        self
    }

    /// Holds the primary button without a press edge.
    #[must_use]
    pub fn held(mut self) -> Self {
        self.input.primary_down = true;
        self
    }

    /// Moves the pointer outside the window.
    #[must_use]
    pub fn outside(mut self) -> Self {
        self.input.pointer_inside = false;
        self
    }

    /// Records a key press.
    #[must_use]
    pub fn key(mut self, code: KeyCode) -> Self {
        self.input.keys_pressed.push(code);
        self.input.keys_held.push(code);
        self
    }

    /// Adds scroll.
    #[must_use]
    pub fn scroll(mut self, amount: f32) -> Self {
        self.input.scroll += amount;
        self
    }

    /// The finished input.
    #[must_use]
    pub fn build(self) -> UiInput {
        self.input
    }
}

impl Default for UiInputBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A colour helper a widget can use for a hover highlight.
#[must_use]
pub fn hover_tint(base: Color8, amount: u8) -> Color8 {
    crate::theme::lighten(base, amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::UiRect;

    #[test]
    fn the_pointer_rounds_to_the_nearest_pixel() {
        let input = UiInputBuilder::new().at(10.4, 6.6).build();
        assert_eq!(input.point(), (10, 7));
    }

    #[test]
    fn a_pointer_outside_the_window_hovers_nothing() {
        // Without this a button stays lit while the player clicks in another
        // window, because the last inside position is still the last position.
        let rect = UiRect::new(0, 0, 100, 100);
        assert!(
            UiInputBuilder::new()
                .at(50.0, 50.0)
                .build()
                .pointer_over(rect)
        );
        assert!(
            !UiInputBuilder::new()
                .at(50.0, 50.0)
                .outside()
                .build()
                .pointer_over(rect)
        );
    }

    #[test]
    fn end_frame_clears_edges_and_keeps_levels() {
        let mut input = UiInputBuilder::new()
            .at(1.0, 1.0)
            .click()
            .scroll(3.0)
            .key(65)
            .build();
        assert!(input.primary_pressed);
        assert!(input.key_pressed(65));
        input.end_frame();
        assert!(
            !input.primary_pressed,
            "the press edge must not survive a frame"
        );
        assert!(!input.key_pressed(65));
        assert_eq!(input.scroll, 0.0);
        assert!(
            input.primary_down,
            "the level must survive: a drag depends on it"
        );
        assert!(input.key_held(65), "held keys must survive");
    }

    #[test]
    fn any_key_pressed_matches_one_of_several() {
        let input = UiInputBuilder::new().key(27).build();
        assert!(input.any_key_pressed(&[13, 27]));
        assert!(!input.any_key_pressed(&[13, 32]));
    }
}
