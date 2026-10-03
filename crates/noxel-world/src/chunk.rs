//! Chunks: the unit of generation, streaming and rendering.
//!
//! A [`Chunk`] is a dense, flat snapshot of a `tiles × tiles` square of world:
//! tile ids, terrain heights, slopes, the static colliders the physics layer
//! needs, the props and buildings that stand on it, and the road centrelines
//! crossing it. Nothing in here is lazy: once a chunk exists, every query is an
//! array index.
//!
//! ```
//! use noxel_world::chunk::{Chunk, ChunkPos};
//!
//! let chunk = Chunk::empty(ChunkPos::new(0, 0), 32, 1.0, noxel_world::BiomeId::PLAINS);
//! assert_eq!(chunk.tiles(), 32);
//! assert_eq!(chunk.tile(1, 2), 0);
//! assert_eq!(chunk.index(32, 0), None);
//! assert!(chunk.bounds().is_finite());
//! ```

use noxel_asset::format::{TileDef, TileSet};
use noxel_core::math::{Aabb, Vec3};

use crate::biome::BiomeId;
use crate::town::BuildingInstance as TownBuilding;

pub use noxel_core::math::ChunkPos;

/// World-space size of one tile, used when a chunk has to answer a geometric
/// query without access to the generator that produced it.
pub const DEFAULT_CHUNK_TILE_SIZE: f32 = 1.0;

/// A generated chunk.
///
/// The arrays are all `tiles * tiles` long and row-major: index `y * tiles + x`,
/// which is the order the renderer uploads and the order the golden-world text
/// dump prints.
#[derive(Clone, Debug)]
pub struct Chunk {
    /// Where this chunk sits in the world.
    pub pos: ChunkPos,
    /// Tile ids, row-major `y * tiles + x`.
    pub tiles: Vec<u32>,
    /// Terrain height in metres per tile (the top surface).
    pub heights: Vec<f32>,
    /// Slope magnitude per tile, for tile selection and for path-finding cost.
    pub slopes: Vec<f32>,
    /// The biome at the chunk's centre.
    pub biome: BiomeId,
    /// Static collision boxes in world space, as `(bounds, user_data)`.
    /// The physics crate consumes this; do not depend on it here.
    pub colliders: Vec<(Aabb, u64)>,
    /// Decorative props placed from the prefab library.
    pub props: Vec<PropInstance>,
    /// Buildings that intersect this chunk.
    pub buildings: Vec<TownBuilding>,
    /// Road centre lines that intersect this chunk, in world space.
    ///
    /// Each entry is the piece of a road that crosses this chunk, clipped to the
    /// chunk footprint (plus half the paved width), so the pieces tile the
    /// network exactly and two neighbouring chunks never report the same
    /// pavement twice.
    pub roads: Vec<crate::road::RoadSegment>,
    /// True when this chunk contains any part of a town.
    pub has_town: bool,
    /// Wall-clock generation time in milliseconds (0 when read from cache).
    pub generated_ms: f32,
    /// World size of one tile in metres, copied from the generator's config.
    ///
    /// The contract's field list does not include it; it is additive and exists
    /// so that [`Chunk::bounds`], [`Chunk::tile_world_center`] and
    /// [`Chunk::world_to_tile`] work without borrowing the generator. A chunk
    /// built by [`Chunk::empty`] or by the generator always sets it.
    pub tile_size: f32,
}

impl Chunk {
    /// An all-zero chunk of the given shape.
    ///
    /// Used by tests and by the tools that want a placeholder before the real
    /// chunk arrives. `tile_size` is sanitised to a positive value.
    #[must_use]
    pub fn empty(pos: ChunkPos, tiles: u32, tile_size: f32, biome: BiomeId) -> Self {
        let count = (tiles as usize).saturating_mul(tiles as usize);
        Self {
            pos,
            tiles: vec![0; count],
            heights: vec![0.0; count],
            slopes: vec![0.0; count],
            biome,
            colliders: Vec::new(),
            props: Vec::new(),
            buildings: Vec::new(),
            roads: Vec::new(),
            has_town: false,
            generated_ms: 0.0,
            tile_size: sanitize_tile_size(tile_size),
        }
    }

    /// Tiles per side.
    ///
    /// Returns `0` for a chunk whose tile array is not a perfect square (which
    /// only happens if a caller mutated the public `tiles` vector by hand).
    #[must_use]
    pub fn tiles(&self) -> u32 {
        let len = self.tiles.len();
        if len == 0 {
            return 0;
        }
        let side = (len as f64).sqrt().round() as usize;
        if side.saturating_mul(side) == len {
            side as u32
        } else {
            0
        }
    }

    /// Row-major index of a tile, or `None` when it is outside the chunk.
    #[must_use]
    pub fn index(&self, x: u32, y: u32) -> Option<usize> {
        let side = self.tiles();
        if side == 0 || x >= side || y >= side {
            return None;
        }
        Some(y as usize * side as usize + x as usize)
    }

    /// The tile id at `(x, y)`, or `0` when out of range.
    ///
    /// Tile id `0` is the tile set's first tile by convention; an out-of-range
    /// read must not be able to panic, and returning the "empty" id keeps
    /// `tile()` usable in the renderer's inner loop.
    #[must_use]
    pub fn tile(&self, x: u32, y: u32) -> u32 {
        match self.index(x, y) {
            Some(i) => self.tiles[i],
            None => 0,
        }
    }

    /// Terrain height in metres at `(x, y)`; `0.0` when out of range.
    #[must_use]
    pub fn height(&self, x: u32, y: u32) -> f32 {
        match self.index(x, y) {
            Some(i) => self.heights[i],
            None => 0.0,
        }
    }

    /// Slope magnitude (rise over run) at `(x, y)`; `0.0` when out of range.
    #[must_use]
    pub fn slope(&self, x: u32, y: u32) -> f32 {
        match self.index(x, y) {
            Some(i) => self.slopes[i],
            None => 0.0,
        }
    }

    /// The tile definition behind `(x, y)`, if the tile set has it.
    #[must_use]
    pub fn tile_def<'a>(&self, x: u32, y: u32, set: &'a TileSet) -> Option<&'a TileDef> {
        self.index(x, y)?;
        set.tile(self.tile(x, y))
    }

    /// True when an actor may stand on `(x, y)`.
    ///
    /// Out-of-range tiles are not walkable: an actor must not walk off the edge
    /// of a loaded chunk into nothing.
    #[must_use]
    pub fn is_walkable(&self, x: u32, y: u32, set: &TileSet) -> bool {
        self.tile_def(x, y, set)
            .is_some_and(|tile| tile.flags.walkable)
    }

    /// True when `(x, y)` blocks line of sight.
    ///
    /// Out-of-range tiles block: the visibility system must not see through the
    /// edge of the streamed world.
    #[must_use]
    pub fn blocks_sight(&self, x: u32, y: u32, set: &TileSet) -> bool {
        match self.tile_def(x, y, set) {
            Some(tile) => tile.flags.blocks_sight,
            None => true,
        }
    }

    /// World bounds of the chunk, including the height of everything on it.
    ///
    /// The XZ extent is the tile footprint; the Y extent covers the terrain
    /// range plus the tallest collider standing on the chunk, so a caller can
    /// cull the chunk without walking its buildings.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let side = self.tiles();
        let size = side as f32 * self.tile_size;
        let x0 = self.pos.x as f32 * size;
        let z0 = self.pos.y as f32 * size;
        if side == 0 {
            return Aabb::new(Vec3::new(x0, 0.0, z0), Vec3::new(x0, 0.0, z0));
        }
        let mut y_min = f32::INFINITY;
        let mut y_max = f32::NEG_INFINITY;
        for &h in &self.heights {
            if h.is_finite() {
                y_min = y_min.min(h);
                y_max = y_max.max(h);
            }
        }
        if !y_min.is_finite() || !y_max.is_finite() {
            y_min = 0.0;
            y_max = 0.0;
        }
        for (bounds, _) in &self.colliders {
            y_min = y_min.min(bounds.min.y);
            y_max = y_max.max(bounds.max.y);
        }
        Aabb::new(
            Vec3::new(x0, y_min, z0),
            Vec3::new(x0 + size, y_max, z0 + size),
        )
    }

    /// World position of the centre of tile `(x, y)`.
    ///
    /// Height is the tile's terrain height, so the result is on the ground. The
    /// tile size is sanitised, so a hand-built chunk with `tile_size = 0` still
    /// returns finite coordinates.
    #[must_use]
    pub fn tile_world_center(&self, x: u32, y: u32) -> Vec3 {
        let tile_size = sanitize_tile_size(self.tile_size);
        let origin_x = self.pos.x as f32 * self.tiles() as f32 * tile_size;
        let origin_z = self.pos.y as f32 * self.tiles() as f32 * tile_size;
        Vec3::new(
            origin_x + (x as f32 + 0.5) * tile_size,
            self.height(x, y),
            origin_z + (y as f32 + 0.5) * tile_size,
        )
    }

    /// The tile containing a world position, or `None` when it is not in this
    /// chunk.
    #[must_use]
    pub fn world_to_tile(&self, world: Vec3) -> Option<(u32, u32)> {
        let side = self.tiles();
        if side == 0 || !world.is_finite() {
            return None;
        }
        let tile_size = sanitize_tile_size(self.tile_size);
        let size = side as f32 * tile_size;
        let local_x = world.x - self.pos.x as f32 * size;
        let local_z = world.z - self.pos.y as f32 * size;
        if local_x < 0.0 || local_z < 0.0 || local_x >= size || local_z >= size {
            return None;
        }
        let tx = (local_x / tile_size).floor() as u32;
        let tz = (local_z / tile_size).floor() as u32;
        if tx >= side || tz >= side {
            return None;
        }
        Some((tx, tz))
    }

    /// The occluder volumes on this chunk: building runs plus props flagged as
    /// occluders, each paired with a stable id.
    ///
    /// Buildings contribute a handful of merged boxes rather than one per voxel;
    /// the camera occlusion system ray-tests these every frame, so the merging
    /// is what keeps it affordable.
    pub fn occluder_boxes(&self) -> impl Iterator<Item = (Aabb, u64)> + '_ {
        let buildings = self.buildings.iter().flat_map(|building| {
            building
                .occluders
                .iter()
                .copied()
                .map(|b| (b, building.id()))
        });
        let props = self
            .props
            .iter()
            .filter_map(|prop| prop.occluder_bounds().map(|b| (b, prop.id())));
        buildings.chain(props)
    }

    /// Approximate heap footprint in bytes, including the strings.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        use core::mem::size_of;
        let vecs = self.tiles.capacity() * size_of::<u32>()
            + self.heights.capacity() * size_of::<f32>()
            + self.slopes.capacity() * size_of::<f32>()
            + self.colliders.capacity() * size_of::<(Aabb, u64)>()
            + self.props.capacity() * size_of::<PropInstance>()
            + self.buildings.capacity() * size_of::<TownBuilding>()
            + self.roads.capacity() * size_of::<crate::road::RoadSegment>();
        let strings: usize = self
            .props
            .iter()
            .map(|p| p.name.capacity())
            .chain(self.buildings.iter().map(|b| b.prefab.capacity()))
            .sum();
        let occluders: usize = self
            .buildings
            .iter()
            .map(|b| b.occluders.capacity() * size_of::<Aabb>())
            .sum();
        size_of::<Self>() + vecs + strings + occluders
    }
}

/// Writes the first point of a lookup key that is stable across chunk
/// regeneration: a hash of the tile position mixed with a kind tag.
#[must_use]
pub(crate) fn stable_id(seed: u64, x: f32, z: f32, tag: u64) -> u64 {
    let qx = (x * 16.0).round() as i64 as u32;
    let qz = (z * 16.0).round() as i64 as u32;
    let mut h = noxel_core::rng::hash_2d(qx as i32, qz as i32, seed);
    h = noxel_core::rng::hash_combine(h, tag);
    h
}

/// The rough shape of a prop, derived from its name.
///
/// The generator names every prop it places, and both the physics layer and the
/// occlusion system need its *volume*, not just its position. Deriving the shape
/// from the name keeps that information in one place and out of the chunk's
/// memory budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    /// A tree: thin trunk (solid) plus a wide canopy (occluder).
    Tree,
    /// A boulder: solid and low, never an occluder.
    Rock,
    /// Low shrubbery: solid, small.
    Bush,
    /// A fence post, lamp post or sign: thin and solid.
    Post,
    /// Anything unrecognised: a modest solid box.
    Other,
}

impl PropKind {
    /// Classifies a prop by name, case-insensitively and without allocating.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        if contains_ci(name, "tree") || contains_ci(name, "pine") || contains_ci(name, "oak") {
            Self::Tree
        } else if contains_ci(name, "rock")
            || contains_ci(name, "boulder")
            || contains_ci(name, "stone")
        {
            Self::Rock
        } else if contains_ci(name, "bush") || contains_ci(name, "shrub") {
            Self::Bush
        } else if contains_ci(name, "post")
            || contains_ci(name, "fence")
            || contains_ci(name, "lamp")
            || contains_ci(name, "sign")
        {
            Self::Post
        } else {
            Self::Other
        }
    }

    /// True when the prop contributes a collider.
    #[must_use]
    pub fn is_solid(self) -> bool {
        matches!(
            self,
            Self::Tree | Self::Rock | Self::Bush | Self::Post | Self::Other
        )
    }

    /// True when the prop hides whatever is behind it.
    #[must_use]
    pub fn is_occluder(self) -> bool {
        matches!(self, Self::Tree)
    }

    /// `(half width, height)` of the prop's solid part, in metres before scale.
    #[must_use]
    pub fn collider_size(self) -> (f32, f32) {
        match self {
            Self::Tree => (0.28, 1.8),
            Self::Rock => (0.55, 0.8),
            Self::Bush => (0.45, 0.6),
            Self::Post => (0.14, 1.2),
            Self::Other => (0.35, 1.0),
        }
    }

    /// `(half width, base offset, height)` of the prop's occluder volume, in
    /// metres before scale.
    #[must_use]
    pub fn occluder_size(self) -> (f32, f32, f32) {
        match self {
            Self::Tree => (1.5, 1.5, 3.6),
            Self::Rock => (0.6, 0.0, 0.8),
            Self::Bush => (0.5, 0.0, 0.6),
            Self::Post => (0.2, 0.0, 1.2),
            Self::Other => (0.4, 0.0, 1.0),
        }
    }
}

/// Case-insensitive substring test that does not allocate.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    let hay = haystack.as_bytes();
    let pin = needle.as_bytes();
    if pin.is_empty() {
        return true;
    }
    if hay.len() < pin.len() {
        return false;
    }
    hay.windows(pin.len())
        .any(|window| window.eq_ignore_ascii_case(pin))
}

/// One placed prop (tree, rock, fence post...).
#[derive(Clone, Debug)]
pub struct PropInstance {
    /// The prop's name: a prefab name, or one of the generator's built-in kinds
    /// (`"tree"`, `"rock"`, `"bush"`, `"post"`). [`PropKind::from_name`] maps it
    /// to a shape.
    pub name: String,
    /// World position of the prop's base.
    pub position: Vec3,
    /// Rotation about the up axis, in radians.
    pub yaw: f32,
    /// Uniform scale multiplier.
    pub scale: f32,
    /// True when this prop should hide the player when the camera is behind it.
    pub occluder: bool,
}

impl PropInstance {
    /// Creates a prop with a full name and explicit transform.
    #[must_use]
    pub fn new(name: impl Into<String>, position: Vec3, yaw: f32, scale: f32) -> Self {
        let name = name.into();
        let occluder = PropKind::from_name(&name).is_occluder();
        Self {
            name,
            position,
            yaw,
            scale: sanitize_scale(scale),
            occluder,
        }
    }

    /// The prop's shape class.
    #[must_use]
    pub fn kind(&self) -> PropKind {
        PropKind::from_name(&self.name)
    }

    /// A stable id for this prop, used as collider `user_data`.
    #[must_use]
    pub fn id(&self) -> u64 {
        let tag = noxel_core::rng::hash_str(&self.name) ^ 0x7000_0000;
        stable_id(0, self.position.x, self.position.z, tag)
    }

    /// The prop's solid volume in world space, or `None` for decoration.
    #[must_use]
    pub fn collider_bounds(&self) -> Option<Aabb> {
        let kind = self.kind();
        if !kind.is_solid() {
            return None;
        }
        let (half, height) = kind.collider_size();
        let half = half * self.scale;
        let height = height * self.scale;
        Some(Aabb::new(
            Vec3::new(
                self.position.x - half,
                self.position.y - 0.1,
                self.position.z - half,
            ),
            Vec3::new(
                self.position.x + half,
                self.position.y + height,
                self.position.z + half,
            ),
        ))
    }

    /// The prop's occluder volume in world space, or `None` when it never hides
    /// anything.
    #[must_use]
    pub fn occluder_bounds(&self) -> Option<Aabb> {
        let kind = self.kind();
        if !self.occluder || !kind.is_occluder() {
            return None;
        }
        let (half, base, height) = kind.occluder_size();
        let half = half * self.scale;
        let base = self.position.y + base * self.scale;
        Some(Aabb::new(
            Vec3::new(self.position.x - half, base, self.position.z - half),
            Vec3::new(
                self.position.x + half,
                base + height * self.scale,
                self.position.z + half,
            ),
        ))
    }
}

/// Keeps a caller-supplied scale usable: finite and strictly positive.
#[must_use]
pub(crate) fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Keeps a caller-supplied tile size usable: finite and strictly positive.
#[must_use]
pub(crate) fn sanitize_tile_size(tile_size: f32) -> f32 {
    if tile_size.is_finite() && tile_size > 0.0 {
        tile_size
    } else {
        DEFAULT_CHUNK_TILE_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_asset::format::TileFlags;

    fn set() -> TileSet {
        TileSet {
            name: "test".into(),
            texture: "t.png".into(),
            tile_size: 16,
            tiles: vec![
                TileDef {
                    id: 0,
                    name: "void".into(),
                    texture: String::new(),
                    uv: [0, 0, 16, 16],
                    flags: TileFlags {
                        walkable: false,
                        blocks_sight: true,
                        ..TileFlags::default()
                    },
                    height: 0.0,
                    layer: 0,
                },
                TileDef {
                    id: 7,
                    name: "grass".into(),
                    texture: String::new(),
                    uv: [16, 0, 16, 16],
                    flags: TileFlags::default(),
                    height: 0.1,
                    layer: 0,
                },
            ],
        }
    }

    fn chunk() -> Chunk {
        let mut c = Chunk::empty(ChunkPos::new(1, -2), 4, 1.0, BiomeId::FOREST);
        c.tiles[0] = 7;
        c.tiles[5] = 7;
        c.heights[0] = 3.0;
        c.slopes[0] = 0.5;
        c
    }

    #[test]
    fn tiles_reports_the_side_length() {
        assert_eq!(chunk().tiles(), 4);
        assert_eq!(
            Chunk::empty(ChunkPos::ZERO, 32, 1.0, BiomeId::PLAINS).tiles(),
            32
        );
    }

    #[test]
    fn non_square_tile_array_reports_zero() {
        let mut c = chunk();
        c.tiles.push(1);
        assert_eq!(c.tiles(), 0);
        assert_eq!(c.index(0, 0), None);
        assert_eq!(c.tile(0, 0), 0);
        assert!(c.bounds().is_finite());
    }

    #[test]
    fn index_is_row_major() {
        let c = chunk();
        assert_eq!(c.index(0, 0), Some(0));
        assert_eq!(c.index(1, 0), Some(1));
        assert_eq!(c.index(0, 1), Some(4));
        assert_eq!(c.index(3, 3), Some(15));
    }

    #[test]
    fn out_of_range_reads_are_safe() {
        let c = chunk();
        assert_eq!(c.index(4, 0), None);
        assert_eq!(c.index(0, 4), None);
        assert_eq!(c.index(u32::MAX, u32::MAX), None);
        assert_eq!(c.tile(99, 99), 0);
        assert_eq!(c.height(99, 0), 0.0);
        assert_eq!(c.slope(0, 99), 0.0);
        assert!(c.tile_def(99, 0, &set()).is_none());
        assert!(!c.is_walkable(99, 0, &set()));
        assert!(c.blocks_sight(99, 0, &set()));
    }

    #[test]
    fn tile_lookup_matches_the_set() {
        let c = chunk();
        let set = set();
        assert_eq!(
            c.tile_def(0, 0, &set).map(|t| t.name.as_str()),
            Some("grass")
        );
        assert!(c.is_walkable(0, 0, &set));
        assert!(!c.blocks_sight(0, 0, &set));
        // Tile 0 is "void": solid and sight-blocking.
        assert!(!c.is_walkable(1, 0, &set));
        assert!(c.blocks_sight(1, 0, &set));
    }

    #[test]
    fn unknown_tile_id_has_no_definition() {
        let mut c = chunk();
        c.tiles[1] = 999;
        assert!(c.tile_def(1, 0, &set()).is_none());
        assert!(!c.is_walkable(1, 0, &set()));
    }

    #[test]
    fn bounds_cover_the_footprint_and_the_height_range() {
        let c = chunk();
        let b = c.bounds();
        assert!(b.is_finite());
        // Chunk (1, -2) with 4 tiles of 1 m starts at x = 4, z = -8.
        assert_eq!(b.min.x, 4.0);
        assert_eq!(b.max.x, 8.0);
        assert_eq!(b.min.z, -8.0);
        assert_eq!(b.max.z, -4.0);
        assert!(b.min.y <= 3.0 && b.max.y >= 3.0);
    }

    #[test]
    fn tile_world_center_is_inside_the_chunk() {
        let c = chunk();
        let p = c.tile_world_center(0, 0);
        assert_eq!(p.x, 4.5);
        assert_eq!(p.z, -7.5);
        assert_eq!(p.y, 3.0, "height comes from the height array");
        assert!(c.bounds().contains_point(p));
    }

    #[test]
    fn world_to_tile_round_trips() {
        let c = chunk();
        for y in 0..c.tiles() {
            for x in 0..c.tiles() {
                let p = c.tile_world_center(x, y);
                assert_eq!(c.world_to_tile(p), Some((x, y)), "tile {x},{y}");
            }
        }
    }

    #[test]
    fn world_to_tile_rejects_outside_points() {
        let c = chunk();
        assert_eq!(c.world_to_tile(Vec3::new(0.0, 0.0, 0.0)), None);
        assert_eq!(c.world_to_tile(Vec3::new(1e9, 0.0, 0.0)), None);
        assert_eq!(c.world_to_tile(Vec3::new(f32::NAN, 0.0, 0.0)), None);
    }

    #[test]
    fn empty_chunk_answers_everything_without_panicking() {
        let c = Chunk::empty(ChunkPos::ZERO, 0, 1.0, BiomeId::LAKE);
        assert_eq!(c.tiles(), 0);
        assert_eq!(c.tile(0, 0), 0);
        assert_eq!(c.world_to_tile(Vec3::ZERO), None);
        assert!(c.bounds().is_finite());
        assert_eq!(c.tile_world_center(0, 0), Vec3::new(0.5, 0.0, 0.5));
    }

    #[test]
    fn zero_tile_size_is_sanitised() {
        let mut c = chunk();
        c.tile_size = 0.0;
        assert!(c.tile_world_center(1, 1).is_finite());
        assert!(c.bounds().is_finite());
        c.tile_size = f32::NAN;
        assert!(c.tile_world_center(1, 1).is_finite());
    }

    #[test]
    fn memory_bytes_counts_the_arrays() {
        let small = Chunk::empty(ChunkPos::ZERO, 4, 1.0, BiomeId::PLAINS);
        let big = Chunk::empty(ChunkPos::ZERO, 32, 1.0, BiomeId::PLAINS);
        assert!(small.memory_bytes() > 0);
        assert!(
            big.memory_bytes() > small.memory_bytes(),
            "{} vs {}",
            big.memory_bytes(),
            small.memory_bytes()
        );
    }

    #[test]
    fn prop_kinds_come_from_names() {
        assert_eq!(PropKind::from_name("tree_pine"), PropKind::Tree);
        assert_eq!(PropKind::from_name("PINE"), PropKind::Tree);
        assert_eq!(PropKind::from_name("boulder_large"), PropKind::Rock);
        assert_eq!(PropKind::from_name("berry_bush"), PropKind::Bush);
        assert_eq!(PropKind::from_name("fence_post"), PropKind::Post);
        assert_eq!(PropKind::from_name("crate"), PropKind::Other);
    }

    #[test]
    fn tree_props_collide_and_occlude() {
        let tree = PropInstance::new("tree", Vec3::new(1.0, 2.0, 3.0), 0.0, 1.0);
        let solid = tree.collider_bounds().unwrap();
        assert!(solid.is_finite());
        assert!(solid.max.y - solid.min.y > 1.0);
        let occ = tree.occluder_bounds().unwrap();
        assert!(occ.min.y > solid.min.y, "canopy sits above the trunk");
        assert!(
            occ.size().x > solid.size().x,
            "canopy is wider than the trunk"
        );
    }

    #[test]
    fn rocks_are_solid_but_not_occluders() {
        let rock = PropInstance::new("rock", Vec3::ZERO, 0.0, 1.0);
        assert!(rock.collider_bounds().is_some());
        assert!(rock.occluder_bounds().is_none());
    }

    #[test]
    fn prop_scale_is_sanitised() {
        let p = PropInstance::new("tree", Vec3::ZERO, 0.0, 0.0);
        assert_eq!(p.scale, 1.0);
        let p = PropInstance::new("tree", Vec3::ZERO, 0.0, f32::INFINITY);
        assert_eq!(p.scale, 1.0);
        let p = PropInstance::new("tree", Vec3::ZERO, 0.0, 2.0);
        let a = p.occluder_bounds().unwrap();
        let q = PropInstance::new("tree", Vec3::ZERO, 0.0, 1.0);
        let b = q.occluder_bounds().unwrap();
        assert!((a.size().x - b.size().x * 2.0).abs() < 1e-4);
    }

    #[test]
    fn prop_ids_are_stable_and_position_dependent() {
        let a = PropInstance::new("tree", Vec3::new(3.0, 0.0, 4.0), 0.0, 1.0);
        let b = PropInstance::new("tree", Vec3::new(3.0, 0.0, 4.0), 1.5, 0.9);
        let c = PropInstance::new("tree", Vec3::new(3.0, 0.0, 9.0), 0.0, 1.0);
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
    }

    #[test]
    fn occluder_boxes_merge_buildings_and_props() {
        use crate::town::Facing;
        let mut c = chunk();
        c.props.push(PropInstance::new(
            "tree",
            Vec3::new(4.5, 0.0, -7.5),
            0.0,
            1.0,
        ));
        c.props.push(PropInstance::new(
            "rock",
            Vec3::new(5.5, 0.0, -7.5),
            0.0,
            1.0,
        ));
        c.buildings.push(TownBuilding {
            prefab: "house".into(),
            origin: Vec3::new(6.0, 0.0, -7.0),
            yaw: 0.0,
            size_tiles: (2, 2),
            bounds: Aabb::new(Vec3::new(6.0, 0.0, -7.0), Vec3::new(8.0, 3.0, -5.0)),
            facing: Facing::South,
            occluders: vec![Aabb::new(
                Vec3::new(6.0, 0.0, -7.0),
                Vec3::new(8.0, 3.0, -5.0),
            )],
        });
        let boxes: Vec<_> = c.occluder_boxes().collect();
        assert_eq!(boxes.len(), 2, "one building run plus one tree canopy");
        assert!(boxes.iter().any(|(b, _)| b.max.y >= 3.0 - 1e-6));
    }

    #[test]
    fn contains_ci_is_case_insensitive() {
        assert!(contains_ci("TreePine", "tree"));
        assert!(contains_ci("pine", "PINE"));
        assert!(!contains_ci("roc", "rock"));
        assert!(!contains_ci("", "rock"));
        assert!(contains_ci("anything", ""));
    }
}
