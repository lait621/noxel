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
    /// Whether Escape closes the window.
    ///
    /// `true` by default, which is what a demo wants: one key to get out of a
    /// viewer. A **game** should set it to `false` and give Escape to its own
    /// menus, because a player expects Escape to back out of a screen and a key
    /// that ends the session instead loses an afternoon to a mis-press.
    pub quit_on_escape: bool,
    /// The frame rate the host paces itself to, or `None` to run flat out.
    ///
    /// A redraw request is free and the window server does not wait for the
    /// display, so a host that asks for the next frame the moment it finishes
    /// this one renders at whatever rate the CPU allows — a software renderer
    /// with nothing on screen still pins a core at 100%, a laptop gets hot, and
    /// the fan spins up while the player reads a menu. Pacing to a target rate
    /// costs one `WaitUntil` and takes the idle load to nearly zero.
    ///
    /// `None` is the honest choice for a benchmark, which is the one caller that
    /// wants to know how fast the machine can go.
    pub target_fps: Option<u32>,
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
            quit_on_escape: true,
            target_fps: Some(60),
        }
    }
}

impl WindowConfig {
    /// Leaves Escape to the game rather than closing the window with it.
    #[must_use]
    pub fn keep_escape(mut self) -> Self {
        self.quit_on_escape = false;
        self
    }

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

    /// Sets the frame rate the host paces itself to.
    ///
    /// `None` runs flat out, which is what a benchmark wants and what a player
    /// does not: see [`WindowConfig::target_fps`].
    #[must_use]
    pub fn with_target_fps(mut self, fps: Option<u32>) -> Self {
        self.target_fps = fps.filter(|fps| *fps > 0);
        self
    }

    /// How long one paced frame lasts, or `None` when unpaced.
    #[must_use]
    pub fn frame_period(&self) -> Option<std::time::Duration> {
        self.target_fps
            .filter(|fps| *fps > 0)
            .map(|fps| std::time::Duration::from_secs_f64(1.0 / f64::from(fps)))
    }

    /// The largest whole-number scale that fits `window`.
    #[must_use]
    pub fn integer_scale(&self, window: (u32, u32)) -> u32 {
        let sx = window.0 / self.internal.0.max(1);
        let sy = window.1 / self.internal.1.max(1);
        sx.min(sy).max(1)
    }

    /// How a framebuffer of the internal size is placed in a window of `window`
    /// pixels.
    ///
    /// This is the **single source of truth** for the mapping in both directions.
    /// The presenter uses it to scale the image up, and the input side uses it to
    /// scale the cursor down; deriving those two separately is how a game ends up
    /// with a cursor that is off by the letterbox offset — a bug that is invisible
    /// at one window size and obvious at every other.
    #[must_use]
    pub fn presentation(&self, window: (u32, u32)) -> Presentation {
        let (width, height) = (window.0.max(1), window.1.max(1));
        let (source_width, source_height) = (self.internal.0.max(1), self.internal.1.max(1));
        if !self.pixel_perfect {
            // Stretch to fill: the mapping is the whole window either way.
            return Presentation {
                window: (width, height),
                source: (source_width, source_height),
                scale: 0,
                offset: (0, 0),
            };
        }
        let scale = self.integer_scale((width, height));
        let offset = (
            ((width - source_width * scale) / 2) as i32,
            ((height - source_height * scale) / 2) as i32,
        );
        Presentation {
            window: (width, height),
            source: (source_width, source_height),
            scale,
            offset,
        }
    }
}

/// Decides when the next paced frame is due.
///
/// Split out of the winit host deliberately: the arithmetic here is the
/// difference between a machine that sleeps between frames and one that pins a
/// core forever, and it is the part a test can actually reach — there is no
/// window, event loop or display in this file's tests.
///
/// The policy is **period-aligned, not gap-aligned**. After a frame has run, the
/// next one is due one period after the *previous deadline*, not one period
/// after the frame finished. If it were the latter, every millisecond the frame
/// took would be added to the cadence and the frame rate would sag away from the
/// target. A frame that ran so late that the next deadline has already passed
/// resynchronises on the present instead, because the alternative is a burst of
/// back-to-back frames to "catch up", which is exactly the stutter pacing exists
/// to remove.
#[derive(Clone, Copy, Debug)]
pub struct FramePacer {
    period: Option<std::time::Duration>,
    next: std::time::Instant,
}

impl FramePacer {
    /// A pacer that wants a frame every `period`, or as often as possible when
    /// `period` is `None`. The first frame is due immediately.
    #[must_use]
    pub fn new(period: Option<std::time::Duration>, now: std::time::Instant) -> Self {
        Self {
            period: period.filter(|period| !period.is_zero()),
            next: now,
        }
    }

    /// How long to wait before the next frame, or `None` when it is due now.
    #[must_use]
    pub fn wait_for(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        let period = self.period?;
        let _ = period;
        if now >= self.next {
            return None;
        }
        Some(self.next - now)
    }

    /// Records that a frame ran at `now` and schedules the next one.
    ///
    /// The deadline stays on the grid the pacer started on: one period after the
    /// previous deadline, skipping whole periods if the host is behind. A frame
    /// that took a third of its period therefore does not push the next one a
    /// third of a period later, and a host that stalled for ten periods resumes
    /// on the next grid point rather than rendering ten frames back to back.
    pub fn mark(&mut self, now: std::time::Instant) {
        let Some(period) = self.period else {
            return;
        };
        let mut next = self.next + period;
        if next <= now {
            let behind = now.duration_since(next);
            let period_nanos = period.as_nanos().max(1);
            let missed = behind.as_nanos() / period_nanos;
            let missed = u32::try_from(missed.saturating_add(1)).unwrap_or(u32::MAX);
            next += period.saturating_mul(missed);
            // `saturating_mul` can stop short of the present for an absurd
            // stall; the invariant that matters is "the next frame is in the
            // future", not "the phase survived it".
            if next <= now {
                next = now + period;
            }
        }
        self.next = next;
    }

    /// The deadline the next frame is due at.
    #[must_use]
    pub fn next_deadline(&self) -> std::time::Instant {
        self.next
    }

    /// True when nothing is pacing the host.
    #[must_use]
    pub fn is_uncapped(&self) -> bool {
        self.period.is_none()
    }
}

/// Where a framebuffer sits inside a window, and how the two map onto each other.
///
/// Produced by [`WindowConfig::presentation`]; see that method for why the two
/// directions must share one derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presentation {
    /// The window size in physical pixels.
    pub window: (u32, u32),
    /// The framebuffer size.
    pub source: (u32, u32),
    /// The whole-number upscale, or `0` when the image is stretched to fill.
    pub scale: u32,
    /// The letterbox offset of the framebuffer's top-left inside the window.
    pub offset: (i32, i32),
}

impl Presentation {
    /// Converts a cursor position in window pixels to framebuffer pixels.
    ///
    /// The result may be outside the framebuffer — the letterbox bars are part of
    /// the window and the cursor genuinely is there. A caller that cares tests
    /// the result against the framebuffer's bounds rather than clamping, because
    /// clamping would turn "the pointer left the game" into "the pointer is
    /// pinned to the edge", and a pinned pointer keeps a widget hovered forever.
    #[must_use]
    pub fn cursor_to_framebuffer(&self, cursor: (f32, f32)) -> (f32, f32) {
        if self.scale == 0 {
            let (width, height) = (
                f64::from(self.window.0.max(1)),
                f64::from(self.window.1.max(1)),
            );
            return (
                (f64::from(cursor.0) * f64::from(self.source.0) / width) as f32,
                (f64::from(cursor.1) * f64::from(self.source.1) / height) as f32,
            );
        }
        let scale = f32::from(self.scale as u16);
        (
            (cursor.0 - self.offset.0 as f32) / scale,
            (cursor.1 - self.offset.1 as f32) / scale,
        )
    }

    /// Whether a cursor position in window pixels is over the framebuffer rather
    /// than over a letterbox bar.
    #[must_use]
    pub fn contains_cursor(&self, cursor: (f32, f32)) -> bool {
        let (x, y) = self.cursor_to_framebuffer(cursor);
        x >= 0.0 && y >= 0.0 && x < self.source.0 as f32 && y < self.source.1 as f32
    }

    /// Where the framebuffer's top-left lands in the window.
    #[must_use]
    pub fn drawn_size(&self) -> (u32, u32) {
        if self.scale == 0 {
            self.window
        } else {
            (self.source.0 * self.scale, self.source.1 * self.scale)
        }
    }
}

/// One frame's worth of player input.
///
/// Deliberately the same shape as `noxel_app::InputState`: a game can copy one
/// into the other field by field, or read this directly. The key codes are
/// identical too (see [`key`]), so `movement_axis(87, 83, 65, 68)` means WASD in
/// both.
#[derive(Clone, Debug)]
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
    /// Buttons that went down this frame.
    pub mouse_pressed: [bool; 3],
    /// Buttons that came up this frame.
    pub mouse_released: [bool; 3],
    /// Cursor position in **framebuffer** pixels.
    ///
    /// The window host converts it with the same [`Presentation`] it scaled the
    /// image up with. A UI hit-tests in framebuffer space, so filling a widget's
    /// rectangle from `mouse` (window pixels) would put every click in the wrong
    /// place at every window size but one.
    pub cursor: (f32, f32),
    /// Whether the cursor is over the framebuffer rather than a letterbox bar.
    pub cursor_inside: bool,
    /// True while either shift key is held.
    pub shift: bool,
    /// True while either control key is held.
    pub control: bool,
    /// True while either alt/option key is held.
    pub alt: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            pressed: Vec::new(),
            released: Vec::new(),
            mouse: (0.0, 0.0),
            mouse_delta: (0.0, 0.0),
            scroll: 0.0,
            mouse_buttons: [false; 3],
            mouse_pressed: [false; 3],
            mouse_released: [false; 3],
            cursor: (0.0, 0.0),
            // A cursor that reports "outside" by default would make a headless
            // run hover nothing, which is the safe direction to be wrong in.
            // A window you just opened is one you are looking at, so the cursor
            // counts as inside until something says otherwise. Defaulting to
            // `false` meant a click before the first mouse *move* was silently
            // dropped — the interface could not place the pointer and refused to
            // hover anything, which looks exactly like a button that does not
            // work.
            cursor_inside: true,
            shift: false,
            control: false,
            alt: false,
        }
    }
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

    /// Records a mouse button going down.
    pub fn press_mouse(&mut self, button: usize) {
        if let Some(slot) = self.mouse_buttons.get_mut(button) {
            *slot = true;
        }
        if let Some(slot) = self.mouse_pressed.get_mut(button) {
            *slot = true;
        }
    }

    /// Records a mouse button coming up.
    pub fn release_mouse(&mut self, button: usize) {
        if let Some(slot) = self.mouse_buttons.get_mut(button) {
            *slot = false;
        }
        if let Some(slot) = self.mouse_released.get_mut(button) {
            *slot = true;
        }
    }

    /// True while a mouse button is held.
    #[must_use]
    pub fn is_mouse_down(&self, button: usize) -> bool {
        self.mouse_buttons.get(button).copied().unwrap_or(false)
    }

    /// True only on the frame a mouse button went down.
    #[must_use]
    pub fn was_mouse_pressed(&self, button: usize) -> bool {
        self.mouse_pressed.get(button).copied().unwrap_or(false)
    }

    /// True only on the frame a mouse button came up.
    #[must_use]
    pub fn was_mouse_released(&self, button: usize) -> bool {
        self.mouse_released.get(button).copied().unwrap_or(false)
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
        // Levels survive, edges do not: a drag depends on `mouse_buttons` still
        // being true on the next frame, and a click must not repeat.
        self.mouse_pressed = [false; 3];
        self.mouse_released = [false; 3];
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
mod presentation_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A 320x180 framebuffer in a 960x540 window: exactly 3x, no letterbox.
    fn exact() -> WindowConfig {
        WindowConfig::default().with_internal(320, 180)
    }

    #[test]
    fn an_exact_multiple_has_no_letterbox_offset() {
        let presentation = exact().presentation((960, 540));
        assert_eq!(presentation.scale, 3);
        assert_eq!(presentation.offset, (0, 0));
        assert_eq!(presentation.drawn_size(), (960, 540));
    }

    #[test]
    fn the_offset_is_half_the_remaining_space_so_the_image_is_centred() {
        // 3x of 320x180 is 960x540 inside 1000x600: 40 spare horizontally, 60
        // vertically, so the image starts at (20, 30).
        let presentation = exact().presentation((1000, 600));
        assert_eq!(presentation.scale, 3);
        assert_eq!(presentation.offset, (20, 30));
    }

    #[test]
    fn the_cursor_mapping_inverts_the_upscale_and_the_offset() {
        let presentation = exact().presentation((1000, 600));
        // The top-left pixel of the image maps back to framebuffer (0, 0).
        let origin = presentation.cursor_to_framebuffer((20.0, 30.0));
        assert_eq!(origin, (0.0, 0.0));
        // One window pixel further right is a third of a framebuffer pixel.
        let stepped = presentation.cursor_to_framebuffer((23.0, 33.0));
        assert!((stepped.0 - 1.0).abs() < 1e-5, "{stepped:?}");
        assert!((stepped.1 - 1.0).abs() < 1e-5, "{stepped:?}");
    }

    #[test]
    fn a_cursor_in_the_letterbox_is_outside_the_framebuffer() {
        // Clamping instead of reporting the truth would pin the pointer to the
        // edge and keep a widget hovered while the player is in the black bars.
        let presentation = exact().presentation((1000, 600));
        assert!(!presentation.contains_cursor((5.0, 300.0)), "left bar");
        assert!(!presentation.contains_cursor((995.0, 300.0)), "right bar");
        assert!(!presentation.contains_cursor((500.0, 5.0)), "top bar");
        assert!(presentation.contains_cursor((500.0, 300.0)), "centre");
    }

    #[test]
    fn the_mapping_agrees_with_the_presenter_at_every_window_size() {
        // The bug this guards: a presenter that rounds the letterbox offset one
        // way and a cursor mapper that rounds it another. The two are only
        // guaranteed to agree because they share `presentation`.
        let config = exact();
        for window in [
            (960, 540),
            (1000, 600),
            (1280, 720),
            (500, 400),
            (321, 181),
            (4000, 3000),
        ] {
            let presentation = config.presentation(window);
            let (drawn_w, drawn_h) = presentation.drawn_size();
            assert!(
                drawn_w <= window.0 && drawn_h <= window.1,
                "the image must fit {window:?}"
            );
            // The centre of the window is the centre of the framebuffer.
            let centre =
                presentation.cursor_to_framebuffer((window.0 as f32 / 2.0, window.1 as f32 / 2.0));
            let expect = (
                presentation.source.0 as f32 / 2.0,
                presentation.source.1 as f32 / 2.0,
            );
            assert!(
                (centre.0 - expect.0).abs() <= presentation.scale as f32
                    && (centre.1 - expect.1).abs() <= presentation.scale as f32,
                "centre maps to {centre:?}, expected about {expect:?} at {window:?}"
            );
            // Every drawn pixel maps back inside the framebuffer.
            let corner = presentation.cursor_to_framebuffer((
                (presentation.offset.0 + drawn_w as i32 - 1) as f32,
                (presentation.offset.1 + drawn_h as i32 - 1) as f32,
            ));
            assert!(
                corner.0 < presentation.source.0 as f32 && corner.1 < presentation.source.1 as f32
            );
        }
    }

    #[test]
    fn a_stretched_presentation_maps_the_window_onto_the_framebuffer() {
        let config = WindowConfig {
            pixel_perfect: false,
            ..WindowConfig::default().with_internal(320, 180)
        };
        let presentation = config.presentation((640, 360));
        assert_eq!(presentation.scale, 0);
        assert_eq!(
            presentation.cursor_to_framebuffer((320.0, 180.0)),
            (160.0, 90.0)
        );
        assert!(presentation.contains_cursor((639.0, 359.0)));
    }

    #[test]
    fn mouse_edges_fire_once_and_levels_survive_a_frame() {
        let mut input = Input::new();
        input.press_mouse(0);
        assert!(input.is_mouse_down(0));
        assert!(input.was_mouse_pressed(0));
        input.end_frame();
        assert!(
            input.is_mouse_down(0),
            "a drag depends on the level surviving"
        );
        assert!(
            !input.was_mouse_pressed(0),
            "a click must not repeat every frame"
        );

        input.release_mouse(0);
        assert!(!input.is_mouse_down(0));
        assert!(input.was_mouse_released(0));
        input.end_frame();
        assert!(!input.was_mouse_released(0));
    }

    #[test]
    fn an_out_of_range_button_index_is_ignored_rather_than_panicking() {
        let mut input = Input::new();
        input.press_mouse(9);
        assert!(!input.is_mouse_down(9));
        assert!(!input.was_mouse_pressed(9));
    }

    // ------------------------------------------------------------ the pacer
    //
    // This is the part of the window host that decides whether a machine sleeps
    // between frames or pins a core. It is testable here precisely because the
    // window, the event loop and the display are all somewhere else.
    //
    // One base instant per test, because `Instant::now()` twice is two different
    // instants and a cadence test that is 40 nanoseconds out asserts nothing.

    /// The two-hundred-and-forty-hertz-per-second period every pacer here uses.
    fn period() -> Duration {
        Duration::from_secs_f64(1.0 / 60.0)
    }

    fn pacer_60(base: Instant) -> FramePacer {
        FramePacer::new(Some(period()), base)
    }

    #[test]
    fn the_default_window_is_paced() {
        assert_eq!(WindowConfig::default().target_fps, Some(60));
        assert_eq!(WindowConfig::default().frame_period(), Some(period()));
        // A zero rate is not "pace to zero frames a second", it is "do not pace".
        assert_eq!(
            WindowConfig::default().with_target_fps(Some(0)).target_fps,
            None
        );
        assert!(
            WindowConfig::default()
                .with_target_fps(None)
                .frame_period()
                .is_none()
        );
    }

    #[test]
    fn the_first_frame_is_due_immediately() {
        let base = Instant::now();
        let pacer = pacer_60(base);
        assert!(
            pacer.wait_for(base).is_none(),
            "the window must draw at once"
        );
    }

    #[test]
    fn a_paced_frame_waits_out_its_period() {
        let base = Instant::now();
        let mut pacer = pacer_60(base);
        pacer.mark(base);
        // 10 ms later there is still a wait to serve...
        let remaining = pacer.wait_for(base + Duration::from_millis(10));
        assert!(
            remaining.is_some_and(|r| r > Duration::from_millis(6)),
            "{remaining:?}"
        );
        // ...and 20 ms later the frame is overdue.
        assert!(pacer.wait_for(base + Duration::from_millis(20)).is_none());
    }

    #[test]
    fn the_cadence_does_not_drift_with_the_work_a_frame_does() {
        // The bug this guards: scheduling "one period after the frame finished"
        // adds every millisecond of work to the period, so a 16.7 ms target
        // becomes 20.7 ms and then 24.7 ms, and the frame rate sags away from
        // the one that was asked for.
        let base = Instant::now();
        let mut pacer = pacer_60(base);
        let mut deadline = base;
        let mut now = base;
        for _ in 0..120 {
            // The frame finished at `now`; the next one starts on the grid.
            pacer.mark(now);
            deadline += period();
            assert_eq!(pacer.next_deadline(), deadline, "the phase drifted");
            now = deadline + Duration::from_millis(4);
        }
    }

    #[test]
    fn a_frame_that_missed_several_periods_resynchronises_instead_of_bursting() {
        let base = Instant::now();
        let mut pacer = pacer_60(base);
        pacer.mark(base);
        // A 200 ms stall — a debugger pause, a slow first chunk.
        let after_stall = base + Duration::from_millis(200);
        pacer.mark(after_stall);
        let deadline = pacer.next_deadline();
        assert!(
            deadline > after_stall,
            "the next frame is in the past: {deadline:?}"
        );
        assert!(
            deadline <= after_stall + period(),
            "it must resume on the next grid point, not queue {} frames to catch up",
            (deadline - after_stall).as_secs_f64() / period().as_secs_f64()
        );
    }

    #[test]
    fn an_uncapped_pacer_never_waits() {
        let base = Instant::now();
        let mut pacer = FramePacer::new(None, base);
        assert!(pacer.is_uncapped());
        for _ in 0..5 {
            pacer.mark(base);
            assert!(pacer.wait_for(base).is_none());
        }
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
    fn a_fresh_window_counts_the_cursor_as_inside() {
        // A window you just opened is one you are looking at. Defaulting to
        // `false` meant the first click before any mouse *movement* was
        // dropped, because the interface could not place the pointer and so
        // refused to hover anything — which is indistinguishable, from the
        // outside, from a button that does not work.
        assert!(Input::default().cursor_inside);
    }

    #[test]
    fn escape_closes_the_window_unless_the_game_asks_for_the_key() {
        // A demo wants one key to get out of a viewer. A game wants Escape to
        // back out of a menu, and a host that closes on it takes the key before
        // the game ever sees it — which is what made holding Escape quit the
        // session in the middle of a farm.
        assert!(
            WindowConfig::default().quit_on_escape,
            "the demo convention is the default"
        );
        assert!(!WindowConfig::default().keep_escape().quit_on_escape);
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
