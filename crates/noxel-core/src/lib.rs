//! # Noxel Core
//!
//! Foundation layer of the Noxel engine. **Zero dependencies, zero `unsafe`.**
//!
//! Everything in here is deliberately boring and general: maths, deterministic
//! random numbers, frame timing, a small cooperative job system, object pools,
//! a double-buffered event bus and the spatial acceleration structures that the
//! physics, visibility and ray-tracing layers all share.
//!
//! ## Why this crate exists separately
//!
//! Every other Noxel crate depends on `noxel-core` and nothing below it. That
//! gives contributors (human or AI) a single, small, heavily tested surface to
//! learn before they can work anywhere in the engine. See
//! `docs/01-architecture.md` for the full dependency rules.
//!
//! ## Module map
//!
//! | Module | Purpose |
//! |---|---|
//! | [`math`] | `Vec2/3/4`, `Mat3/4`, `Quat`, shapes, `Frustum`, colour |
//! | [`rng`] | `Pcg32` + splittable [`rng::RngStream`]s for reproducible worlds |
//! | [`time`] | fixed-step clock, frame timing, [`time::Profiler`] |
//! | [`jobs`] | deterministic fork/join task pool |
//! | [`pool`] | generational slot maps, free lists, bitsets, ring buffers |
//! | [`spatial`] | uniform grid, spatial hash, dynamic BVH, ray queries |
//! | [`events`] | typed double-buffered event bus |
//!
//! ## Example
//!
//! ```
//! use noxel_core::math::{Vec3, Mat4};
//! use noxel_core::rng::Pcg32;
//!
//! let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
//! let p = m.transform_point3(Vec3::ZERO);
//! assert_eq!(p, Vec3::new(1.0, 2.0, 3.0));
//!
//! let mut rng = Pcg32::new(42);
//! let _ = rng.next_u32();
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod events;
pub mod jobs;
pub mod math;
pub mod pool;
pub mod rng;
pub mod spatial;
pub mod time;

/// Re-export of the most commonly used items.
///
/// `use noxel_core::prelude::*;` is the intended way for other Noxel crates to
/// pull in the core vocabulary.
pub mod prelude {
    pub use crate::events::{EventBus, EventReader};
    pub use crate::jobs::{JobPool, Progress};
    pub use crate::math::{
        Aabb, Color, Frustum, Mat3, Mat4, Plane, Quat, Ray, Rect, Sphere, Transform, Vec2, Vec3,
        Vec4,
    };
    pub use crate::pool::{Handle, SlotMap};
    pub use crate::rng::{Pcg32, RngStream};
    pub use crate::spatial::{Bvh, RayHit, SpatialHash, UniformGrid};
    pub use crate::time::{FrameTiming, GameClock, Profiler};
}

/// The engine version string, taken from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default world unit interpretation: **1.0 world unit == 1 metre**.
///
/// Noxel keeps every spatial subsystem (physics, culling, audio falloff,
/// ray-tracing epsilon) on the same scale so a single set of tuning constants
/// works everywhere. Do not introduce a second scale.
pub const METER: f32 = 1.0;

/// A conventional tile edge length in world units.
///
/// Top-down pixel-art RPGs are authored on a grid. The engine never assumes
/// this value internally, but the demo project and the world generator default
/// to it, and `docs/guides/world-building.md` recommends keeping it.
pub const DEFAULT_TILE_SIZE: f32 = 1.0;
