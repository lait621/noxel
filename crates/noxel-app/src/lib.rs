//! # Noxel app
//!
//! The runtime host: it owns the world, the entities, the physics, the camera,
//! the renderer and the frame loop, and hands them to plugins.
//!
//! ```no_run
//! use noxel_app::{App, AppConfig, Plugin};
//!
//! struct Hello;
//! impl Plugin for Hello {
//!     fn name(&self) -> &str { "hello" }
//!     fn update(&mut self, app: &mut App, dt: f32) {
//!         app.debug_mut().record_counter("dt_ms", dt * 1000.0);
//!     }
//! }
//!
//! let mut app = App::new(AppConfig::default()).unwrap();
//! app.add_plugin(Hello);
//! // app.run_headless(600);
//! ```
//!
//! ## What an "app" is, and is not
//!
//! It is **not** a windowing layer. Opening a window needs a platform API, and
//! Noxel has no dependencies (`docs/adr/0002-no-dependencies.md`). An `App`
//! produces a [`Framebuffer`](noxel_render::Framebuffer) and consumes an
//! [`InputState`]; presenting that buffer is the host's job — the `town-demo`
//! example writes PNG frames, and `docs/guides/windowing.md` shows how to plug
//! in `winit` or `SDL2` behind a feature flag.
//!
//! What the app *does* own is the part that is easy to get wrong and tedious to
//! write twice: a fixed-timestep clock, a well-ordered frame, and a single place
//! where every subsystem is available to every plugin.
//!
//! ## Frame order
//!
//! ```text
//! poll input
//!   -> begin_frame        (debug starts timing)
//!   -> fixed update       (0..n substeps of physics + gameplay + camera)
//!   -> stream             (load chunks around the camera)
//!   -> visibility         (frustum, distance, occlusion, fades)
//!   -> render             (raster, hybrid or ray trace)
//!   -> overlay            (debug panels, drawn after the image is shaded)
//!   -> post + resolve     (tone map, dither, sRGB)
//!   -> end_frame          (publish the statistics)
//! ```
//!
//! The order matters: streaming and visibility must run *after* the camera has
//! moved, or the world lags a frame behind the player.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod app;
pub mod plugin;
pub mod runner;

pub use app::{App, AppConfig, AppContext, AppError, InputState};
pub use noxel_debug::{DebugConfig, DumpFormat};
pub use plugin::{Plugin, PluginRegistry};
pub use runner::{HeadlessReport, RunMode};

/// The crate version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
