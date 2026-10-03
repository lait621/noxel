//! The window host: what turns the demo from a PNG generator into a game.
//!
//! Everything here is behind the `window` feature, because everything here needs
//! a platform API and the engine does not have one
//! (`docs/adr/0002-no-dependencies.md`). Without the feature the module is not
//! compiled at all, and `--window` prints an explanation instead of failing.

use noxel_app::{App, InputState};
use noxel_render::Framebuffer;
use noxel_window::{Host, Input, WindowConfig, key};

use crate::args::Args;

/// The demo, driven by a window instead of by a frame counter.
pub struct Interactive {
    app: App,
    /// Seconds of real time since the window opened.
    elapsed: f32,
    /// Frames the window has presented.
    frames: u64,
    /// Whether the statistics overlay is currently drawn.
    overlay: bool,
    /// The frame dump key, latched: `F8` is a *request*, not a state.
    dump_requested: bool,
    /// Stop after this many frames, so `--window --frames N` terminates and a
    /// script can use it.
    quit_after: Option<u32>,
}

impl Interactive {
    /// Wraps an app.
    #[must_use]
    pub fn new(app: App, overlay: bool, quit_after: Option<u32>) -> Self {
        Self {
            app,
            elapsed: 0.0,
            frames: 0,
            overlay,
            dump_requested: false,
            quit_after,
        }
    }

    /// Handles the keys that are the *demo's* business rather than the engine's.
    fn handle_hotkeys(&mut self, input: &Input) {
        if input.was_pressed(key::F1) {
            self.overlay = !self.overlay;
            let mut config = self.app.debug().config().clone();
            config.stats_panel = self.overlay;
            config.frame_graph = self.overlay;
            config.camera_panel = self.overlay;
            config.culling_panel = self.overlay;
            self.app.debug_mut().set_config(config);
        }
        if input.was_pressed(key::F8) {
            self.dump_requested = true;
        }
    }
}

impl Host for Interactive {
    fn step(&mut self, dt: f32, input: &Input) -> &Framebuffer {
        // The engine's `InputState` and the window's `Input` have the same shape
        // on purpose; reconciling through the *edges* keeps the two in step
        // without either one having to know about the other.
        sync_input(input, self.app.input_mut());
        self.handle_hotkeys(input);

        self.elapsed += dt;
        self.app.step(dt);
        self.frames += 1;

        // F8 is a request, not a state, so it is consumed here rather than read
        // from `input` again on the next frame.
        if std::mem::take(&mut self.dump_requested) {
            match self.app.dump_current_frame() {
                Ok(Some(path)) => println!("wrote {}", path.display()),
                Ok(None) => println!("frame dumping is off; run with --dump DIR to enable it"),
                Err(error) => eprintln!("could not write the frame: {error}"),
            }
        }
        self.app.framebuffer()
    }

    fn should_quit(&self) -> bool {
        self.quit_after
            .is_some_and(|limit| self.frames >= u64::from(limit))
    }

    fn internal_size(&self) -> (u32, u32) {
        let framebuffer = self.app.framebuffer();
        (framebuffer.width(), framebuffer.height())
    }

    fn title_suffix(&self) -> String {
        let stats = self.app.debug().stats();
        // The window title is the cheapest debug overlay there is: it costs a
        // `set_title` every thirtieth frame and tells you the frame rate without
        // drawing over the art.
        format!(
            "{} frames · {:.0} fps · {} chunks · {} drawn",
            self.frames,
            if self.elapsed > 0.0 {
                self.frames as f32 / self.elapsed
            } else {
                0.0
            },
            self.app.context.streamer.stats().loaded,
            stats.current.instances
        )
    }
}

/// Copies the window's input into the engine's, using the frame's edges.
pub fn sync_input(from: &Input, to: &mut InputState) {
    for code in from.released() {
        to.release(*code);
    }
    for code in from.pressed() {
        to.press(*code);
    }
    to.mouse = from.mouse;
    to.mouse_delta = from.mouse_delta;
    to.scroll = from.scroll;
    to.mouse_buttons = from.mouse_buttons;
}

/// The window configuration for these arguments.
#[must_use]
pub fn window_config(args: &Args) -> WindowConfig {
    WindowConfig::default()
        .with_title(format!("Noxel — town-demo (seed {:#x})", args.seed))
        .with_internal(args.size.0, args.size.1)
}

/// Runs the demo in a window.
///
/// # Errors
/// Returns the window's error when the platform cannot provide one. With the
/// `window` feature off this always fails, with a message explaining how to turn
/// it on.
pub fn run(args: &Args, app: App) -> Result<(), noxel_window::WindowError> {
    let config = window_config(args);
    // Only an explicit `--frames` closes the window. Without one the player
    // decides when they are done, which is the entire point of a window.
    let quit_after = if args.frames_given {
        Some(args.frames as u32)
    } else {
        None
    };
    let host = Interactive::new(app, args.debug, quit_after);
    noxel_window::run(config, host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_input_copies_keys_and_mouse() {
        let mut from = Input::new();
        from.press(key::W);
        from.mouse = (3.0, 4.0);
        from.mouse_delta = (1.0, 2.0);
        from.mouse_buttons[0] = true;

        let mut to = InputState::new();
        sync_input(&from, &mut to);
        assert!(to.is_held(key::W));
        assert_eq!(to.mouse, (3.0, 4.0));
        assert_eq!(to.mouse_delta, (1.0, 2.0));
        assert!(to.mouse_buttons[0]);
    }

    #[test]
    fn sync_input_propagates_a_release() {
        let mut from = Input::new();
        let mut to = InputState::new();
        from.press(key::W);
        sync_input(&from, &mut to);
        assert!(to.is_held(key::W));

        from.end_frame();
        from.release(key::W);
        sync_input(&from, &mut to);
        assert!(!to.is_held(key::W), "the release must reach the engine");
        assert!(to.was_released(key::W));
    }

    #[test]
    fn sync_input_is_idempotent_within_a_frame() {
        let mut from = Input::new();
        from.press(key::A);
        let mut to = InputState::new();
        sync_input(&from, &mut to);
        sync_input(&from, &mut to);
        assert!(to.is_held(key::A));
        assert!(to.was_pressed(key::A));
    }

    #[test]
    fn the_window_title_reports_the_seed() {
        let args = Args {
            seed: 0xABCD,
            ..Args::default()
        };
        let config = window_config(&args);
        assert!(config.title.contains("abcd"), "{}", config.title);
        assert_eq!(config.internal, args.size);
    }

    #[test]
    fn escape_is_never_mapped_to_a_movement_key() {
        // Escape quits, so it must not also be `A` or `D` or the player would
        // walk sideways on the way out.
        assert_ne!(key::ESCAPE, key::A);
        assert_ne!(key::ESCAPE, key::D);
        assert_ne!(key::ESCAPE, key::W);
        assert_ne!(key::ESCAPE, key::S);
    }
}
