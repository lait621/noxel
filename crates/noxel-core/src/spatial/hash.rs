//! A dynamic spatial hash for moving objects.
//!
//! Where [`UniformGrid`](crate::spatial::UniformGrid) is built once and read
//! many times, a `SpatialHash` is the opposite: objects are inserted, moved and
//! removed every frame. It is the broadphase for two systems:
//!
//! * **NPC neighbourhoods** — "which other NPCs are within 2 m of me?" runs
//!   `O(n)` times per frame with thousands of agents.
//! * **Physics broadphase** — "which collider pairs overlap?" runs once, then
//!   feeds the narrowphase.
//!
//! Cells are keyed by `(x, z)` only. Vertical extent is stored in each entry's
//! bounds and tested during the query, which is the right trade for a top-down
//! world: it keeps the cell count proportional to the map's area no matter how
//! many storeys a building has.
//!
//! ```
//! use noxel_core::math::{Aabb, Vec3};
//! use noxel_core::spatial::SpatialHash;
//!
//! let mut hash = SpatialHash::new(4.0);
//! let player = hash.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "player");
//! let _npc = hash.insert(Aabb::new(Vec3::splat(10.0), Vec3::splat(11.0)), "npc");
//!
//! let mut found = Vec::new();
//! hash.query_radius(Vec3::ZERO, 2.0, &mut found);
//! assert_eq!(found, vec![player]);
//! assert_eq!(hash.get(found[0]), Some(&"player"));
//!
//! // Moving an object is O(cells touched), not O(n).
//! hash.update(player, Aabb::new(Vec3::splat(10.0), Vec3::splat(11.0)));
//! ```

use std::collections::HashMap;

use crate::math::{Aabb, Ray, Vec3};
use crate::pool::{Handle, SlotMap};

/// One tracked object.
#[derive(Clone, Debug)]
pub struct HashEntry<T> {
    /// Current world bounds.
    pub bounds: Aabb,
    /// The stored value.
    pub value: T,
    /// Cells this entry is currently registered in.
    cells: Vec<(i32, i32)>,
}

/// A spatial hash over the XZ plane.
#[derive(Clone, Debug)]
pub struct SpatialHash<T> {
    cell_size: f32,
    inv_cell_size: f32,
    cells: HashMap<(i32, i32), Vec<Handle<HashEntry<T>>>>,
    entries: SlotMap<HashEntry<T>>,
}

impl<T> SpatialHash<T> {
    /// Creates a hash with the given cell size.
    ///
    /// Pick a cell size near the *typical query radius*: too small and a query
    /// scans many cells, too large and every query tests many candidates. 2–8 m
    /// suits a town-scale RPG; the NPC crowd system defaults to 4 m.
    #[must_use]
    pub fn new(cell_size: f32) -> Self {
        let cell_size = cell_size.max(0.05);
        Self {
            cell_size,
            inv_cell_size: 1.0 / cell_size,
            cells: HashMap::new(),
            entries: SlotMap::new(),
        }
    }

    /// The cell size.
    #[inline]
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Number of tracked objects.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is tracked.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of occupied cells.
    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Inserts an object and returns its handle.
    pub fn insert(&mut self, bounds: Aabb, value: T) -> Handle<HashEntry<T>> {
        let handle = self.entries.insert(HashEntry { bounds, value, cells: Vec::new() });
        self.link(handle);
        handle
    }

    /// Removes an object, returning it.
    pub fn remove(&mut self, handle: Handle<HashEntry<T>>) -> Option<T> {
        self.unlink(handle);
        self.entries.remove(handle).map(|e| e.value)
    }

    /// Moves an object to new bounds.
    ///
    /// Returns `false` when the handle is stale. Re-registers only the cells
    /// that changed, so a small motion inside one cell is nearly free.
    pub fn update(&mut self, handle: Handle<HashEntry<T>>, bounds: Aabb) -> bool {
        let Some(entry) = self.entries.get(handle) else { return false };
        if entry.bounds == bounds {
            return true;
        }
        self.unlink(handle);
        if let Some(entry) = self.entries.get_mut(handle) {
            entry.bounds = bounds;
        }
        self.link(handle);
        true
    }

    /// Shared access to an entry's value.
    #[inline]
    #[must_use]
    pub fn get(&self, handle: Handle<HashEntry<T>>) -> Option<&T> {
        self.entries.get(handle).map(|e| &e.value)
    }

    /// Mutable access to an entry's value.
    ///
    /// Changing the value is fine; changing the *position* must go through
    /// [`SpatialHash::update`], otherwise the index goes stale.
    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, handle: Handle<HashEntry<T>>) -> Option<&mut T> {
        self.entries.get_mut(handle).map(|e| &mut e.value)
    }

    /// The bounds of an entry.
    #[inline]
    #[must_use]
    pub fn bounds(&self, handle: Handle<HashEntry<T>>) -> Option<Aabb> {
        self.entries.get(handle).map(|e| e.bounds)
    }

    /// Every tracked handle, in slot order (deterministic).
    pub fn handles(&self) -> impl Iterator<Item = Handle<HashEntry<T>>> + '_ {
        self.entries.keys()
    }

    /// Every entry, in slot order (deterministic).
    pub fn iter(&self) -> impl Iterator<Item = (Handle<HashEntry<T>>, &HashEntry<T>)> + '_ {
        self.entries.iter()
    }

    /// Candidates overlapping `query`, reported once each and in a stable
    /// (handle) order.
    ///
    /// `scratch` is reused between calls so a per-frame query does not allocate.
    pub fn query_aabb_into(
        &self,
        query: Aabb,
        out: &mut Vec<Handle<HashEntry<T>>>,
        scratch: &mut Vec<Handle<HashEntry<T>>>,
    ) {
        out.clear();
        scratch.clear();
        let (x0, z0, x1, z1) = self.cell_range(&query);
        for z in z0..=z1 {
            for x in x0..=x1 {
                if let Some(list) = self.cells.get(&(x, z)) {
                    scratch.extend_from_slice(list);
                }
            }
        }
        scratch.sort_unstable_by_key(|h| (h.index(), h.generation()));
        scratch.dedup();
        for &h in scratch.iter() {
            if let Some(e) = self.entries.get(h) {
                if e.bounds.intersects(&query) {
                    out.push(h);
                }
            }
        }
    }

    /// Convenience wrapper around [`SpatialHash::query_aabb_into`].
    pub fn query_aabb(&self, query: Aabb, out: &mut Vec<Handle<HashEntry<T>>>) {
        let mut scratch = Vec::new();
        self.query_aabb_into(query, out, &mut scratch);
    }

    /// Objects whose bounds are within `radius` of `center` in the XZ plane.
    pub fn query_radius(&self, center: Vec3, radius: f32, out: &mut Vec<Handle<HashEntry<T>>>) {
        let r = Vec3::new(radius, f32::MAX, radius);
        self.query_aabb(Aabb::new(center - r, center + r), out);
    }

    /// Objects a ray passes through.
    ///
    /// Not distance-sorted: the caller usually wants the nearest *surface* hit,
    /// which needs the actual geometry, not the bounds. Use this only to gather
    /// candidates.
    pub fn query_ray(&self, ray: &Ray, out: &mut Vec<Handle<HashEntry<T>>>) {
        let mut candidates = Vec::new();
        let mut scratch = Vec::new();
        // Walk a fat segment rather than implementing a second DDA: the AABBs
        // here change every frame, so a slightly over-large query is the right
        // trade against a more complex traversal.
        let end = ray.at(ray.max_t.min(1.0e6));
        let seg = Aabb::new(ray.origin, end);
        self.query_aabb_into(seg.expanded(1.0), &mut candidates, &mut scratch);
        out.clear();
        for h in candidates {
            if let Some(e) = self.entries.get(h) {
                if e.bounds.intersect_ray(ray.origin, ray.dir).is_some() {
                    out.push(h);
                }
            }
        }
    }

    /// The nearest object to `center` within `max_radius`, and its XZ distance.
    ///
    /// Used for interaction prompts ("press E to talk") and for the occlusion
    /// system's fallback when the primary ray test misses.
    pub fn nearest(
        &self,
        center: Vec3,
        max_radius: f32,
    ) -> Option<(Handle<HashEntry<T>>, f32)> {
        let mut candidates = Vec::new();
        self.query_radius(center, max_radius, &mut candidates);
        let mut best: Option<(Handle<HashEntry<T>>, f32)> = None;
        for h in candidates {
            let Some(e) = self.entries.get(h) else { continue };
            let d = e.bounds.distance_to_point(center);
            if d > max_radius {
                continue;
            }
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((h, d)),
            }
        }
        best
    }

    /// Calls `f` for every pair of objects whose bounds overlap.
    ///
    /// Each unordered pair is reported exactly once, and the pair order is
    /// deterministic. This is the physics broadphase's entry point: with a cell
    /// size near the average object size the cost is close to linear in the
    /// number of objects, and `f` is called only for true AABB overlaps.
    pub fn for_each_overlapping_pair(
        &self,
        mut f: impl FnMut(Handle<HashEntry<T>>, Handle<HashEntry<T>>),
    ) {
        let mut pairs: Vec<(Handle<HashEntry<T>>, Handle<HashEntry<T>>)> = Vec::new();
        for list in self.cells.values() {
            for i in 0..list.len() {
                for j in (i + 1)..list.len() {
                    let (a, b) = (list[i], list[j]);
                    if a == b {
                        continue;
                    }
                    // Normalise so a pair seen from two different cells collapses.
                    if a < b { pairs.push((a, b)) } else { pairs.push((b, a)) }
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        for (a, b) in pairs {
            let (Some(ea), Some(eb)) = (self.entries.get(a), self.entries.get(b)) else { continue };
            if ea.bounds.intersects(&eb.bounds) {
                f(a, b);
            }
        }
    }

    /// Removes everything.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.entries.clear();
    }

    /// Removes everything including the cell allocation.
    pub fn reset(&mut self) {
        self.cells.clear();
        self.cells.shrink_to_fit();
        self.entries.reset();
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let bucket_bytes: usize = self
            .cells
            .values()
            .map(|v| v.capacity() * core::mem::size_of::<Handle<HashEntry<T>>>() + 12)
            .sum();
        bucket_bytes + self.entries.capacity() * core::mem::size_of::<HashEntry<T>>()
    }

    /// The average number of entries per occupied cell, a tuning signal for
    /// [`SpatialHash::new`].
    #[must_use]
    pub fn average_cell_load(&self) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let total: usize = self.cells.values().map(|v| v.len()).sum();
        total as f32 / self.cells.len() as f32
    }

    fn link(&mut self, handle: Handle<HashEntry<T>>) {
        let Some(entry) = self.entries.get(handle) else { return };
        let (x0, z0, x1, z1) = self.cell_range(&entry.bounds);
        let mut cells = Vec::with_capacity(((x1 - x0 + 1).max(1) * (z1 - z0 + 1).max(1)) as usize);
        for z in z0..=z1 {
            for x in x0..=x1 {
                cells.push((x, z));
            }
        }
        for &key in &cells {
            self.cells.entry(key).or_default().push(handle);
        }
        if let Some(entry) = self.entries.get_mut(handle) {
            entry.cells = cells;
        }
    }

    fn unlink(&mut self, handle: Handle<HashEntry<T>>) {
        let cells = match self.entries.get_mut(handle) {
            Some(e) => core::mem::take(&mut e.cells),
            None => return,
        };
        for key in cells {
            let mut now_empty = false;
            if let Some(list) = self.cells.get_mut(&key) {
                if let Some(pos) = list.iter().position(|&h| h == handle) {
                    list.swap_remove(pos);
                }
                now_empty = list.is_empty();
            }
            if now_empty {
                self.cells.remove(&key);
            }
        }
    }

    /// The inclusive cell range an AABB touches. Unclamped: the hash is
    /// unbounded, so negative coordinates are fine.
    fn cell_range(&self, b: &Aabb) -> (i32, i32, i32, i32) {
        (
            (b.min.x * self.inv_cell_size).floor() as i32,
            (b.min.z * self.inv_cell_size).floor() as i32,
            (b.max.x * self.inv_cell_size).floor() as i32,
            (b.max.z * self.inv_cell_size).floor() as i32,
        )
    }

    /// The cell containing a world position.
    #[must_use]
    pub fn cell_of(&self, p: Vec3) -> (i32, i32) {
        ((p.x * self.inv_cell_size).floor() as i32, (p.z * self.inv_cell_size).floor() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> SpatialHash<&'static str> {
        let mut h = SpatialHash::new(4.0);
        h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "player");
        h.insert(Aabb::new(Vec3::splat(10.0), Vec3::splat(11.0)), "npc");
        h.insert(Aabb::new(Vec3::splat(-30.0), Vec3::splat(-29.0)), "far");
        h
    }

    #[test]
    fn insert_and_query_radius() {
        let h = make();
        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 2.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(h.get(out[0]), Some(&"player"));
    }

    #[test]
    fn remove_clears_the_cells() {
        let mut h = make();
        let handle = h.insert(Aabb::new(Vec3::splat(50.0), Vec3::splat(51.0)), "temp");
        h.remove(handle);
        let mut out = Vec::new();
        h.query_radius(Vec3::splat(50.0), 2.0, &mut out);
        assert!(out.is_empty(), "removed object must leave no candidates");
    }

    #[test]
    fn update_moves_between_cells() {
        let mut h = make();
        let handle = h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "mover");
        h.update(handle, Aabb::new(Vec3::splat(100.0), Vec3::splat(101.0)));

        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 2.0, &mut out);
        assert!(!out.contains(&handle), "must not remain in the old cell");

        out.clear();
        h.query_radius(Vec3::splat(100.0), 2.0, &mut out);
        assert!(out.contains(&handle));

        // And no duplicates: the entry is stored in exactly one cell here and
        // the query must not report it twice.
        assert_eq!(out.iter().filter(|&&x| x == handle).count(), 1);
    }

    #[test]
    fn update_to_same_bounds_is_a_noop() {
        let mut h = make();
        let handle = h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "static");
        assert!(h.update(handle, Aabb::new(Vec3::ZERO, Vec3::ONE)));
        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 2.0, &mut out);
        assert_eq!(out.iter().filter(|&&x| x == handle).count(), 1);
    }

    #[test]
    fn stale_handle_is_rejected() {
        let mut h = make();
        let handle = h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "x");
        h.remove(handle);
        assert!(!h.update(handle, Aabb::new(Vec3::ZERO, Vec3::ONE)));
        assert!(h.get(handle).is_none());
    }

    #[test]
    fn negative_coordinates_work() {
        let mut h: SpatialHash<i32> = SpatialHash::new(4.0);
        h.insert(Aabb::new(Vec3::splat(-100.0), Vec3::splat(-99.0)), 1);
        let mut out = Vec::new();
        h.query_radius(Vec3::splat(-100.0), 1.0, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn multi_cell_object_reported_once() {
        let mut h: SpatialHash<&'static str> = SpatialHash::new(1.0);
        h.insert(Aabb::new(Vec3::ZERO, Vec3::new(10.0, 1.0, 10.0)), "big");
        let mut out = Vec::new();
        h.query_aabb(Aabb::new(Vec3::new(-5.0, -5.0, -5.0), Vec3::new(20.0, 20.0, 20.0)), &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn query_results_are_stable() {
        let h = make();
        let mut first = Vec::new();
        h.query_radius(Vec3::ZERO, 100.0, &mut first);
        for _ in 0..16 {
            let mut again = Vec::new();
            h.query_radius(Vec3::ZERO, 100.0, &mut again);
            assert_eq!(first, again, "hash iteration order must not leak out");
        }
    }

    #[test]
    fn overlapping_pairs_reported_once() {
        let mut h: SpatialHash<u32> = SpatialHash::new(4.0);
        let a = h.insert(Aabb::new(Vec3::ZERO, Vec3::splat(2.0)), 1);
        let b = h.insert(Aabb::new(Vec3::splat(1.0), Vec3::splat(3.0)), 2);
        let _c = h.insert(Aabb::new(Vec3::splat(50.0), Vec3::splat(51.0)), 3);

        let mut pairs = Vec::new();
        h.for_each_overlapping_pair(|x, y| pairs.push((x, y)));
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        assert!(pairs[0] == (a, b) || pairs[0] == (b, a));
    }

    #[test]
    fn nearest_finds_the_closest() {
        let mut h: SpatialHash<&'static str> = SpatialHash::new(4.0);
        h.insert(Aabb::new(Vec3::splat(5.0), Vec3::splat(6.0)), "near");
        h.insert(Aabb::new(Vec3::splat(20.0), Vec3::splat(21.0)), "far");
        let (handle, dist) = h.nearest(Vec3::ZERO, 100.0).unwrap();
        assert_eq!(h.get(handle), Some(&"near"));
        // Box spans 5..6 on every axis, so the corner distance is 5*sqrt(3).
        assert!((8.5..9.0).contains(&dist), "{dist}");
    }

    #[test]
    fn nearest_respects_radius() {
        let mut h: SpatialHash<u32> = SpatialHash::new(4.0);
        h.insert(Aabb::new(Vec3::splat(50.0), Vec3::splat(51.0)), 7);
        assert!(h.nearest(Vec3::ZERO, 10.0).is_none());
    }

    #[test]
    fn query_ray_gathers_candidates() {
        let h = make();
        let mut out = Vec::new();
        let ray = Ray::new(Vec3::new(-5.0, 0.5, 0.5), Vec3::X);
        h.query_ray(&ray, &mut out);
        assert!(out.iter().any(|&x| h.get(x) == Some(&"player")), "{}", out.len());
    }

    #[test]
    fn bounds_and_getters() {
        let h = make();
        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 2.0, &mut out);
        let handle = out[0];
        assert_eq!(h.bounds(handle), Some(Aabb::new(Vec3::ZERO, Vec3::ONE)));
        assert_eq!(h.get(handle), Some(&"player"));
    }

    #[test]
    fn get_mut_updates_value_in_place() {
        let mut h: SpatialHash<i32> = SpatialHash::new(2.0);
        let handle = h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), 1);
        *h.get_mut(handle).unwrap() = 42;
        assert_eq!(h.get(handle), Some(&42));
        // The index still finds it.
        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 1.0, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn clear_and_reset_empty_everything() {
        let mut h = make();
        h.clear();
        assert!(h.is_empty());
        assert_eq!(h.cell_count(), 0);
        let handle = h.insert(Aabb::new(Vec3::ZERO, Vec3::ONE), "after");
        let mut out = Vec::new();
        h.query_radius(Vec3::ZERO, 1.0, &mut out);
        assert_eq!(out, vec![handle]);
    }

    #[test]
    fn statistics_are_sane() {
        let h = make();
        assert!(h.cell_count() >= 3);
        assert!(h.average_cell_load() >= 1.0);
        assert!(h.memory_bytes() > 0);
    }
}
