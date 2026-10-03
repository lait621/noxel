//! # Noxel window
//!
//! Presents the engine's framebuffer on screen and turns platform events into
//! player input.
//!
//! ```no_run
//! use noxel_window::{Host, Input, WindowConfig};
//! use noxel_render::Framebuffer;
//!
//! # struct Game { framebuffer: Framebuffer }
//! # impl Host for Game {
//! #     fn step(&mut self, _dt: f32, _input: &Input) -> &Framebuffer { &self.framebuffer }
//! #     fn internal_size(&self) -> (u32, u32) { (320, 180) }
//! # }
//! # let game = Game { framebuffer: Framebuffer::new(320, 180) };
//! noxel_window::run(WindowConfig::default(), game).unwrap();
//! ```
//!
//! ## Why this crate exists, and why it is last
//!
//! Every other crate in the workspace has **zero third-party dependencies** and
//! builds with no network (`docs/adr/0002-no-dependencies.md`). A window cannot
//! honour that: opening one needs a platform API, and on every desktop that
//! means FFI into the OS toolkit.
//!
//! So this crate is the exception, and it is boxed in on three sides:
//!
//! 1. It is the **only** crate with a dependency, and the dependency is
//!    **optional**. `cargo build --workspace` downloads nothing and still works
//!    with no network — verified by test.
//! 2. Without the `window` feature the crate compiles to a stub that returns a
//!    clear error, so nothing above it needs a `cfg`.
//! 3. The engine never sees it. `Host` is a trait, and the host owns the window;
//!    `noxel-app`, `noxel-render` and the rest do not know this crate exists.
//!
//! Building with a window:
//!
//! ```text
//! cargo run -p town-demo --features window -- --window
//! ```
//!
//! ## What it does
//!
//! * Opens a window with the internal resolution upscaled by a whole number, so
//!   the pixel art stays pixel art (`docs/guides/windowing.md` explains the
//!   letterboxing).
//! * Converts keyboard and mouse events into [`Input`], using the same key codes
//!   the engine's own `InputState` uses, so a game can read either.
//! * Runs the fixed-timestep loop: it measures the real elapsed time and hands
//!   it to [`Host::step`], which is what keeps gameplay speed independent of the
//!   frame rate.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

use noxel_render::Framebuffer;

mod error;
pub use error::WindowError;

#[cfg(feature = "window")]
mod host;
#[cfg(feature = "window")]
pub use host::run;

#[cfg(not(feature = "window"))]
mod stub;
#[cfg(not(feature = "window"))]
pub use stub::run;

pub mod key;

/// How the window should look and behave.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowConfig {
    /// The window title.
    pub title: String,
    /// The requested window size in physical pixels.
    ///
    /// The actual size is the internal resolution multiplied by the largest
    /// whole scale that fits on screen, unless `pixel_perfect` is off, in which
    /// case this is used directly.
    pub window: (u32, u32),
    /// The internal resolution.
    pub internal: (u32, u32),
    /// Upscale by whole numbers only, letterboxing the remainder.
    ///
    /// Turning this off stretches to the window and makes the art shimmer; it
    /// exists so a screenshot at an arbitrary size is possible.
    pub pixel_perfect: bool,
    /// Clear the window to this colour before the first frame.
    pub background: [u8; 3],
    /// Exit after this many seconds. `None` runs until the window is closed.
    ///
    /// Used by the tests and by `--frames`, which need a headless-ish run.
    pub exit_after: Option<f32>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Noxel".to_string(),
            window: (960, 540),
            internal: (320, 180),
            pixel_perfect: true,
            background: [8, 10, 16],
            exit_after: None,
        }
    }
}

impl WindowConfig {
    /// Sets the title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the internal resolution.
    #[must_use]
    pub fn with_internal(mut self, width: u32, height: u32) -> Self {
        self.internal = (width.max(1), height.max(1));
        self
    }

    /// Quits after a fixed number of seconds.
    #[must_use]
    pub fn with_exit_after(mut self, seconds: f32) -> Self {
        self.exit_after = Some(seconds.max(0.0));
        self
    }

    /// The largest whole-number scale that fits `window`.
    #[must_use]
    pub fn integer_scale(&self, window: (u32, u32)) -> u32 {
        let sx = window.0 / self.internal.0.max(1);
        let sy = window.1 / self.internal.1.max(1);
        sx.min(sy).max(1)
    }
}

/// One frame's worth of player input.
///
/// Deliberately the same shape as `noxel_app::InputState`: a game can copy one
/// into the other field by field, or read this directly. The key codes are
/// identical too (see [`key`]), so `movement_axis(87, 83, 65, 68)` means WASD in
/// both.
#[derive(Clone, Debug, Default)]
pub struct Input {
    held: Vec<u32>,
    pressed: Vec<u32>,
    released: Vec<u32>,
    /// Cursor position in window pixels.
    pub mouse: (f32, f32),
    /// Cursor movement since the previous frame.
    pub mouse_delta: (f32, f32),
    /// Scroll wheel delta since the previous frame.
    pub scroll: f32,
    /// Left, middle, right.
    pub mouse_buttons: [bool; 3],
    /// True while either shift key is held.
    pub shift: bool,
    /// True while either control key is held.
    pub control: bool,
    /// True while either alt/option key is held.
    pub alt: bool,
}

impl Input {
    /// An input with nothing pressed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a key going down.
    pub fn press(&mut self, code: u32) {
        if !self.held.contains(&code) {
            self.held.push(code);
        }
        if !self.pressed.contains(&code) {
            self.pressed.push(code);
        }
    }

    /// Records a key coming up.
    pub fn release(&mut self, code: u32) {
        self.held.retain(|k| *k != code);
        if !self.released.contains(&code) {
            self.released.push(code);
        }
    }

    /// True while a key is held.
    #[must_use]
    pub fn is_held(&self, code: u32) -> bool {
        self.held.contains(&code)
    }

    /// True only on the frame the key went down.
    #[must_use]
    pub fn was_pressed(&self, code: u32) -> bool {
        self.pressed.contains(&code)
    }

    /// True only on the frame the key came up.
    #[must_use]
    pub fn was_released(&self, code: u32) -> bool {
        self.released.contains(&code)
    }

    /// The movement axis as `(x, z)`, from four key codes.
    ///
    /// `x` is positive to the right and `z` positive away from the camera, which
    /// is what a top-down game's steering wants.
    #[must_use]
    pub fn movement_axis(&self, up: u32, down: u32, left: u32, right: u32) -> (f32, f32) {
        let x = f32::from(self.is_held(right)) - f32::from(self.is_held(left));
        let z = f32::from(self.is_held(down)) - f32::from(self.is_held(up));
        (x, z)
    }

    /// WASD as a movement axis.
    #[must_use]
    pub fn wasd(&self) -> (f32, f32) {
        self.movement_axis(key::W, key::S, key::A, key::D)
    }

    /// The arrow keys as a movement axis.
    #[must_use]
    pub fn arrows(&self) -> (f32, f32) {
        self.movement_axis(key::UP, key::DOWN, key::LEFT, key::RIGHT)
    }

    /// Every key currently held.
    #[must_use]
    pub fn held(&self) -> &[u32] {
        &self.held
    }

    /// Keys pressed this frame.
    #[must_use]
    pub fn pressed(&self) -> &[u32] {
        &self.pressed
    }

    /// Keys released this frame.
    #[must_use]
    pub fn released(&self) -> &[u32] {
        &self.released
    }

    /// Releases everything, for a focus loss.
    pub fn release_all(&mut self) {
        self.released.extend(self.held.iter().copied());
        self.held.clear();
        self.mouse_buttons = [false; 3];
    }

    /// Clears the per-frame edges. Called by the window host after each frame.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.mouse_delta = (0.0, 0.0);
        self.scroll = 0.0;
    }
}

/// A thing the window can drive.
///
/// The host owns the loop, this owns the game. `step` is called once per frame
/// with the real elapsed time and the current input, and returns the framebuffer
/// to present.
pub trait Host {
    /// Advances one frame and returns the framebuffer to put on screen.
    fn step(&mut self, dt: f32, input: &Input) -> &Framebuffer;

    /// The internal resolution the host renders at.
    fn internal_size(&self) -> (u32, u32);

    /// Called once when the window is closing.
    fn shutdown(&mut self) {}

    /// Return `true` to close the window.
    ///
    /// Checked after every frame, so a host implementing `--frames 300` can stop
    /// itself without reaching for the event loop.
    fn should_quit(&self) -> bool {
        false
    }

    /// A title suffix, for a frame counter or a seed.
    fn title_suffix(&self) -> String {
        String::new()
    }
}

/// A trivial [`Host`] that renders a solid colour, used by the tests and as the
/// smallest possible example.
pub struct ClearColor {
    framebuffer: Framebuffer,
    frames: u64,
}

impl ClearColor {
    /// Creates a host that fills a framebuffer with a colour.
    #[must_use]
    pub fn new(width: u32, height: u32, color: [f32; 3]) -> Self {
        let mut framebuffer = Framebuffer::new(width, height);
        framebuffer.clear(color);
        Self {
            framebuffer,
            frames: 0,
        }
    }

    /// Frames stepped so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

impl Host for ClearColor {
    fn step(&mut self, _dt: f32, _input: &Input) -> &Framebuffer {
        self.frames += 1;
        &self.framebuffer
    }

    fn internal_size(&self) -> (u32, u32) {
        (self.framebuffer.width(), self.framebuffer.height())
    }

    fn title_suffix(&self) -> String {
        format!("frame {}", self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_a_pixel_art_window() {
        let config = WindowConfig::default();
        assert_eq!(config.internal, (320, 180));
        assert!(config.pixel_perfect);
        assert!(config.exit_after.is_none());
        assert!(!config.title.is_empty());
    }

    #[test]
    fn integer_scale_picks_the_largest_whole_multiple() {
        let config = WindowConfig::default().with_internal(320, 180);
        assert_eq!(config.integer_scale((320, 180)), 1);
        assert_eq!(config.integer_scale((640, 360)), 2);
        assert_eq!(config.integer_scale((960, 540)), 3);
        // The window is wider than it is tall: the height is the limit.
        assert_eq!(config.integer_scale((1920, 400)), 2);
    }

    #[test]
    fn integer_scale_never_returns_zero() {
        let config = WindowConfig::default();
        assert_eq!(config.integer_scale((1, 1)), 1);
        let tiny = WindowConfig::default().with_internal(0, 0);
        assert_eq!(tiny.integer_scale((10, 10)), 10);
    }

    #[test]
    fn builders_chain() {
        let config = WindowConfig::default()
            .with_title("A Village")
            .with_internal(160, 90)
            .with_exit_after(2.0);
        assert_eq!(config.title, "A Village");
        assert_eq!(config.internal, (160, 90));
        assert_eq!(config.exit_after, Some(2.0));
    }

    #[test]
    fn exit_after_is_clamped_to_non_negative() {
        let config = WindowConfig::default().with_exit_after(-3.0);
        assert_eq!(config.exit_after, Some(0.0));
    }

    #[test]
    fn input_tracks_held_and_edge_states() {
        let mut input = Input::new();
        input.press(key::W);
        assert!(input.is_held(key::W));
        assert!(input.was_pressed(key::W));
        assert!(!input.was_released(key::W));
        input.end_frame();
        assert!(input.is_held(key::W), "held survives a frame boundary");
        assert!(!input.was_pressed(key::W), "the edge does not");
        input.release(key::W);
        assert!(!input.is_held(key::W));
        assert!(input.was_released(key::W));
    }

    #[test]
    fn pressing_twice_does_not_duplicate() {
        let mut input = Input::new();
        input.press(key::SPACE);
        input.press(key::SPACE);
        assert_eq!(input.held().len(), 1);
        assert_eq!(input.pressed().len(), 1);
    }

    #[test]
    fn wasd_axis_matches_the_engine_convention() {
        let mut input = Input::new();
        assert_eq!(input.wasd(), (0.0, 0.0));
        input.press(key::D);
        assert_eq!(input.wasd(), (1.0, 0.0), "d is right");
        input.press(key::W);
        assert_eq!(input.wasd(), (1.0, -1.0), "w is away from the camera");
        input.press(key::S);
        input.release(key::D);
        assert_eq!(input.wasd(), (-0.0, 0.0));
    }

    #[test]
    fn arrows_and_wasd_are_separate() {
        let mut input = Input::new();
        input.press(key::UP);
        assert_eq!(input.arrows(), (0.0, -1.0));
        assert_eq!(input.wasd(), (0.0, 0.0));
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut input = Input::new();
        for code in [key::W, key::S, key::A, key::D] {
            input.press(code);
        }
        assert_eq!(input.wasd(), (0.0, 0.0));
    }

    #[test]
    fn release_all_clears_everything() {
        let mut input = Input::new();
        input.press(key::W);
        input.mouse_buttons[0] = true;
        input.release_all();
        assert!(!input.is_held(key::W));
        assert!(input.was_released(key::W));
        assert_eq!(input.mouse_buttons, [false; 3]);
    }

    #[test]
    fn end_frame_clears_the_deltas() {
        let mut input = Input::new();
        input.mouse_delta = (3.0, 4.0);
        input.scroll = 1.0;
        input.end_frame();
        assert_eq!(input.mouse_delta, (0.0, 0.0));
        assert_eq!(input.scroll, 0.0);
    }

    #[test]
    fn clear_color_host_steps_and_reports_its_size() {
        let mut host = ClearColor::new(64, 32, [1.0, 0.0, 0.0]);
        assert_eq!(host.internal_size(), (64, 32));
        let input = Input::new();
        let framebuffer = host.step(1.0 / 60.0, &input);
        assert_eq!(framebuffer.width(), 64);
        assert_eq!(framebuffer.get(0, 0).unwrap()[0], 1.0);
        assert_eq!(host.frames(), 1);
        assert!(host.title_suffix().contains("frame 1"));
    }

    #[cfg(not(feature = "window"))]
    #[test]
    fn run_without_the_feature_reports_the_feature() {
        let host = ClearColor::new(8, 8, [0.0; 3]);
        let result = run(WindowConfig::default().with_exit_after(0.0), host);
        assert!(matches!(result, Err(WindowError::FeatureDisabled)));
    }

    // `run` with the feature on cannot be smoke-tested from here: winit requires
    // an event loop on the main thread, and a test harness does not own it. The
    // binary is where a real window gets exercised — see `scripts/run-window.sh`.
    #[cfg(feature = "window")]
    #[test]
    fn the_feature_build_still_exposes_the_same_api() {
        let host = ClearColor::new(8, 8, [0.0; 3]);
        assert_eq!(host.internal_size(), (8, 8));
        let _: fn(WindowConfig, ClearColor) -> Result<(), WindowError> = run::<ClearColor>;
    }
}
