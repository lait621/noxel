//! A BVH specialised for ray/triangle intersection.
//!
//! [`noxel_core::spatial::Bvh`] answers "which *boxes* does this ray cross". The
//! ray tracer needs "which *triangle* does this ray hit, and where", which means
//! the triangle test has to happen inside the traversal — testing every triangle
//! in a leaf only after the tree walk would defeat the point of the tree.
//!
//! # Layout
//!
//! Nodes are stored in an array with the same trick the core BVH uses: a node's
//! left child is always `index + 1`, so an interior node only needs to store its
//! right child. Leaves store `(start, count)` into a permutation array of
//! triangle indices, so the triangles themselves never move.
//!
//! # Traversal
//!
//! * [`TriangleBvh::intersect`] visits the nearer child first and prunes with
//!   the best distance so far — a primary ray usually touches a handful of
//!   nodes.
//! * [`TriangleBvh::any_hit`] has no ordering at all: a shadow ray can stop at
//!   the first blocker it finds.

use noxel_core::math::{Aabb, Ray, Vec3};

/// A triangle in world space.
#[derive(Clone, Copy, Debug)]
pub struct WorldTriangle {
    /// First vertex position.
    pub a: Vec3,
    /// Second vertex position.
    pub b: Vec3,
    /// Third vertex position.
    pub c: Vec3,
    /// Normal at `a`.
    pub na: Vec3,
    /// Normal at `b`.
    pub nb: Vec3,
    /// Normal at `c`.
    pub nc: Vec3,
    /// Texture coordinate at `a`.
    pub uva: noxel_core::math::Vec2,
    /// Texture coordinate at `b`.
    pub uvb: noxel_core::math::Vec2,
    /// Texture coordinate at `c`.
    pub uvc: noxel_core::math::Vec2,
    /// Index into the frame's material table.
    pub material: u32,
    /// Instance id, for the ID buffer.
    pub instance: u32,
    /// Geometric normal (the plane's facing), precomputed.
    pub geometric_normal: Vec3,
}

impl WorldTriangle {
    /// Builds a triangle and computes its geometric normal and bounds.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        a: Vec3,
        b: Vec3,
        c: Vec3,
        na: Vec3,
        nb: Vec3,
        nc: Vec3,
        uva: noxel_core::math::Vec2,
        uvb: noxel_core::math::Vec2,
        uvc: noxel_core::math::Vec2,
        material: u32,
        instance: u32,
    ) -> Self {
        Self {
            a,
            b,
            c,
            na,
            nb,
            nc,
            uva,
            uvb,
            uvc,
            material,
            instance,
            geometric_normal: (b - a).cross(c - a).normalize_or_zero(),
        }
    }

    /// Axis-aligned bounds.
    #[inline]
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        Aabb::new(
            self.a.min(self.b).min(self.c),
            self.a.max(self.b).max(self.c),
        )
    }

    /// Centroid.
    #[inline]
    #[must_use]
    pub fn centroid(&self) -> Vec3 {
        (self.a + self.b + self.c) / 3.0
    }

    /// Area.
    #[inline]
    #[must_use]
    pub fn area(&self) -> f32 {
        (self.b - self.a).cross(self.c - self.a).length() * 0.5
    }

    /// A point on the triangle from barycentric coordinates.
    #[inline]
    #[must_use]
    pub fn point_at(&self, u: f32, v: f32) -> Vec3 {
        let w = 1.0 - u - v;
        self.a * w + self.b * u + self.c * v
    }

    /// The interpolated shading normal at a barycentric position.
    #[inline]
    #[must_use]
    pub fn normal_at(&self, u: f32, v: f32) -> Vec3 {
        let w = 1.0 - u - v;
        (self.na * w + self.nb * u + self.nc * v).normalize_or_zero()
    }

    /// The interpolated texture coordinate at a barycentric position.
    #[inline]
    #[must_use]
    pub fn uv_at(&self, u: f32, v: f32) -> noxel_core::math::Vec2 {
        let w = 1.0 - u - v;
        self.uva * w + self.uvb * u + self.uvc * v
    }

    /// Möller–Trumbore intersection. Returns `(t, u, v)`.
    ///
    /// The ray tracer's innermost loop, so it is written to keep the branch
    /// count low and to reject parallel rays immediately.
    #[inline]
    #[must_use]
    pub fn intersect(&self, ray: &Ray) -> Option<(f32, f32, f32)> {
        const EPS: f32 = 1e-9;
        let e1 = self.b - self.a;
        let e2 = self.c - self.a;
        let pvec = ray.dir.cross(e2);
        let det = e1.dot(pvec);
        // Two-sided: a normal-mapped or double-sided surface must be hit from
        // either side, and culling here would produce holes in reflections.
        if det.abs() < EPS {
            return None;
        }
        let inv_det = 1.0 / det;
        let tvec = ray.origin - self.a;
        let u = tvec.dot(pvec) * inv_det;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let qvec = tvec.cross(e1);
        let v = ray.dir.dot(qvec) * inv_det;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(qvec) * inv_det;
        if t < 1e-5 || t > ray.max_t {
            None
        } else {
            Some((t, u, v))
        }
    }
}

/// One BVH node.
#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// Leaf: first index into `tri_indices`. Interior: the right child.
    start_or_right: u32,
    /// Leaf: triangle count. Interior: zero.
    count: u32,
}

/// Leaf size. Four triangles keeps a leaf's data in one or two cache lines while
/// keeping the tree shallow.
const LEAF_SIZE: usize = 4;

/// A triangle BVH.
#[derive(Clone, Debug, Default)]
pub struct TriangleBvh {
    nodes: Vec<Node>,
    /// Triangle indices, permuted so each leaf's triangles are contiguous.
    tri_indices: Vec<u32>,
}

impl TriangleBvh {
    /// Builds a tree over `triangles`.
    #[must_use]
    pub fn build(triangles: &[WorldTriangle]) -> Self {
        if triangles.is_empty() {
            return Self::default();
        }
        let mut indices: Vec<u32> = (0..triangles.len() as u32).collect();
        let mut nodes = Vec::with_capacity(triangles.len() * 2);
        build_node(triangles, &mut indices, 0, &mut nodes);
        Self {
            nodes,
            tri_indices: indices,
        }
    }

    /// True when the tree is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Number of nodes.
    #[inline]
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The triangles this tree was built over, in the tree's order.
    #[must_use]
    pub fn ordered_indices(&self) -> &[u32] {
        &self.tri_indices
    }

    /// Bounds of the whole tree.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// The nearest triangle hit by `ray`.
    ///
    /// Returns the triangle's index, the distance and the barycentric
    /// coordinates.
    #[must_use]
    pub fn intersect(
        &self,
        triangles: &[WorldTriangle],
        ray: &Ray,
    ) -> Option<(u32, f32, f32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(u32, f32, f32, f32)> = None;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        let root = self.nodes[0];
        let Some((root_t0, root_t1)) = root.bounds.intersect_ray(ray.origin, ray.dir) else {
            return None;
        };
        if root_t1 < 0.0 || root_t0 > ray.max_t {
            return None;
        }
        stack.push(0);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            let Some((t0, t1)) = node.bounds.intersect_ray(ray.origin, ray.dir) else {
                continue;
            };
            if t1 < 0.0 || t0 > ray.max_t {
                continue;
            }
            if let Some((_, best_t, _, _)) = best {
                if t0 > best_t {
                    continue;
                }
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for &tri in &self.tri_indices[start..start + node.count as usize] {
                    let Some(t) = triangles.get(tri as usize) else {
                        continue;
                    };
                    if let Some((hit_t, u, v)) = t.intersect(ray) {
                        if best.is_none_or(|(_, bt, _, _)| hit_t < bt) {
                            best = Some((tri, hit_t, u, v));
                        }
                    }
                }
            } else {
                let (left, right) = (node_index + 1, node.start_or_right);
                let lt = self.nodes[left as usize]
                    .bounds
                    .intersect_ray(ray.origin, ray.dir)
                    .map_or(f32::INFINITY, |(a, _)| a);
                let rt = self.nodes[right as usize]
                    .bounds
                    .intersect_ray(ray.origin, ray.dir)
                    .map_or(f32::INFINITY, |(a, _)| a);
                // Push farther first so the nearer child is processed first and
                // the best-distance prune bites sooner.
                if lt <= rt {
                    if rt <= ray.max_t {
                        stack.push(right);
                    }
                    if lt <= ray.max_t {
                        stack.push(left);
                    }
                } else {
                    if lt <= ray.max_t {
                        stack.push(left);
                    }
                    if rt <= ray.max_t {
                        stack.push(right);
                    }
                }
            }
        }
        best
    }

    /// True when anything blocks the ray. Stops at the first hit.
    #[must_use]
    pub fn any_hit(&self, triangles: &[WorldTriangle], ray: &Ray) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            let Some((t0, t1)) = node.bounds.intersect_ray(ray.origin, ray.dir) else {
                continue;
            };
            if t1 < 0.0 || t0 > ray.max_t {
                continue;
            }
            if node.count > 0 {
                let start = node.start_or_right as usize;
                for &tri in &self.tri_indices[start..start + node.count as usize] {
                    if let Some(t) = triangles.get(tri as usize) {
                        if t.intersect(ray).is_some() {
                            return true;
                        }
                    }
                }
            } else {
                stack.push(node.start_or_right);
                stack.push(node_index + 1);
            }
        }
        false
    }

    /// Maximum tree depth, for the statistics panel.
    #[must_use]
    pub fn depth(&self) -> u32 {
        if self.nodes.is_empty() {
            return 0;
        }
        fn walk(bvh: &TriangleBvh, index: u32) -> u32 {
            let n = bvh.nodes[index as usize];
            if n.count > 0 {
                1
            } else {
                1 + walk(bvh, index + 1).max(walk(bvh, n.start_or_right))
            }
        }
        walk(self, 0)
    }

    /// Sum of node surface areas over the root's. Lower is better.
    #[must_use]
    pub fn quality(&self) -> f32 {
        let root = self.bounds().surface_area();
        if root <= 0.0 {
            return 0.0;
        }
        self.nodes
            .iter()
            .map(|n| n.bounds.surface_area())
            .sum::<f32>()
            / root
    }

    /// Bytes of heap used.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.nodes.capacity() * core::mem::size_of::<Node>() + self.tri_indices.capacity() * 4
    }
}

/// Recursively builds a subtree over `indices[..]`, returning the created node's
/// index.
fn build_node(
    triangles: &[WorldTriangle],
    indices: &mut [u32],
    offset: usize,
    nodes: &mut Vec<Node>,
) -> u32 {
    let mut bounds = Aabb::EMPTY;
    let mut centroids = Aabb::EMPTY;
    for &i in indices.iter() {
        if let Some(t) = triangles.get(i as usize) {
            bounds.grow_aabb(&t.bounds());
            centroids.grow(t.centroid());
        }
    }
    let index = nodes.len() as u32;
    nodes.push(Node {
        bounds,
        start_or_right: offset as u32,
        count: indices.len() as u32,
    });
    if indices.len() <= LEAF_SIZE {
        return index;
    }

    // Split at the median of the centroid distribution along the widest axis.
    let axis = centroids.longest_axis();
    if centroids.size()[axis] <= 1e-7 {
        return index; // all centroids coincide; splitting cannot make progress
    }
    indices.sort_unstable_by(|a, b| {
        let ca = triangles[*a as usize].centroid()[axis];
        let cb = triangles[*b as usize].centroid()[axis];
        ca.partial_cmp(&cb).unwrap_or(core::cmp::Ordering::Equal)
    });
    let mid = indices.len() / 2;
    let (left, right) = indices.split_at_mut(mid);
    build_node(triangles, left, offset, nodes);
    let right_index = build_node(triangles, right, offset + mid, nodes);
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
    use noxel_core::math::Vec2;

    fn tri(a: Vec3, b: Vec3, c: Vec3) -> WorldTriangle {
        WorldTriangle::new(
            a,
            b,
            c,
            Vec3::Y,
            Vec3::Y,
            Vec3::Y,
            Vec2::ZERO,
            Vec2::X,
            Vec2::Y,
            0,
            0,
        )
    }

    fn grid() -> (Vec<WorldTriangle>, TriangleBvh) {
        let mut tris = Vec::new();
        for z in 0..10 {
            for x in 0..10 {
                let (fx, fz) = (x as f32 * 2.0, z as f32 * 2.0);
                tris.push(tri(
                    Vec3::new(fx, 0.0, fz),
                    Vec3::new(fx + 1.0, 0.0, fz),
                    Vec3::new(fx, 1.0, fz + 1.0),
                ));
            }
        }
        let bvh = TriangleBvh::build(&tris);
        (tris, bvh)
    }

    #[test]
    fn empty_tree_is_safe() {
        let bvh = TriangleBvh::build(&[]);
        assert!(bvh.is_empty());
        assert_eq!(bvh.depth(), 0);
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN);
        assert!(bvh.intersect(&[], &ray).is_none());
        assert!(!bvh.any_hit(&[], &ray));
    }

    #[test]
    fn single_triangle_hit() {
        let tris = vec![tri(Vec3::ZERO, Vec3::X, Vec3::Z)];
        let bvh = TriangleBvh::build(&tris);
        let ray = Ray::new(Vec3::new(0.2, 1.0, 0.2), Vec3::DOWN);
        let (index, t, u, v) = bvh.intersect(&tris, &ray).expect("must hit");
        assert_eq!(index, 0);
        assert!((t - 1.0).abs() < 1e-5);
        assert!(u >= 0.0 && v >= 0.0 && u + v <= 1.0);
    }

    #[test]
    fn miss_returns_none() {
        let tris = vec![tri(Vec3::ZERO, Vec3::X, Vec3::Z)];
        let bvh = TriangleBvh::build(&tris);
        let ray = Ray::new(Vec3::new(5.0, 1.0, 5.0), Vec3::DOWN);
        assert!(bvh.intersect(&tris, &ray).is_none());
        assert!(!bvh.any_hit(&tris, &ray));
    }

    #[test]
    fn nearest_of_many_is_returned() {
        // Two triangles stacked along Y; a downward ray must hit the upper one.
        let tris = vec![
            tri(Vec3::ZERO, Vec3::X, Vec3::Z),
            tri(
                Vec3::new(0.0, 5.0, 0.0),
                Vec3::new(1.0, 5.0, 0.0),
                Vec3::new(0.0, 5.0, 1.0),
            ),
        ];
        let bvh = TriangleBvh::build(&tris);
        let ray = Ray::new(Vec3::new(0.2, 10.0, 0.2), Vec3::DOWN);
        let (index, t, _, _) = bvh.intersect(&tris, &ray).unwrap();
        assert_eq!(index, 1, "the upper triangle is nearer");
        assert!((t - 5.0).abs() < 1e-4);
    }

    #[test]
    fn respects_max_t() {
        let tris = vec![tri(Vec3::ZERO, Vec3::X, Vec3::Z)];
        let bvh = TriangleBvh::build(&tris);
        let ray = Ray::with_max_t(Vec3::new(0.2, 10.0, 0.2), Vec3::DOWN, 5.0);
        assert!(bvh.intersect(&tris, &ray).is_none());
        assert!(!bvh.any_hit(&tris, &ray));
    }

    #[test]
    fn grid_traversal_matches_brute_force() {
        let (tris, bvh) = grid();
        let mut hits = 0;
        for i in 0..500 {
            let x = (i % 25) as f32 * 0.8;
            let z = (i / 25) as f32 * 0.8;
            let ray = Ray::new(Vec3::new(x, 10.0, z), Vec3::DOWN);
            let fast = bvh.intersect(&tris, &ray);
            let mut slow: Option<(u32, f32, f32, f32)> = None;
            for (index, t) in tris.iter().enumerate() {
                if let Some((tt, u, v)) = t.intersect(&ray) {
                    if slow.is_none_or(|(_, bt, _, _)| tt < bt) {
                        slow = Some((index as u32, tt, u, v));
                    }
                }
            }
            match (fast, slow) {
                (Some((fi, ft, _, _)), Some((si, st, _, _))) => {
                    assert_eq!(fi, si, "different triangle at ({x},{z})");
                    assert!((ft - st).abs() < 1e-4, "different distance at ({x},{z})");
                    hits += 1;
                }
                (None, None) => {}
                (a, b) => panic!("disagreement at ({x},{z}): {a:?} vs {b:?}"),
            }
        }
        assert!(hits > 100, "the test must actually hit things: {hits}");
    }

    #[test]
    fn any_hit_agrees_with_intersect() {
        let (tris, bvh) = grid();
        for i in 0..200 {
            let x = (i % 20) as f32 * 1.0;
            let z = (i / 20) as f32 * 1.0;
            let ray = Ray::new(Vec3::new(x, 10.0, z), Vec3::DOWN);
            assert_eq!(
                bvh.any_hit(&tris, &ray),
                bvh.intersect(&tris, &ray).is_some()
            );
        }
    }

    #[test]
    fn tree_is_balanced() {
        let (_, bvh) = grid();
        assert!(bvh.depth() <= 12, "depth {}", bvh.depth());
        assert!(bvh.quality() < 40.0, "quality {}", bvh.quality());
    }

    #[test]
    fn all_triangles_are_stored_once() {
        let (tris, bvh) = grid();
        let mut indices = bvh.ordered_indices().to_vec();
        indices.sort_unstable();
        assert_eq!(indices, (0..tris.len() as u32).collect::<Vec<_>>());
    }

    #[test]
    fn two_sided_intersection() {
        let tris = vec![tri(Vec3::ZERO, Vec3::X, Vec3::Z)];
        let bvh = TriangleBvh::build(&tris);
        // From below: the triangle faces +Y but must still be hit.
        let ray = Ray::new(Vec3::new(0.2, -1.0, 0.2), Vec3::UP);
        assert!(bvh.intersect(&tris, &ray).is_some());
    }

    #[test]
    fn degenerate_identical_triangles_still_build() {
        let t = tri(Vec3::ZERO, Vec3::X, Vec3::Z);
        let tris = vec![t; 100];
        let bvh = TriangleBvh::build(&tris);
        assert_eq!(bvh.ordered_indices().len(), 100);
        let ray = Ray::new(Vec3::new(0.2, 1.0, 0.2), Vec3::DOWN);
        assert!(bvh.intersect(&tris, &ray).is_some());
    }

    #[test]
    fn triangle_helpers() {
        let t = tri(Vec3::ZERO, Vec3::X, Vec3::Z);
        assert!(
            t.geometric_normal.approx_eq(Vec3::Y, 1e-5)
                || t.geometric_normal
                    .approx_eq(Vec3::new(0.0, -1.0, 0.0), 1e-5)
        );
        assert!((t.area() - 0.5).abs() < 1e-5);
        assert!(t.point_at(0.0, 0.0).approx_eq(Vec3::ZERO, 1e-5));
        assert!(t.point_at(1.0, 0.0).approx_eq(Vec3::X, 1e-5));
        assert!(t.point_at(0.0, 1.0).approx_eq(Vec3::Z, 1e-5));
        assert!(t.normal_at(0.3, 0.3).y.abs() > 0.9);
    }

    #[test]
    fn memory_is_reported() {
        let (_, bvh) = grid();
        assert!(bvh.memory_bytes() > 0);
        assert!(bvh.node_count() > 1);
    }
}
