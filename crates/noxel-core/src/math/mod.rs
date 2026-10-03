//! Mathematics for Noxel.
//!
//! Everything is `f32` and `#[repr(C)]` so buffers can be handed to a GPU
//! backend without a conversion pass. See
//! `docs/adr/0001-coordinate-system.md` for the conventions, and
//! `docs/modules/noxel-core.md` for the guided tour.
//!
//! # Conventions at a glance
//!
//! | Concern | Choice |
//! |---|---|
//! | Handedness | right-handed |
//! | Up axis | `+Y` |
//! | Forward axis | `-Z` |
//! | Angles | radians, yaw about `+Y` |
//! | Matrix storage | column-major, `[f32; 16]`-compatible |
//! | Clip-space depth | `[0, 1]` (wgpu/D3D/Metal), not `[-1, 1]` |
//! | World scale | 1.0 unit = 1 metre |
//!
//! # Rotations
//!
//! For a top-down game nearly every rotation is a **yaw**. Use
//! [`Quat::from_rotation_y`] / [`Quat::to_yaw`] rather than building full Euler
//! angles: yaw-only rotations cannot gimbal-lock, and `Quat::yaw_delta` gives
//! the short-arc difference the camera and NPC steering need.

pub mod color;
pub mod mat;
pub mod noise;
pub mod quat;
pub mod scalar;
pub mod shapes;
pub mod vec;

pub use color::{Color, Color8, Palette, PaletteEntry};
pub use mat::{Mat3, Mat4};
pub use noise::{
    fbm_2d, fbm_simplex_2d, hash_signed_2d, hash01_2d, hash01_3d, perlin_2d, ridged_2d, simplex_2d,
    tileable_value_2d, value_2d, value_3d, warped_fbm_2d, worley_2d,
};
pub use quat::Quat;
pub use scalar::{
    EPSILON, RAY_EPSILON, TAU, angle_delta, approx_eq, clamp_safe, damp, damp_factor, div_floor,
    inv_lerp, lerp, lerp_clamped, linear_to_srgb, rem_euclid_i32, rotate_towards, smootherstep,
    smoothstep, srgb_to_linear, to_degrees, to_radians, tonemap_aces, tonemap_reinhard, wrap_angle,
};
pub use shapes::{Aabb, Frustum, Plane, Ray, Rect, Sphere, Transform};
pub use vec::{Vec2, Vec3, Vec4};

/// A bare 2D point in **grid** space (tile centres), used by the world
/// generator and the path-finding grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub struct GridPos {
    /// Column (world X divided by the tile size).
    pub x: i32,
    /// Row (world Z divided by the tile size).
    pub y: i32,
}

impl GridPos {
    /// The origin cell.
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Constructs a grid position.
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Neighbour offset by `(dx, dy)`.
    #[inline]
    #[must_use]
    pub const fn offset(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
        }
    }

    /// Chebyshev distance (the number of steps a king makes).
    #[inline]
    #[must_use]
    pub const fn chebyshev_distance(self, other: Self) -> i32 {
        let dx = (self.x - other.x).abs();
        let dy = (self.y - other.y).abs();
        if dx > dy { dx } else { dy }
    }

    /// Manhattan distance.
    #[inline]
    #[must_use]
    pub const fn manhattan_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    /// Tiles whose centre lies inside `radius` (Euclidean, inclusive).
    ///
    /// Used for area-of-effect queries, light culling and the "wake nearby NPCs"
    /// broadphase.
    #[must_use]
    pub fn in_radius(self, radius: i32) -> Vec<GridPos> {
        let mut out = Vec::new();
        let r2 = radius * radius;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= r2 {
                    out.push(self.offset(dx, dy));
                }
            }
        }
        out
    }

    /// Converts a grid cell to a world position (cell centre) on the ground
    /// plane. `origin` is the world position of cell `(0, 0)`'s minimum corner.
    #[inline]
    #[must_use]
    pub fn to_world(self, tile_size: f32, origin: Vec3, y: f32) -> Vec3 {
        Vec3::new(
            origin.x + (self.x as f32 + 0.5) * tile_size,
            y,
            origin.z + (self.y as f32 + 0.5) * tile_size,
        )
    }

    /// Converts a world position to the grid cell containing it.
    #[inline]
    #[must_use]
    pub fn from_world(p: Vec3, tile_size: f32, origin: Vec3) -> Self {
        Self::new(
            ((p.x - origin.x) / tile_size).floor() as i32,
            ((p.z - origin.z) / tile_size).floor() as i32,
        )
    }
}

/// A chunk coordinate: the index of a fixed-size block of the world.
///
/// Worlds are streamed chunk by chunk; this is the key type for every chunk map,
/// cache and streaming decision in `noxel-world`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub struct ChunkPos {
    /// Chunk column index along X.
    pub x: i32,
    /// Chunk row index along Z.
    pub y: i32,
}

impl ChunkPos {
    /// The origin chunk.
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Constructs a chunk coordinate.
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// The chunk containing a world position.
    #[inline]
    #[must_use]
    pub fn from_world(p: Vec3, chunk_world_size: f32) -> Self {
        Self::new(
            div_floor_f((p.x / chunk_world_size).floor()),
            div_floor_f((p.z / chunk_world_size).floor()),
        )
    }

    /// The grid position of this chunk's minimum corner, in tiles.
    #[inline]
    #[must_use]
    pub const fn to_grid_origin(self, tiles_per_chunk: i32) -> GridPos {
        GridPos::new(self.x * tiles_per_chunk, self.y * tiles_per_chunk)
    }

    /// The chunk containing a tile.
    #[inline]
    #[must_use]
    pub const fn from_grid(g: GridPos, tiles_per_chunk: i32) -> Self {
        Self::new(
            div_floor(g.x, tiles_per_chunk),
            div_floor(g.y, tiles_per_chunk),
        )
    }

    /// The world-space centre of this chunk.
    #[inline]
    #[must_use]
    pub fn to_world_center(self, chunk_world_size: f32, y: f32) -> Vec3 {
        Vec3::new(
            (self.x as f32 + 0.5) * chunk_world_size,
            y,
            (self.y as f32 + 0.5) * chunk_world_size,
        )
    }

    /// The axis-aligned world bounds of this chunk, spanning `height` metres.
    #[inline]
    #[must_use]
    pub fn to_aabb(self, chunk_world_size: f32, y_min: f32, y_max: f32) -> Aabb {
        Aabb::new(
            Vec3::new(
                self.x as f32 * chunk_world_size,
                y_min,
                self.y as f32 * chunk_world_size,
            ),
            Vec3::new(
                (self.x + 1) as f32 * chunk_world_size,
                y_max,
                (self.y + 1) as f32 * chunk_world_size,
            ),
        )
    }

    /// Chebyshev distance in chunks, the metric the streaming radius uses.
    #[inline]
    #[must_use]
    pub const fn chebyshev_distance(self, other: Self) -> i32 {
        let dx = (self.x - other.x).abs();
        let dy = (self.y - other.y).abs();
        if dx > dy { dx } else { dy }
    }

    /// Every chunk within `radius` (Chebyshev), nearest first is **not**
    /// guaranteed; the streaming code sorts explicitly.
    #[must_use]
    pub fn in_radius(self, radius: i32) -> Vec<ChunkPos> {
        let mut out = Vec::with_capacity(((radius * 2 + 1) * (radius * 2 + 1)) as usize);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                out.push(Self::new(self.x + dx, self.y + dy));
            }
        }
        out
    }
}

#[inline]
fn div_floor_f(v: f32) -> i32 {
    // `as` saturates, so an out-of-range coordinate degrades to a far-away
    // chunk rather than undefined behaviour.
    v.floor() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_radius_is_circular() {
        let cells = GridPos::new(0, 0).in_radius(2);
        assert!(cells.contains(&GridPos::new(2, 0)));
        assert!(cells.contains(&GridPos::new(1, 1)));
        assert!(
            !cells.contains(&GridPos::new(2, 2)),
            "corner must be excluded"
        );
    }

    #[test]
    fn grid_world_roundtrip() {
        let origin = Vec3::new(-50.0, 0.0, -50.0);
        let g = GridPos::new(12, -7);
        let w = g.to_world(1.0, origin, 0.0);
        assert_eq!(GridPos::from_world(w, 1.0, origin), g);
    }

    #[test]
    fn chunk_from_world_handles_negatives() {
        // -0.5 is inside chunk -1, not chunk 0: floor, not truncation.
        assert_eq!(
            ChunkPos::from_world(Vec3::new(-0.5, 0.0, -0.5), 32.0),
            ChunkPos::new(-1, -1)
        );
        assert_eq!(
            ChunkPos::from_world(Vec3::new(0.5, 0.0, 0.5), 32.0),
            ChunkPos::new(0, 0)
        );
    }

    #[test]
    fn chunk_grid_conversion() {
        let g = GridPos::new(-1, 33);
        let c = ChunkPos::from_grid(g, 32);
        assert_eq!(c, ChunkPos::new(-1, 1));
        assert_eq!(c.to_grid_origin(32), GridPos::new(-32, 32));
    }

    #[test]
    fn chunk_aabb_matches_world_centre() {
        let c = ChunkPos::new(1, -2);
        let a = c.to_aabb(32.0, 0.0, 8.0);
        let center = c.to_world_center(32.0, 4.0);
        assert!(a.contains_point(center));
    }
}
