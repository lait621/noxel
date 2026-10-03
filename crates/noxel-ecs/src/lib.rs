//! # Noxel ECS
//!
//! A small, safe, dependency-free entity-component-system.
//!
//! ```no_run
//! use noxel_ecs::{Scheduler, World};
//! use noxel_core::math::Vec3;
//!
//! #[derive(Clone, Copy, Debug, PartialEq)]
//! struct Position(Vec3);
//! #[derive(Clone, Copy, Debug)]
//! struct Velocity(Vec3);
//!
//! struct Ctx { world: World, dt: f32 }
//!
//! let mut world = World::new();
//! let npc = world.spawn_named("npc-0");
//! world.insert(npc, Position(Vec3::ZERO));
//! world.insert(npc, Velocity(Vec3::new(1.0, 0.0, 0.0)));
//!
//! let mut scheduler: Scheduler<Ctx> = Scheduler::new();
//! scheduler.add_stage("sim");
//! scheduler.add_system("sim", "integrate", |c: &mut Ctx| {
//!     let dt = c.dt;
//!     c.world.for_each2_mut::<Position, Velocity, _>(|_e, p, v| p.0 += v.0 * dt);
//! });
//!
//! let mut ctx = Ctx { world, dt: 1.0 / 60.0 };
//! scheduler.run(&mut ctx);
//! assert!(ctx.world.get::<Position>(npc).unwrap().0.x > 0.0);
//! ```
//!
//! # Design notes
//!
//! * **Entities are generational ids.** A despawned id never resolves, even
//!   after its slot is recycled. See [`Entity`].
//! * **Components are any `Send + Sync + 'static` type.** No derive macro, no
//!   registration. `struct Health(f32);` is already a component.
//! * **Storage is a sparse set per component type**, held in a flat `Vec` so
//!   two of them can be split-borrowed at once. That is what makes
//!   [`World::for_each2_mut2`] safe without `unsafe` and without runtime borrow
//!   tracking.
//! * **Queries are closures**, not iterator combinators. See the [`world`]
//!   module docs for why.
//! * **The scheduler is generic over its context**, so this crate never needs to
//!   know about the renderer or physics.
//!
//! # What this crate deliberately does *not* do
//!
//! * No archetypes. Noxel entities carry 4–20 components and the systems are
//!   dominated by "iterate one store, look up one or two others", so a sparse
//!   set wins on simplicity and loses almost nothing on cache behaviour.
//! * No automatic system parallelism. Determinism is worth more here than the
//!   few percent a dependency-driven parallel scheduler would add; coarse
//!   parallelism is available explicitly through [`noxel_core::jobs`].
//! * No reflection, no runtime type registry, no serialisation. Saving a game is
//!   the game's job; `noxel-world` ships the format for world *data*.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod entity;
pub mod scheduler;
pub mod storage;
pub mod world;

pub use entity::Entity;
pub use scheduler::{Scheduler, Stage, SystemEntry, SystemStat};
pub use storage::{ErasedStorage, Storage};
pub use world::{Component, Resource, World, WorldStats};

/// The types most systems need.
pub mod prelude {
    pub use crate::entity::Entity;
    pub use crate::scheduler::Scheduler;
    pub use crate::world::{Component, Resource, World};
}

/// The crate version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
