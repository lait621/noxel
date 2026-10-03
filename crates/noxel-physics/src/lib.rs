//! # Noxel Physics
//!
//! A fixed-timestep, deterministic, **safe Rust** rigid-body engine for a
//! top-down 3D pixel-art RPG. Zero third-party dependencies; the only crate
//! below it is [`noxel_core`], whose maths, slot map and spatial hash it
//! reuses rather than re-implementing.
//!
//! ## What it is tuned for
//!
//! A town full of crates, barrels, fences and NPCs, read from above, where the
//! player feels the character controller more than they feel the rigid bodies.
//! Concretely:
//!
//! | Decision | Why |
//! |---|---|
//! | Fixed step, `dt` clamped to `max_step_delta` | Determinism and no tunnelling |
//! | Sequential impulses, no warm starting | The contact set is rebuilt every step, which keeps repeated runs bit-identical |
//! | Contacts exchange **yaw only** | A crate can never be tipped over by friction; pitch/roll stays available to gameplay code |
//! | Boxes are fully oriented; cylinders collide as their bounding box | Exact box SAT with one documented approximation for barrels |
//! | Sleeping bodies act as immovable until a *moving* body touches them | A resting stack settles instead of buzzing |
//! | Non-finite state is recovered, not propagated | One bad gameplay value cannot poison the world |
//!
//! ## Quick start
//!
//! ```
//! use noxel_core::math::{Aabb, Vec3};
//! use noxel_physics::{BodyDesc, ColliderShape, PhysicsConfig, PhysicsWorld};
//!
//! let mut world = PhysicsWorld::new(PhysicsConfig::default());
//! world.insert_static_aabb(
//!     Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)),
//!     0,
//! );
//! let crate_ = world.insert(
//!     BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) })
//!         .at(Vec3::new(0.0, 4.0, 0.0)),
//! );
//!
//! for _ in 0..240 {
//!     world.step(1.0 / 60.0);
//! }
//! assert!((world.body(crate_).unwrap().position.y - 0.5).abs() < 0.01);
//! ```
//!
//! ## Module map
//!
//! | Module | Contents |
//! |---|---|
//! | `shape` | collider shapes, bounds, support functions, ray tests |
//! | `body` | bodies, descriptors, layers, sleep state |
//! | `world` | broadphase, integration, contacts, events, stats |
//! | `query` | ray casts, overlap queries, shape sweeps |
//! | `character` | the kinematic character controller |
//!
//! ## Determinism
//!
//! Nothing in the step iterates a `HashMap`. Bodies are visited in slot-map
//! order, broadphase pairs and contacts are sorted by [`BodyHandle`], and
//! previous-frame pair sets are ordered trees. Two runs of the same scenario in
//! the same binary produce bit-identical body states.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

mod body;
mod character;
mod config;
mod events;
mod narrow;
mod query;
mod shape;
mod solver;
mod world;

pub use crate::body::{
    Body, BodyDesc, BodyHandle, BodyKind, DEFAULT_DENSITY, LAYER_ALL, LAYER_CAMERA, LAYER_NPC,
    LAYER_PLAYER, LAYER_PROP, LAYER_TRIGGER, LAYER_WORLD,
};
pub use crate::character::{CharacterMove, GROUND_SNAP_DISTANCE, MAX_SLOPE_ANGLE, STEP_HEIGHT};
pub use crate::config::{DEFAULT_CELL_SIZE, PhysicsConfig};
pub use crate::events::PhysicsEvent;
pub use crate::narrow::Contact;
pub use crate::query::{QueryFilter, RaycastHit};
pub use crate::shape::ColliderShape;
pub use crate::solver::yaw_inertia;
pub use crate::world::{PhysicsStats, PhysicsWorld};
