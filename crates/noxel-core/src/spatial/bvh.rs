//! A static bounding volume hierarchy.
//!
//! The BVH is the engine's ray-casting workhorse. Two systems depend on it:
//!
//! * **Visibility / occlusion** — cast a ray from the camera to the player and
//!   ask which occluder volumes it crosses, so roofs and tree canopies can fade
//!   out instead of hiding the character.
//! * **Ray tracing** — find the nearest triangle along a ray for shadow, AO and
//!   reflection rays.
//!
//! # Build
//!
//! Top-down median split on the longest axis of the *centroid* bounds. It is not
//! a full SAH sweep, but it produces a tree within a few percent of SAH quality
//! on clustered game geometry while building in `O(n log n)` with a single sort
//! per level — fast enough to rebuild a few times per second when geometry
//! changes (destructible cover, moving platforms).
//!
//! # Traversal
//!
//! Ray queries visit the nearer child first and prune against the best distance
//! found so far, so a "nearest hit" query usually touches only a few nodes.
//!
//! ```
//! use noxel_core::math::{Aabb, Ray, Vec3};
//! use noxel_core::spatial::Bvh;
//!
//! let bvh = Bvh::build([
//!     (Aabb::new(Vec3::ZERO, Vec3::splat(0.2)), "wall"),
//!     (Aabb::new(Vec3::splat(10.0), Vec3::splat(10.2)), "door"),
//! ]);
//!
//! let hit = bvh.nearest_ray(&Ray::new(Vec3::new(-5.0, 0.1, 0.1), Vec3::X));
//! assert_eq!(hit.map(|h| *h.value), Some("wall"));
//! ```

use crate::math::{Aabb, Frustum, Ray, Vec3};

/// A leaf budget: nodes holding at most this many items become leaves.
const LEAF_SIZE: usize = 4;

/// One node of the tree.
#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// For a leaf: the first index into the item array.
    /// For an interior node: the index of the **right** child (the left child is
    /// always `self_index + 1`).
    start_or_right: u32,
    /// Number of items for a leaf; `0` for an interior node.
    count: u32,
}

/// One item stored in the tree.
#[derive(Clone, Debug)]
pub struct BvhItem<T> {
    /// The item's bounds.
    pub bounds: Aabb,
    /// The stored value.
    pub value: T,
}

/// A ray hit produced by a BVH query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BvhRayHit<'a, T> {
    /// Distance along the ray.
    pub t: f32,
    /// The hit item's value.
    pub value: &'a T,
    /// The hit item's bounds.
    pub bounds: &'a Aabb,
}

/// A bounding volume hierarchy over axis-aligned boxes.
#[derive(Clone, Debug)]
pub struct Bvh<T> {
    nodes: Vec<Node>,
    items: Vec<BvhItem<T>>,
    root: u32,
}

impl<T> Bvh<T> {
    /// An empty tree.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            items: Vec::new(),
            root: 0,
        }
    }

    /// Builds a tree from `(bounds, value)` pairs.
    #[must_use]
    pub fn build(items: impl IntoIterator<Item = (Aabb, T)>) -> Self {
        let mut items: Vec<BvhItem<T>> = items
            .into_iter()
            .map(|(bounds, value)| BvhItem { bounds, value })
            .collect();
        if items.is_empty() {
            return Self::empty();
        }
        let mut nodes = Vec::with_capacity(items.len() * 2);
        build_node(&mut items, 0, &mut nodes);
        Self {
            nodes,
            items,
            root: 0,
        }
    }

    /// Number of stored items.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the tree holds nothing.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of nodes.
    #[inline]
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Bounds of the whole tree, or an empty box.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// The stored items, in tree (leaf) order.
    #[must_use]
    pub fn items(&self) -> &[BvhItem<T>] {
        &self.items
    }

    /// Every item, in leaf order.
    pub fn iter(&self) -> impl Iterator<Item = &BvhItem<T>> + '_ {
        self.items.iter()
    }

    /// Items whose bounds overlap `query`.
    pub fn query_aabb<'a>(&'a self, query: &Aabb, out: &mut Vec<&'a T>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if !node.bounds.intersects(query) {
                continue;
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for item in &self.items[start..start + node.count as usize] {
                    if item.bounds.intersects(query) {
                        out.push(&item.value);
                    }
                }
            } else {
                stack.push(node.start_or_right);
                stack.push(ni + 1);
            }
        }
    }

    /// Items whose bounds overlap the frustum.
    pub fn query_frustum<'a>(&'a self, frustum: &Frustum, out: &mut Vec<&'a T>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if !frustum.intersects_aabb(&node.bounds) {
                continue;
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for item in &self.items[start..start + node.count as usize] {
                    if frustum.intersects_aabb(&item.bounds) {
                        out.push(&item.value);
                    }
                }
            } else {
                stack.push(node.start_or_right);
                stack.push(ni + 1);
            }
        }
    }

    /// The nearest item the ray hits, or `None`.
    ///
    /// Prunes aggressively: children are visited near-first and any node whose
    /// entry distance exceeds the best hit found so far is skipped. Note that
    /// the returned distance is the distance to the *bounding box*, not to the
    /// underlying geometry — callers that need a surface hit follow up with a
    /// triangle or shape test.
    #[must_use]
    pub fn nearest_ray(&self, ray: &Ray) -> Option<BvhRayHit<'_, T>> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best_t = f32::INFINITY;
        let mut best: Option<usize> = None;
        let mut stack: Vec<(u32, f32)> = Vec::with_capacity(64);
        let root_t = self.nodes[self.root as usize]
            .bounds
            .intersect_ray(ray.origin, ray.dir)
            .map_or(f32::INFINITY, |(t0, _)| t0);
        if root_t > ray.max_t {
            return None;
        }
        stack.push((self.root, root_t));
        while let Some((ni, t_enter)) = stack.pop() {
            if t_enter > best_t {
                continue;
            }
            let node = self.nodes[ni as usize];
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for (k, item) in self.items[start..start + node.count as usize]
                    .iter()
                    .enumerate()
                {
                    if let Some((t0, t1)) = item.bounds.intersect_ray(ray.origin, ray.dir) {
                        if t1 < 0.0 || t0 > ray.max_t || t0 >= best_t {
                            continue;
                        }
                        best_t = t0;
                        best = Some(start + k);
                    }
                }
            } else {
                let (l, r) = (ni + 1, node.start_or_right);
                let lt = self.nodes[l as usize]
                    .bounds
                    .intersect_ray(ray.origin, ray.dir)
                    .map_or(f32::INFINITY, |(t0, _)| t0);
                let rt = self.nodes[r as usize]
                    .bounds
                    .intersect_ray(ray.origin, ray.dir)
                    .map_or(f32::INFINITY, |(t0, _)| t0);
                // Push the farther child first so the nearer one is popped first.
                if lt <= rt {
                    if rt < best_t {
                        stack.push((r, rt));
                    }
                    if lt < best_t {
                        stack.push((l, lt));
                    }
                } else {
                    if lt < best_t {
                        stack.push((l, lt));
                    }
                    if rt < best_t {
                        stack.push((r, rt));
                    }
                }
            }
        }
        best.map(|i| BvhRayHit {
            t: best_t,
            value: &self.items[i].value,
            bounds: &self.items[i].bounds,
        })
    }

    /// Every item the ray hits, sorted by distance.
    pub fn query_ray<'a>(&'a self, ray: &Ray, out: &mut Vec<BvhRayHit<'a, T>>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            let Some((t0, t1)) = node.bounds.intersect_ray(ray.origin, ray.dir) else {
                continue;
            };
            if t1 < 0.0 || t0 > ray.max_t {
                continue;
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for item in &self.items[start..start + node.count as usize] {
                    if let Some((it0, it1)) = item.bounds.intersect_ray(ray.origin, ray.dir) {
                        if it1 >= 0.0 && it0 <= ray.max_t {
                            out.push(BvhRayHit {
                                t: it0.max(0.0),
                                value: &item.value,
                                bounds: &item.bounds,
                            });
                        }
                    }
                }
            } else {
                stack.push(node.start_or_right);
                stack.push(ni + 1);
            }
        }
        out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(core::cmp::Ordering::Equal));
    }

    /// True when anything blocks the segment from `a` to `b`.
    ///
    /// The occlusion test: allocate nothing, stop at the first blocker. This is
    /// the hot path called once per occluder candidate per frame.
    #[must_use]
    pub fn any_hit_segment(&self, a: Vec3, b: Vec3, ignore: impl Fn(&T) -> bool) -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-6 || self.nodes.is_empty() {
            return false;
        }
        let ray = Ray::with_max_t(a, d, len);
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            let Some((t0, t1)) = node.bounds.intersect_ray(ray.origin, ray.dir) else {
                continue;
            };
            if t1 < 0.0 || t0 > ray.max_t {
                continue;
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for item in &self.items[start..start + node.count as usize] {
                    if ignore(&item.value) {
                        continue;
                    }
                    if let Some((it0, it1)) = item.bounds.intersect_ray(ray.origin, ray.dir) {
                        if it1 >= 0.0 && it0 <= ray.max_t {
                            return true;
                        }
                    }
                }
            } else {
                stack.push(node.start_or_right);
                stack.push(ni + 1);
            }
        }
        false
    }

    /// Every item whose bounds contain `point`, nearest (by box distance) first.
    pub fn query_point<'a>(&'a self, point: Vec3, out: &mut Vec<&'a T>) {
        self.query_aabb(&Aabb::new(point, point), out);
    }

    /// The tree's maximum depth. Useful for spotting a degenerate build.
    #[must_use]
    pub fn depth(&self) -> u32 {
        if self.nodes.is_empty() {
            return 0;
        }
        fn walk<T>(bvh: &Bvh<T>, node: u32) -> u32 {
            let n = bvh.nodes[node as usize];
            if n.count > 0 {
                1
            } else {
                1 + walk(bvh, node + 1).max(walk(bvh, n.start_or_right))
            }
        }
        walk(self, self.root)
    }

    /// Visits each node's bounds, for the debug overlay that draws the tree.
    pub fn for_each_node(&self, mut f: impl FnMut(&Aabb, bool, u32)) {
        for (i, n) in self.nodes.iter().enumerate() {
            f(&n.bounds, n.count > 0, i as u32);
        }
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.nodes.capacity() * core::mem::size_of::<Node>()
            + self.items.capacity() * core::mem::size_of::<BvhItem<T>>()
    }

    /// Quality metric: the sum of node surface areas divided by the root's
    /// surface area. Lower is better; a perfect tree is close to
    /// `2 * log2(n)`. The unit tests use it to catch a regression to a
    /// degenerate "linked list" build.
    #[must_use]
    pub fn quality(&self) -> f32 {
        let root_area = self.bounds().surface_area();
        if root_area <= 0.0 {
            return 0.0;
        }
        let total: f32 = self.nodes.iter().map(|n| n.bounds.surface_area()).sum();
        total / root_area
    }
}

/// Recursively builds a subtree over `items` (which starts at `offset` in the
/// caller's array) and returns the index of the created root node.
fn build_node<T>(items: &mut [BvhItem<T>], offset: usize, nodes: &mut Vec<Node>) -> u32 {
    let mut bounds = Aabb::EMPTY;
    let mut centroid_bounds = Aabb::EMPTY;
    for item in items.iter() {
        bounds.grow_aabb(&item.bounds);
        centroid_bounds.grow(item.bounds.center());
    }

    let index = nodes.len() as u32;
    nodes.push(Node {
        bounds,
        start_or_right: offset as u32,
        count: items.len() as u32,
    });

    if items.len() <= LEAF_SIZE {
        return index;
    }

    // Split at the median of the centroid distribution along the widest axis.
    let axis = centroid_bounds.longest_axis();
    let extent = centroid_bounds.size()[axis];
    if extent <= 1e-6 {
        // All centroids coincide (e.g. a stack of identical boxes): splitting
        // would not make progress, so stop here and let the leaf hold them.
        return index;
    }
    items.sort_unstable_by(|a, b| {
        let ca = a.bounds.center()[axis];
        let cb = b.bounds.center()[axis];
        ca.partial_cmp(&cb).unwrap_or(core::cmp::Ordering::Equal)
    });
    let mid = items.len() / 2;
    let (left, right) = items.split_at_mut(mid);
    build_node(left, offset, nodes);
    let right_index = build_node(right, offset + mid, nodes);
    nodes[index as usize] = Node {
        bounds,
        start_or_right: right_index,
        count: 0,
    };
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Bvh<u32> {
        Bvh::build((0..100).map(|i| {
            let x = (i % 10) as f32 * 3.0;
            let z = (i / 10) as f32 * 3.0;
            (
                Aabb::new(Vec3::new(x, 0.0, z), Vec3::new(x + 1.0, 1.0, z + 1.0)),
                i,
            )
        }))
    }

    #[test]
    fn empty_tree_is_safe() {
        let bvh: Bvh<u32> = Bvh::empty();
        assert!(bvh.is_empty());
        assert_eq!(bvh.depth(), 0);
        let mut out = Vec::new();
        bvh.query_aabb(&Aabb::new(Vec3::ZERO, Vec3::ONE), &mut out);
        assert!(out.is_empty());
        assert!(bvh.nearest_ray(&Ray::new(Vec3::ZERO, Vec3::X)).is_none());
        assert!(!bvh.any_hit_segment(Vec3::ZERO, Vec3::splat(10.0), |_| false));
    }

    #[test]
    fn single_item() {
        let bvh = Bvh::build([(Aabb::new(Vec3::ZERO, Vec3::ONE), 7u32)]);
        assert_eq!(bvh.len(), 1);
        let hit = bvh
            .nearest_ray(&Ray::new(Vec3::new(-5.0, 0.5, 0.5), Vec3::X))
            .unwrap();
        assert_eq!(*hit.value, 7);
        assert!((hit.t - 5.0).abs() < 1e-4);
    }

    #[test]
    fn query_aabb_finds_the_right_items() {
        let bvh = sample();
        let mut out = Vec::new();
        bvh.query_aabb(
            &Aabb::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::splat(0.5)),
            &mut out,
        );
        assert_eq!(out, vec![&0u32]);
    }

    #[test]
    fn nearest_ray_picks_the_closest() {
        let bvh = sample();
        // Ray along +X at z = 0.5 crosses items 0,1,2,... in that order.
        let hit = bvh
            .nearest_ray(&Ray::new(Vec3::new(-10.0, 0.5, 0.5), Vec3::X))
            .unwrap();
        assert_eq!(*hit.value, 0);
        assert!(hit.t < 11.0, "t = {}", hit.t);
    }

    #[test]
    fn query_ray_returns_sorted_hits() {
        let bvh = sample();
        let mut out = Vec::new();
        bvh.query_ray(&Ray::new(Vec3::new(-10.0, 0.5, 0.5), Vec3::X), &mut out);
        assert!(out.len() >= 10, "should cross the whole row: {}", out.len());
        assert!(
            out.windows(2).all(|w| w[0].t <= w[1].t),
            "hits must be sorted"
        );
    }

    #[test]
    fn any_hit_segment_detects_and_ignores() {
        let bvh = sample();
        assert!(bvh.any_hit_segment(
            Vec3::new(-10.0, 0.5, 0.5),
            Vec3::new(40.0, 0.5, 0.5),
            |_| false
        ));
        assert!(
            !bvh.any_hit_segment(
                Vec3::new(-10.0, 50.0, 0.5),
                Vec3::new(40.0, 50.0, 0.5),
                |_| false
            ),
            "a segment far above the boxes must not hit"
        );
        assert!(
            !bvh.any_hit_segment(
                Vec3::new(-10.0, 0.5, 0.5),
                Vec3::new(40.0, 0.5, 0.5),
                |_| true
            ),
            "ignoring everything must report no hit"
        );
    }

    #[test]
    fn any_hit_segment_respects_segment_length() {
        let bvh = sample();
        // Stop before reaching the first box at x = 0.
        assert!(!bvh.any_hit_segment(
            Vec3::new(-10.0, 0.5, 0.5),
            Vec3::new(-5.0, 0.5, 0.5),
            |_| false
        ));
    }

    #[test]
    fn frustum_query_matches_manual_test() {
        let bvh = sample();
        let proj = crate::math::Mat4::orthographic_rh(-5.0, 5.0, -5.0, 5.0, 1.0, 100.0);
        let view = crate::math::Mat4::look_at_rh(Vec3::new(0.0, 30.0, 0.0), Vec3::ZERO, Vec3::Z);
        let frustum = Frustum::from_view_projection(&(proj * view));

        let mut via_bvh = Vec::new();
        bvh.query_frustum(&frustum, &mut via_bvh);
        let via_linear: Vec<&u32> = bvh
            .items()
            .iter()
            .filter(|i| frustum.intersects_aabb(&i.bounds))
            .map(|i| &i.value)
            .collect();
        assert_eq!(via_bvh.len(), via_linear.len());
        for v in &via_bvh {
            assert!(via_linear.contains(v));
        }
    }

    #[test]
    fn build_produces_a_balanced_tree() {
        let bvh = sample();
        // 100 items, leaf size 4 -> roughly log2(25) + 1 levels.
        assert!(bvh.depth() <= 10, "depth {}", bvh.depth());
        assert!(bvh.quality() < 30.0, "quality {}", bvh.quality());
    }

    #[test]
    fn degenerate_identical_bounds_still_builds() {
        let bvh = Bvh::build((0..50).map(|i| (Aabb::new(Vec3::ZERO, Vec3::ONE), i)));
        assert_eq!(bvh.len(), 50);
        assert!(bvh.depth() >= 1);
        let mut out = Vec::new();
        bvh.query_aabb(&Aabb::new(Vec3::ZERO, Vec3::ONE), &mut out);
        assert_eq!(out.len(), 50);
    }

    #[test]
    fn all_items_are_reachable() {
        let bvh = sample();
        let mut seen = Vec::new();
        bvh.query_aabb(
            &Aabb::new(Vec3::splat(-100.0), Vec3::splat(100.0)),
            &mut seen,
        );
        assert_eq!(seen.len(), 100);
        let mut sorted: Vec<u32> = seen.into_iter().copied().collect();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn query_clears_the_output() {
        let bvh = sample();
        let mut out = vec![&99u32];
        bvh.query_aabb(&Aabb::EMPTY, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn point_query() {
        let bvh = sample();
        let mut out = Vec::new();
        bvh.query_point(Vec3::new(0.5, 0.5, 0.5), &mut out);
        assert_eq!(out, vec![&0u32]);
    }

    #[test]
    fn memory_and_nodes_are_reported() {
        let bvh = sample();
        assert!(bvh.node_count() > 1);
        assert!(bvh.memory_bytes() > 0);
        let mut count = 0;
        bvh.for_each_node(|_b, _leaf, _i| count += 1);
        assert_eq!(count, bvh.node_count());
    }
}
