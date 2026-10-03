//! The real window host: winit for events, softbuffer for pixels.
//!
//! Both crates are optional and only this file sees them. Everything above the
//! [`Host`](crate::Host) trait is engine code that has never heard of either.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::{Host, Input, WindowConfig, key};

use crate::WindowError;

/// Opens a window and runs the host until the window closes (or `exit_after`
/// elapses).
///
/// # Errors
/// Returns [`WindowError::EventLoop`] when the platform refuses to start an event
/// loop, and [`WindowError::Surface`] when it starts but no drawable surface can
/// be created for the window.
pub fn run<H: Host>(config: WindowConfig, host: H) -> Result<(), WindowError> {
    let event_loop = EventLoop::new().map_err(|e| WindowError::EventLoop(e.to_string()))?;
    // `Wait`: do nothing until there is an event. The frame loop is driven by
    // `request_redraw`, which the compositor throttles, so this does not spin.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut handler = Handler {
        config,
        host,
        window: None,
        surface: None,
        context: None,
        input: Input::new(),
        last_frame: Instant::now(),
        started: Instant::now(),
        frames: 0,
    };
    event_loop
        .run_app(&mut handler)
        .map_err(|e| WindowError::EventLoop(e.to_string()))
}

struct Handler<H: Host> {
    config: WindowConfig,
    host: H,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    input: Input,
    last_frame: Instant,
    started: Instant,
    frames: u64,
}

impl<H: Host> ApplicationHandler for Handler<H> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                f64::from(self.config.window.0),
                f64::from(self.config.window.1),
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("noxel-window: could not create a window: {error}");
                event_loop.exit();
                return;
            }
        };
        let window = Arc::new(window);
        match softbuffer::Context::new(window.clone()) {
            Ok(context) => match softbuffer::Surface::new(&context, window.clone()) {
                Ok(surface) => {
                    self.surface = Some(surface);
                    self.context = Some(context);
                }
                Err(error) => {
                    eprintln!("noxel-window: no drawable surface: {error}");
                    event_loop.exit();
                    return;
                }
            },
            Err(error) => {
                eprintln!("noxel-window: no display context: {error}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.last_frame = Instant::now();
        self.started = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.host.shutdown();
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let mapped = map_key(code);
                    if mapped != 0 {
                        match event.state {
                            ElementState::Pressed => self.input.press(mapped),
                            ElementState::Released => self.input.release(mapped),
                        }
                        // Shift/control/alt are read as flags as well as codes,
                        // because a game wants "is shift down" far more often
                        // than "was shift pressed on this exact frame".
                        match mapped {
                            key::SHIFT => self.input.shift = event.state == ElementState::Pressed,
                            key::CONTROL => {
                                self.input.control = event.state == ElementState::Pressed;
                            }
                            key::ALT => self.input.alt = event.state == ElementState::Pressed,
                            _ => {}
                        }
                        // Escape quits, which is the one convention every player
                        // expects and no game should have to implement.
                        if mapped == key::ESCAPE && event.state == ElementState::Pressed {
                            self.host.shutdown();
                            event_loop.exit();
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let next = (position.x as f32, position.y as f32);
                let previous = self.input.mouse;
                self.input.mouse = next;
                self.input.mouse_delta.0 += next.0 - previous.0;
                self.input.mouse_delta.1 += next.1 - previous.1;
                // A game hit-tests in framebuffer pixels, so the host converts
                // here rather than leaving every caller to redo the arithmetic.
                let presentation =
                    self.config
                        .presentation(self.window.as_ref().map_or((1, 1), |w| {
                            let size = w.inner_size();
                            (size.width.max(1), size.height.max(1))
                        }));
                self.input.cursor = presentation.cursor_to_framebuffer(next);
                self.input.cursor_inside = presentation.contains_cursor(next);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let slot = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                match state {
                    ElementState::Pressed => self.input.press_mouse(slot),
                    ElementState::Released => self.input.release_mouse(slot),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 16.0,
                };
            }
            WindowEvent::Focused(false) => {
                // Otherwise the character keeps walking while the window is in
                // the background, which is the single most common input bug in a
                // desktop game.
                self.input.release_all();
            }
            WindowEvent::RedrawRequested => {
                self.frame();
                // A host that wants to stop asks for it here rather than by
                // panicking or by closing its own window.
                if self.host.should_quit() {
                    self.host.shutdown();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(exit_after) = self.config.exit_after {
            if self.started.elapsed().as_secs_f32() >= exit_after {
                self.host.shutdown();
                event_loop.exit();
                return;
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        } else {
            event_loop.exit();
        }
    }
}

impl<H: Host> Handler<H> {
    /// Steps the host once and puts the result on screen.
    fn frame(&mut self) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface.as_mut()) else {
            return;
        };

        let now = Instant::now();
        // Clamp a long stall: after a debugger pause or an alt-tab, a 30-second
        // `dt` would teleport the player. `GameClock` caps the substeps as well,
        // so this is belt and braces.
        let dt = (now - self.last_frame).as_secs_f32().min(0.25);
        self.last_frame = now;

        let framebuffer = self.host.step(dt, &self.input);
        let image = framebuffer.resolve(&self.config.internal_resolve());
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        // The same `Presentation` the cursor is mapped through, so the image and
        // the pointer can never disagree about where the framebuffer is.
        let presentation = self.config.presentation((width, height));
        blit(
            &image.pixels,
            image.width,
            image.height,
            &mut buffer,
            width,
            height,
            presentation.scale,
            self.config.background,
        );
        // A failed present is worth hearing about exactly once, and never worth
        // taking the frame loop down for: the next frame will try again.
        if let Err(error) = buffer.present() {
            if self.frames < 2 {
                eprintln!("noxel-window: could not present a frame: {error}");
            }
        }

        self.frames += 1;
        let suffix = self.host.title_suffix();
        if !suffix.is_empty() && self.frames % 30 == 1 {
            window.set_title(&format!("{} — {suffix}", self.config.title));
        }
        self.input.end_frame();
    }
}

impl WindowConfig {
    /// The resolve settings a window uses.
    ///
    /// A window presents the final image, so the tone curve and the sRGB
    /// conversion both belong here — this is the last step before a human sees
    /// the pixels.
    fn internal_resolve(&self) -> noxel_render::framebuffer::ResolveSettings {
        noxel_render::framebuffer::ResolveSettings::default()
    }
}

/// Scales an RGBA image into a softbuffer pixel buffer.
///
/// `scale == 0` stretches to fill; anything else is a whole-number upscale
/// centred in the window with the background showing through the letterbox. That
/// is what keeps a 320x180 frame looking like pixel art rather than like a
/// blurry low-resolution 3D render (see `docs/guides/windowing.md`).
#[allow(clippy::too_many_arguments)]
fn blit(
    pixels: &[noxel_core::math::Color8],
    source_width: u32,
    source_height: u32,
    out: &mut [u32],
    width: u32,
    height: u32,
    scale: u32,
    background: [u8; 3],
) {
    let fill =
        u32::from(background[0]) << 16 | u32::from(background[1]) << 8 | u32::from(background[2]);
    out.fill(fill);

    if source_width == 0 || source_height == 0 || pixels.is_empty() {
        return;
    }
    // `scale == 0` means stretch. Keep the request and the derived geometry
    // separate: shadowing `scale` with the letterbox factor here silently turns
    // the stretch path into a 1:1 crop, which is the kind of bug that shows up as
    // "the window is mostly background colour".
    let (scale, offset_x, offset_y) = if scale == 0 {
        (0u32, 0i64, 0i64)
    } else {
        let drawn_w = source_width * scale;
        let drawn_h = source_height * scale;
        let ox = (i64::from(width) - i64::from(drawn_w)) / 2;
        let oy = (i64::from(height) - i64::from(drawn_h)) / 2;
        (scale, ox, oy)
    };

    // Two samplers, chosen once rather than per pixel: a typed branch inside a
    // per-pixel loop is the kind of thing that quietly costs a millisecond at
    // 1080p, and the compiler cannot hoist it out of the match on `scale`.
    if scale == 0 {
        for y in 0..height {
            let sy = (u64::from(y) * u64::from(source_height) / u64::from(height.max(1))) as u32;
            let source_row = (sy.min(source_height - 1) as usize) * (source_width as usize);
            let out_row = (y as usize) * (width as usize);
            for x in 0..width {
                let sx = (u64::from(x) * u64::from(source_width) / u64::from(width.max(1))) as u32;
                let source = pixels[source_row + sx.min(source_width - 1) as usize];
                out[out_row + x as usize] = pack(source);
            }
        }
        return;
    }

    // Whole-number upscale, centred. `scale` is at least 1 here, which is what
    // makes the divisions total.
    let scale = scale.max(1);
    for y in 0..height {
        let Some(sy) = sample_axis(i64::from(y) - offset_y, scale) else {
            continue;
        };
        if sy >= source_height {
            continue;
        }
        let source_row = (sy as usize) * (source_width as usize);
        let out_row = (y as usize) * (width as usize);
        for x in 0..width {
            let Some(sx) = sample_axis(i64::from(x) - offset_x, scale) else {
                continue;
            };
            if sx >= source_width {
                continue;
            }
            out[out_row + x as usize] = pack(pixels[source_row + sx as usize]);
        }
    }
}

/// Which source pixel a destination coordinate samples, or `None` when it falls
/// in the letterbox.
fn sample_axis(local: i64, scale: u32) -> Option<u32> {
    if local < 0 {
        return None;
    }
    Some((local as u64 / u64::from(scale)) as u32)
}

/// Packs an RGBA colour into softbuffer's `0x00RRGGBB`.
fn pack(color: noxel_core::math::Color8) -> u32 {
    u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

/// Maps a platform key to an engine key code.
fn map_key(code: KeyCode) -> u32 {
    match code {
        KeyCode::KeyA => key::A,
        KeyCode::KeyB => key::letter('B'),
        KeyCode::KeyC => key::C,
        KeyCode::KeyD => key::D,
        KeyCode::KeyE => key::E,
        KeyCode::KeyF => key::F,
        KeyCode::KeyG => key::letter('G'),
        KeyCode::KeyH => key::letter('H'),
        KeyCode::KeyI => key::letter('I'),
        KeyCode::KeyJ => key::letter('J'),
        KeyCode::KeyK => key::letter('K'),
        KeyCode::KeyL => key::letter('L'),
        KeyCode::KeyM => key::M,
        KeyCode::KeyN => key::letter('N'),
        KeyCode::KeyO => key::letter('O'),
        KeyCode::KeyP => key::P,
        KeyCode::KeyQ => key::Q,
        KeyCode::KeyR => key::R,
        KeyCode::KeyS => key::S,
        KeyCode::KeyT => key::letter('T'),
        KeyCode::KeyU => key::letter('U'),
        KeyCode::KeyV => key::letter('V'),
        KeyCode::KeyW => key::W,
        KeyCode::KeyX => key::letter('X'),
        KeyCode::KeyY => key::letter('Y'),
        KeyCode::KeyZ => key::letter('Z'),
        KeyCode::Digit0 => key::digit(0),
        KeyCode::Digit1 => key::digit(1),
        KeyCode::Digit2 => key::digit(2),
        KeyCode::Digit3 => key::digit(3),
        KeyCode::Digit4 => key::digit(4),
        KeyCode::Digit5 => key::digit(5),
        KeyCode::Digit6 => key::digit(6),
        KeyCode::Digit7 => key::digit(7),
        KeyCode::Digit8 => key::digit(8),
        KeyCode::Digit9 => key::digit(9),
        KeyCode::Escape => key::ESCAPE,
        KeyCode::Space => key::SPACE,
        KeyCode::Enter | KeyCode::NumpadEnter => key::ENTER,
        KeyCode::Tab => key::TAB,
        KeyCode::Backspace => key::BACKSPACE,
        KeyCode::Delete => key::DELETE,
        KeyCode::Insert => key::INSERT,
        KeyCode::Home => key::HOME,
        KeyCode::End => key::END,
        KeyCode::PageUp => key::PAGE_UP,
        KeyCode::PageDown => key::PAGE_DOWN,
        KeyCode::ArrowLeft => key::LEFT,
        KeyCode::ArrowRight => key::RIGHT,
        KeyCode::ArrowUp => key::UP,
        KeyCode::ArrowDown => key::DOWN,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => key::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => key::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => key::ALT,
        KeyCode::F1 => key::F1,
        KeyCode::F2 => key::F2,
        KeyCode::F3 => key::F3,
        KeyCode::F4 => key::F4,
        KeyCode::F5 => key::F5,
        KeyCode::F6 => key::F6,
        KeyCode::F7 => key::F7,
        KeyCode::F8 => key::F8,
        KeyCode::F9 => key::F9,
        KeyCode::F10 => key::F10,
        KeyCode::F11 => key::F11,
        KeyCode::F12 => key::F12,
        _ => 0,
    }
}

/// The named key for a code, for a debug readout. Unused codes return `None`.
#[allow(dead_code)]
fn named(code: KeyCode) -> Option<NamedKey> {
    match code {
        KeyCode::Escape => Some(NamedKey::Escape),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Color8;

    fn pixels(width: u32, height: u32, color: Color8) -> Vec<Color8> {
        vec![color; (width * height) as usize]
    }

    #[test]
    fn every_letter_and_digit_maps() {
        for (code, expected) in [
            (KeyCode::KeyA, key::A),
            (KeyCode::KeyZ, key::letter('Z')),
            (KeyCode::Digit0, key::digit(0)),
            (KeyCode::Digit9, key::digit(9)),
            (KeyCode::Space, key::SPACE),
            (KeyCode::Escape, key::ESCAPE),
            (KeyCode::ArrowLeft, key::LEFT),
            (KeyCode::ArrowUp, key::UP),
            (KeyCode::ShiftLeft, key::SHIFT),
            (KeyCode::F8, key::F8),
        ] {
            assert_eq!(map_key(code), expected, "{code:?}");
        }
    }

    #[test]
    fn unmapped_keys_are_zero() {
        assert_eq!(map_key(KeyCode::BrowserBack), 0, "an unmapped key is zero");
        assert_eq!(map_key(KeyCode::NumpadAdd), 0);
    }

    #[test]
    fn stretch_fills_the_whole_buffer() {
        let source = pixels(2, 2, Color8::new(255, 0, 0, 255));
        let mut out = vec![0u32; 8 * 4];
        blit(&source, 2, 2, &mut out, 8, 4, 0, [0, 0, 0]);
        assert!(
            out.iter().all(|p| *p == 0x00FF_0000),
            "every pixel is the source colour"
        );
    }

    #[test]
    fn integer_scale_letterboxes() {
        let source = pixels(2, 2, Color8::new(0, 255, 0, 255));
        let mut out = vec![0u32; 6 * 6];
        blit(&source, 2, 2, &mut out, 6, 6, 2, [1, 2, 3]);
        // 2x2 source at scale 2 is 4x4, centred in 6x6: one pixel of border.
        let expected_border = 0x0001_0203;
        assert_eq!(out[0], expected_border, "the corner is letterbox");
        assert_eq!(out[7], 0x0000_FF00, "the art starts one pixel in");
        assert_eq!(
            out[6 * 5],
            expected_border,
            "the last row is letterbox again"
        );
    }

    #[test]
    fn blit_survives_a_degenerate_source() {
        let mut out = vec![7u32; 4];
        blit(&[], 0, 0, &mut out, 2, 2, 1, [9, 9, 9]);
        assert!(out.iter().all(|p| *p == 0x0009_0909));
    }

    #[test]
    fn blit_survives_a_source_larger_than_the_window() {
        let source = pixels(64, 64, Color8::new(10, 20, 30, 255));
        let mut out = vec![0u32; 4];
        blit(&source, 64, 64, &mut out, 2, 2, 1, [0, 0, 0]);
        assert!(out.iter().all(|p| *p == 0x000A_141E));
    }

    #[test]
    fn blit_writes_rgb_in_the_order_softbuffer_expects() {
        let source = vec![Color8::new(0x12, 0x34, 0x56, 255)];
        let mut out = vec![0u32; 1];
        blit(&source, 1, 1, &mut out, 1, 1, 1, [0, 0, 0]);
        assert_eq!(out[0], 0x0012_3456, "0x00RRGGBB");
    }
}
