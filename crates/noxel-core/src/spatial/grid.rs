//! A static uniform grid over the world's XZ plane.
//!
//! Built once for static content (terrain tiles, buildings, tree trunks, light
//! volumes) and queried all frame. The grid is axis-aligned in XZ because a
//! top-down world is naturally a 2D field of columns; an item's vertical extent
//! is kept in its own bounds, so the cell count stays proportional to *area*
//! rather than volume.
//!
//! # Storage layout
//!
//! Items live in one dense `Vec` and cells store `u32` indices into it. That
//! keeps a large building registered in every cell it covers without cloning
//! the value, and makes the whole structure `Clone` for any `T: Clone`.
//!
//! # Avoiding per-query allocation
//!
//! A query that spans more than one cell can encounter the same item more than
//! once. The `*_into` variants take a caller-owned scratch buffer so a hot loop
//! performs no allocation; the plain variants allocate a small one for
//! convenience.
//!
//! ```
//! use noxel_core::math::{Aabb, Vec3};
//! use noxel_core::spatial::UniformGrid;
//!
//! let grid = UniformGrid::build(
//!     Aabb::new(Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 20.0, 50.0)),
//!     4.0,
//!     [
//!         (Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 3.0, 2.0)), "house"),
//!         (Aabb::new(Vec3::new(30.0, 0.0, 30.0), Vec3::new(32.0, 3.0, 32.0)), "barn"),
//!     ],
//! );
//!
//! let mut hits = Vec::new();
//! grid.query_aabb(Aabb::new(Vec3::ZERO, Vec3::splat(1.0)), &mut hits);
//! assert_eq!(hits, vec![&"house"]);
//! ```

use crate::math::{Aabb, GridPos, Ray, Vec3};

/// One stored item and the bounds it occupies.
#[derive(Clone, Debug)]
pub struct GridEntry<T> {
    /// The item's world bounds.
    pub bounds: Aabb,
    /// The stored value.
    pub value: T,
}

/// A uniform grid of buckets over a bounded world region.
#[derive(Clone, Debug)]
pub struct UniformGrid<T> {
    origin: Vec3,
    cell_size: f32,
    inv_cell_size: f32,
    /// Number of cells along X and Z.
    dims: (i32, i32),
    /// Dense item storage; cell lists hold indices into this.
    items: Vec<GridEntry<T>>,
    /// Bucket lists, row-major `z * dims.0 + x`.
    cells: Vec<Vec<u32>>,
    /// Items too large to register per-cell; every query tests them directly.
    overflow: Vec<u32>,
    /// Cap on how many cells one item may register in.
    max_cells_per_item: usize,
}

impl<T> UniformGrid<T> {
    /// The default cap on how many cells one item may register in.
    ///
    /// A single item covering 257+ cells is almost always a mistake (a
    /// mis-scaled bounds, or a "whole world" sentinel). Keeping it out of the
    /// bucket lists bounds the build cost and the query scan.
    pub const DEFAULT_MAX_CELLS_PER_ITEM: usize = 256;

    /// Builds a grid covering `bounds` with cells of `cell_size` world units.
    ///
    /// Items whose bounds extend beyond `bounds` are clamped into the grid's
    /// cell range; the item's true bounds are preserved, so a query still tests
    /// them exactly. Nothing is silently dropped.
    #[must_use]
    pub fn build(bounds: Aabb, cell_size: f32, items: impl IntoIterator<Item = (Aabb, T)>) -> Self {
        let mut grid = Self::empty(bounds, cell_size);
        for (b, v) in items {
            grid.insert(b, v);
        }
        grid
    }

    /// Builds an empty grid, for filling incrementally with [`UniformGrid::insert`].
    #[must_use]
    pub fn empty(bounds: Aabb, cell_size: f32) -> Self {
        let cell_size = cell_size.max(0.01);
        let size = bounds.size();
        let dims = (
            ((size.x / cell_size).ceil() as i32).max(1),
            ((size.z / cell_size).ceil() as i32).max(1),
        );
        let cell_count = (dims.0 as usize) * (dims.1 as usize);
        Self {
            origin: bounds.min,
            cell_size,
            inv_cell_size: 1.0 / cell_size,
            dims,
            items: Vec::new(),
            cells: vec![Vec::new(); cell_count],
            overflow: Vec::new(),
            max_cells_per_item: Self::DEFAULT_MAX_CELLS_PER_ITEM,
        }
    }

    /// Sets the cap on cells per item.
    pub fn set_max_cells_per_item(&mut self, n: usize) {
        self.max_cells_per_item = n.max(1);
    }

    /// Registers `value` in every cell its bounds overlap.
    ///
    /// Returns the item's index, which is stable for the lifetime of the grid.
    pub fn insert(&mut self, bounds: Aabb, value: T) -> u32 {
        let index = self.items.len() as u32;
        self.items.push(GridEntry { bounds, value });
        self.register(index);
        index
    }

    /// Re-registers a moved item so later queries see its new bounds.
    ///
    /// `UniformGrid` is a *static* structure; this exists for the rare case of a
    /// door or bridge that changes once, not for per-frame motion. Use
    /// [`crate::spatial::SpatialHash`] for that.
    pub fn update_bounds(&mut self, index: u32, bounds: Aabb) {
        let Some(entry) = self.items.get_mut(index as usize) else {
            return;
        };
        entry.bounds = bounds;
        self.unregister(index);
        self.register(index);
    }

    fn register(&mut self, index: u32) {
        let (x0, z0, x1, z1) = self.clamped_cell_range(&self.items[index as usize].bounds);
        let span = ((x1 - x0 + 1).max(1) as usize) * ((z1 - z0 + 1).max(1) as usize);
        if span > self.max_cells_per_item {
            self.overflow.push(index);
            return;
        }
        for z in z0..=z1 {
            for x in x0..=x1 {
                let ci = self.cell_index(x, z);
                self.cells[ci].push(index);
            }
        }
    }

    fn unregister(&mut self, index: u32) {
        if let Some(pos) = self.overflow.iter().position(|&i| i == index) {
            self.overflow.swap_remove(pos);
            return;
        }
        let (x0, z0, x1, z1) = self.clamped_cell_range(&self.items[index as usize].bounds);
        for z in z0..=z1 {
            for x in x0..=x1 {
                let ci = self.cell_index(x, z);
                if let Some(pos) = self.cells[ci].iter().position(|&i| i == index) {
                    self.cells[ci].swap_remove(pos);
                }
            }
        }
    }

    /// Shared access to an item by index.
    #[must_use]
    pub fn item(&self, index: u32) -> Option<&GridEntry<T>> {
        self.items.get(index as usize)
    }

    /// The number of registered items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of cells.
    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// The grid cell size in world units.
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// The world-space origin (minimum corner of cell `(0, 0)`).
    #[must_use]
    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    /// The world-space extent of the grid along X and Z.
    #[must_use]
    pub fn world_size(&self) -> (f32, f32) {
        (
            self.dims.0 as f32 * self.cell_size,
            self.dims.1 as f32 * self.cell_size,
        )
    }

    /// Items whose bounds overlap `query`, each reported exactly once.
    ///
    /// `scratch` is reused between calls; pass the same buffer every frame to
    /// avoid allocating.
    pub fn query_aabb_into<'a>(
        &'a self,
        query: Aabb,
        out: &mut Vec<&'a T>,
        scratch: &mut Vec<u32>,
    ) {
        out.clear();
        scratch.clear();
        let (x0, z0, x1, z1) = self.clamped_cell_range(&query);
        let multi = x1 > x0 || z1 > z0;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &idx in &self.cells[self.cell_index(x, z)] {
                    scratch.push(idx);
                }
            }
        }
        for &idx in &self.overflow {
            scratch.push(idx);
        }
        if multi {
            // Sorting + dedup gives a deterministic result order as a bonus:
            // the caller can rely on it for reproducible simulation.
            scratch.sort_unstable();
            scratch.dedup();
        }
        for &idx in scratch.iter() {
            let e = &self.items[idx as usize];
            if e.bounds.intersects(&query) {
                out.push(&e.value);
            }
        }
    }

    /// Convenience wrapper around [`UniformGrid::query_aabb_into`].
    pub fn query_aabb<'a>(&'a self, query: Aabb, out: &mut Vec<&'a T>) {
        let mut scratch = Vec::new();
        self.query_aabb_into(query, out, &mut scratch);
    }

    /// Items whose bounds contain `point`.
    pub fn query_point<'a>(&'a self, point: Vec3, out: &mut Vec<&'a T>) {
        self.query_aabb(Aabb::new(point, point), out);
    }

    /// Items whose bounds are within `radius` of `center` in the XZ plane.
    ///
    /// The vertical extent is unbounded on purpose: a top-down query asks "what
    /// is near me on the map", not "what is at my exact height".
    pub fn query_radius<'a>(&'a self, center: Vec3, radius: f32, out: &mut Vec<&'a T>) {
        let r = Vec3::new(radius, f32::MAX, radius);
        self.query_aabb(Aabb::new(center - r, center + r), out);
    }

    /// Calls `f` for every item whose bounds overlap `query`, exactly once each.
    pub fn for_each_in_aabb<'a>(
        &'a self,
        query: Aabb,
        scratch: &mut Vec<u32>,
        mut f: impl FnMut(&Aabb, &'a T),
    ) {
        scratch.clear();
        let (x0, z0, x1, z1) = self.clamped_cell_range(&query);
        let multi = x1 > x0 || z1 > z0;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &idx in &self.cells[self.cell_index(x, z)] {
                    scratch.push(idx);
                }
            }
        }
        for &idx in &self.overflow {
            scratch.push(idx);
        }
        if multi {
            scratch.sort_unstable();
            scratch.dedup();
        }
        for &idx in scratch.iter() {
            let e = &self.items[idx as usize];
            if e.bounds.intersects(&query) {
                f(&e.bounds, &e.value);
            }
        }
    }

    /// Items whose bounds a ray passes through, using a DDA walk.
    ///
    /// Cells are visited in the order the ray crosses them, so a caller looking
    /// for the first hit can stop early. The intersection distance is not
    /// computed here — test each candidate with [`Aabb::intersect_ray`].
    pub fn query_ray_into<'a>(&'a self, ray: &Ray, out: &mut Vec<&'a T>, scratch: &mut Vec<u32>) {
        out.clear();
        scratch.clear();
        let (dx, dz) = (ray.dir.x, ray.dir.z);
        if dx.abs() < 1e-9 && dz.abs() < 1e-9 {
            // Straight down: every cell under the origin.
            for &idx in &self.overflow {
                scratch.push(idx);
            }
            let p = ray.origin;
            let cell = self.cell_of(p);
            if cell.x >= 0 && cell.y >= 0 && cell.x < self.dims.0 && cell.y < self.dims.1 {
                scratch.extend_from_slice(&self.cells[self.cell_index(cell.x, cell.y)]);
            }
        } else {
            let ox = (ray.origin.x - self.origin.x) * self.inv_cell_size;
            let oz = (ray.origin.z - self.origin.z) * self.inv_cell_size;
            let mut cx = ox.floor() as i32;
            let mut cz = oz.floor() as i32;
            let step_x = if dx > 0.0 { 1 } else { -1 };
            let step_z = if dz > 0.0 { 1 } else { -1 };
            let mut t_max_x = if dx.abs() < 1e-9 {
                f32::INFINITY
            } else {
                ((if dx > 0.0 { cx + 1 } else { cx }) as f32 - ox) / (dx * self.inv_cell_size)
            };
            let mut t_max_z = if dz.abs() < 1e-9 {
                f32::INFINITY
            } else {
                ((if dz > 0.0 { cz + 1 } else { cz }) as f32 - oz) / (dz * self.inv_cell_size)
            };
            let t_delta_x = if dx.abs() < 1e-9 {
                f32::INFINITY
            } else {
                1.0 / (dx.abs() * self.inv_cell_size)
            };
            let t_delta_z = if dz.abs() < 1e-9 {
                f32::INFINITY
            } else {
                1.0 / (dz.abs() * self.inv_cell_size)
            };
            let limit = ray.max_t;

            // A straight line crosses at most dims.0 + dims.1 + 2 cells.
            let max_steps = (self.dims.0 + self.dims.1 + 4) as usize;
            for _ in 0..max_steps {
                if cx >= 0 && cz >= 0 && cx < self.dims.0 && cz < self.dims.1 {
                    let t_enter = t_max_x.min(t_max_z);
                    if t_enter > limit {
                        break;
                    }
                    scratch.extend_from_slice(&self.cells[self.cell_index(cx, cz)]);
                } else if (cx < 0 && step_x < 0)
                    || (cz < 0 && step_z < 0)
                    || (cx >= self.dims.0 && step_x > 0)
                    || (cz >= self.dims.1 && step_z > 0)
                {
                    break; // left the grid in the direction of travel
                }
                if t_max_x < t_max_z {
                    cx += step_x;
                    t_max_x += t_delta_x;
                } else {
                    cz += step_z;
                    t_max_z += t_delta_z;
                }
                if t_max_x.min(t_max_z) > limit {
                    break;
                }
            }
            scratch.extend_from_slice(&self.overflow);
        }
        scratch.sort_unstable();
        scratch.dedup();
        for &idx in scratch.iter() {
            let e = &self.items[idx as usize];
            if e.bounds.intersect_ray(ray.origin, ray.dir).is_some() {
                out.push(&e.value);
            }
        }
    }

    /// Convenience wrapper around [`UniformGrid::query_ray_into`].
    pub fn query_ray<'a>(&'a self, ray: &Ray, out: &mut Vec<&'a T>) {
        let mut scratch = Vec::new();
        self.query_ray_into(ray, out, &mut scratch);
    }

    /// Removes every item, keeping the cell allocation.
    pub fn clear(&mut self) {
        for c in &mut self.cells {
            c.clear();
        }
        self.items.clear();
        self.overflow.clear();
    }

    /// Every item, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &GridEntry<T>> + '_ {
        self.items.iter()
    }

    /// An estimate of heap usage in bytes, for the statistics panel.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let buckets: usize = self
            .cells
            .iter()
            .map(|c| c.capacity() * core::mem::size_of::<u32>())
            .sum();
        let cells_vec = self.cells.capacity() * core::mem::size_of::<Vec<u32>>();
        let items = self.items.capacity() * core::mem::size_of::<GridEntry<T>>();
        let overflow = self.overflow.capacity() * core::mem::size_of::<u32>();
        buckets + cells_vec + items + overflow
    }

    fn cell_index(&self, x: i32, z: i32) -> usize {
        (z as usize) * (self.dims.0 as usize) + (x as usize)
    }

    /// The inclusive cell range an AABB touches, clamped into the grid.
    fn clamped_cell_range(&self, b: &Aabb) -> (i32, i32, i32, i32) {
        let x0 = ((b.min.x - self.origin.x) * self.inv_cell_size).floor() as i32;
        let z0 = ((b.min.z - self.origin.z) * self.inv_cell_size).floor() as i32;
        let x1 = ((b.max.x - self.origin.x) * self.inv_cell_size).floor() as i32;
        let z1 = ((b.max.z - self.origin.z) * self.inv_cell_size).floor() as i32;
        (
            x0.clamp(0, self.dims.0 - 1),
            z0.clamp(0, self.dims.1 - 1),
            x1.clamp(0, self.dims.0 - 1),
            z1.clamp(0, self.dims.1 - 1),
        )
    }

    /// The grid cell containing a world position.
    #[must_use]
    pub fn cell_of(&self, point: Vec3) -> GridPos {
        GridPos::new(
            ((point.x - self.origin.x) * self.inv_cell_size).floor() as i32,
            ((point.z - self.origin.z) * self.inv_cell_size).floor() as i32,
        )
    }

    /// The world-space bounds of one cell (on the ground plane).
    #[must_use]
    pub fn cell_bounds(&self, cell: GridPos) -> Aabb {
        let x0 = self.origin.x + cell.x as f32 * self.cell_size;
        let z0 = self.origin.z + cell.y as f32 * self.cell_size;
        Aabb::new(
            Vec3::new(x0, self.origin.y, z0),
            Vec3::new(x0 + self.cell_size, self.origin.y, z0 + self.cell_size),
        )
    }

    /// Iterates every non-empty cell as `(cell_coord, item_indices)`.
    pub fn iter_cells(&self) -> impl Iterator<Item = (GridPos, &[u32])> + '_ {
        let dim_x = self.dims.0 as usize;
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_empty())
            .map(move |(i, c)| {
                (
                    GridPos::new((i % dim_x) as i32, (i / dim_x) as i32),
                    c.as_slice(),
                )
            })
    }

    /// The number of items that were diverted to the overflow list.
    #[must_use]
    pub fn overflow_count(&self) -> usize {
        self.overflow.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> UniformGrid<&'static str> {
        UniformGrid::build(
            Aabb::new(Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 20.0, 50.0)),
            4.0,
            [
                (
                    Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 3.0, 2.0)),
                    "house",
                ),
                (
                    Aabb::new(Vec3::new(30.0, 0.0, 30.0), Vec3::new(32.0, 3.0, 32.0)),
                    "barn",
                ),
                (
                    Aabb::new(Vec3::new(-40.0, 0.0, -40.0), Vec3::new(-38.0, 2.0, -38.0)),
                    "well",
                ),
            ],
        )
    }

    #[test]
    fn query_aabb_finds_overlapping_only() {
        let g = grid();
        let mut hits = Vec::new();
        g.query_aabb(Aabb::new(Vec3::ZERO, Vec3::splat(1.0)), &mut hits);
        assert_eq!(hits, vec![&"house"]);
    }

    #[test]
    fn query_does_not_duplicate_multi_cell_items() {
        let g = UniformGrid::build(
            Aabb::new(Vec3::ZERO, Vec3::new(100.0, 10.0, 100.0)),
            1.0,
            [(
                Aabb::new(Vec3::new(40.0, 0.0, 40.0), Vec3::new(60.0, 2.0, 60.0)),
                "big",
            )],
        );
        let mut hits = Vec::new();
        g.query_aabb(
            Aabb::new(Vec3::ZERO, Vec3::new(100.0, 5.0, 100.0)),
            &mut hits,
        );
        assert_eq!(hits, vec![&"big"], "a 20x20 item must be reported once");
    }

    #[test]
    fn query_point() {
        let g = grid();
        let mut hits = Vec::new();
        g.query_point(Vec3::new(1.0, 1.0, 1.0), &mut hits);
        assert_eq!(hits, vec![&"house"]);
        hits.clear();
        g.query_point(Vec3::new(1.0, 50.0, 1.0), &mut hits);
        assert!(hits.is_empty(), "above the box");
    }

    #[test]
    fn query_radius_uses_xz() {
        let g = grid();
        let mut hits = Vec::new();
        g.query_radius(Vec3::new(1.0, 0.0, 1.0), 5.0, &mut hits);
        assert_eq!(hits, vec![&"house"]);
    }

    #[test]
    fn query_reuse_does_not_leak_between_calls() {
        let g = grid();
        let mut hits = Vec::new();
        g.query_point(Vec3::new(1.0, 1.0, 1.0), &mut hits);
        assert_eq!(hits.len(), 1);
        g.query_point(Vec3::new(1000.0, 1.0, 1000.0), &mut hits);
        assert!(hits.is_empty(), "out must be cleared, not appended to");
    }

    #[test]
    fn query_ray_hits_the_right_items() {
        let g = grid();
        let mut hits = Vec::new();
        let ray = Ray::new(Vec3::new(-10.0, 1.0, 1.0), Vec3::X);
        g.query_ray(&ray, &mut hits);
        assert!(hits.contains(&&"house"), "{hits:?}");
        assert!(!hits.contains(&&"barn"));
    }

    #[test]
    fn query_ray_axis_aligned_downwards_falls_back() {
        let g = grid();
        let mut hits = Vec::new();
        let ray = Ray::new(Vec3::new(1.0, 20.0, 1.0), Vec3::new(0.0, -1.0, 0.0));
        g.query_ray(&ray, &mut hits);
        assert!(hits.contains(&&"house"));
    }

    #[test]
    fn query_ray_respects_max_t() {
        let g = grid();
        let mut hits = Vec::new();
        let ray = Ray::with_max_t(Vec3::new(-40.0, 1.0, 30.0), Vec3::X, 5.0);
        g.query_ray(&ray, &mut hits);
        assert!(
            hits.is_empty(),
            "ray stops before reaching anything: {hits:?}"
        );
    }

    #[test]
    fn oversized_items_go_to_overflow_and_are_found_once() {
        let mut g: UniformGrid<&'static str> =
            UniformGrid::empty(Aabb::new(Vec3::ZERO, Vec3::new(1000.0, 10.0, 1000.0)), 1.0);
        g.set_max_cells_per_item(4);
        g.insert(
            Aabb::new(Vec3::new(100.0, 0.0, 100.0), Vec3::new(900.0, 5.0, 900.0)),
            "huge",
        );
        assert_eq!(g.overflow_count(), 1);

        let mut hits = Vec::new();
        g.query_aabb(
            Aabb::new(Vec3::new(500.0, 0.0, 500.0), Vec3::new(501.0, 1.0, 501.0)),
            &mut hits,
        );
        assert_eq!(hits, vec![&"huge"]);

        let mut all = Vec::new();
        g.query_aabb(
            Aabb::new(Vec3::ZERO, Vec3::new(1000.0, 10.0, 1000.0)),
            &mut all,
        );
        assert_eq!(all.len(), 1, "must not be reported twice");
    }

    #[test]
    fn for_each_matches_query() {
        let g = grid();
        let q = Aabb::new(Vec3::ZERO, Vec3::splat(35.0));
        let mut via_query = Vec::new();
        g.query_aabb(q, &mut via_query);
        let mut via_iter = Vec::new();
        let mut scratch = Vec::new();
        g.for_each_in_aabb(q, &mut scratch, |_b, v| via_iter.push(v));
        assert_eq!(via_query.len(), via_iter.len());
        for v in &via_iter {
            assert!(via_query.contains(v));
        }
    }

    #[test]
    fn cell_helpers_agree() {
        let g = grid();
        let cell = g.cell_of(Vec3::new(0.0, 0.0, 0.0));
        let b = g.cell_bounds(cell);
        assert!(b.contains_point(Vec3::new(0.1, 0.0, 0.1)));
    }

    #[test]
    fn update_bounds_moves_the_item() {
        let mut g: UniformGrid<&'static str> =
            UniformGrid::empty(Aabb::new(Vec3::ZERO, Vec3::new(64.0, 8.0, 64.0)), 4.0);
        let idx = g.insert(Aabb::new(Vec3::ZERO, Vec3::splat(1.0)), "actor");
        g.update_bounds(idx, Aabb::new(Vec3::splat(40.0), Vec3::splat(41.0)));

        let mut hits = Vec::new();
        g.query_aabb(Aabb::new(Vec3::ZERO, Vec3::splat(2.0)), &mut hits);
        assert!(hits.is_empty(), "old cell must be vacated");
        hits.clear();
        g.query_aabb(Aabb::new(Vec3::splat(40.0), Vec3::splat(41.0)), &mut hits);
        assert_eq!(hits, vec![&"actor"]);
    }

    #[test]
    fn query_results_are_in_deterministic_order() {
        let g = grid();
        let q = Aabb::new(Vec3::ZERO, Vec3::splat(60.0));
        let mut first = Vec::new();
        g.query_aabb(q, &mut first);
        for _ in 0..8 {
            let mut again = Vec::new();
            g.query_aabb(q, &mut again);
            assert_eq!(first, again, "query order must be stable");
        }
    }

    #[test]
    fn iter_cells_lists_only_non_empty() {
        let g = grid();
        // Three items, but a 3-item footprint may straddle a cell boundary, so
        // assert on the invariant rather than an exact count.
        let cells: Vec<_> = g.iter_cells().collect();
        assert!(!cells.is_empty());
        assert!(cells.len() <= 9, "{}", cells.len());
        let total: usize = cells.iter().map(|(_, list)| list.len()).sum();
        assert!(total >= 3);
    }

    #[test]
    fn clear_empties_the_grid() {
        let mut g = grid();
        g.clear();
        assert!(g.is_empty());
        assert_eq!(g.iter_cells().count(), 0);
        let mut hits = Vec::new();
        g.query_aabb(Aabb::new(Vec3::ZERO, Vec3::splat(100.0)), &mut hits);
        assert!(hits.is_empty(), "cell lists must be cleared too");
    }

    #[test]
    fn memory_estimate_is_positive() {
        assert!(grid().memory_bytes() > 0);
    }

    #[test]
    fn item_lookup_by_index() {
        let g = grid();
        assert_eq!(g.item(0).map(|e| e.value), Some("house"));
        assert!(g.item(999).is_none());
    }
}
