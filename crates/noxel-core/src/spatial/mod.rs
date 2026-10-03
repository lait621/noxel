//! Spatial acceleration structures.
//!
//! Three structures, each tuned for a different lifetime and query shape. Pick
//! by asking *when does the data change?*
//!
//! | Structure | Lifetime | Best for | Cost |
//! |---|---|---|---|
//! | [`UniformGrid`] | static (built once) | world tiles, building & occluder lookup, ray marching through a level | build O(n), query O(cells touched) |
//! | [`SpatialHash`] | dynamic (per frame) | NPC neighbour search, physics broadphase, trigger volumes | insert/update O(1), query O(cell contents) |
//! | [`Bvh`] | static-ish (rebuilt per mesh / per frame) | ray casting for occlusion & ray tracing, frustum queries over many primitives | build O(n log n), query O(log n + k) |
//!
//! All three are safe Rust and deterministic: results are returned in a stable
//! order where it matters (see each type's notes).

pub mod bvh;
pub mod grid;
pub mod hash;

pub use bvh::{Bvh, BvhRayHit};
pub use grid::UniformGrid;
pub use hash::SpatialHash;

/// A hit reported by a ray query against any of the structures above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// Distance along the (unit-length) ray.
    pub t: f32,
    /// Point in world space.
    pub point: Vec3,
    /// Surface normal at the hit, when the structure can supply one.
    pub normal: Vec3,
    /// True when the ray hit a back face.
    pub back_face: bool,
}

impl RayHit {
    /// Constructs a hit from a distance and a normal.
    #[inline]
    #[must_use]
    pub fn new(t: f32, point: Vec3, normal: Vec3) -> Self {
        Self { t, point, normal, back_face: false }
    }

    /// Marks the hit as a back face.
    #[inline]
    #[must_use]
    pub fn with_back_face(mut self, back_face: bool) -> Self {
        self.back_face = back_face;
        self
    }
}

use crate::math::Vec3;
