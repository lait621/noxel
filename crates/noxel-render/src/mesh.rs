//! Meshes: vertices, procedural primitives, and the triangle soup the renderers
//! consume.
//!
//! Noxel has no model importer by design (see
//! `docs/adr/0006-asset-pipeline.md`): a pixel-art top-down RPG is built from
//! boxes, quads and billboards, and every one of those is a few lines of
//! arithmetic. Shipping a glTF loader would add thousands of lines and a
//! dependency tree for geometry the world generator already synthesises.
//!
//! Contributors who need imported art should add a loader in `noxel-asset`
//! (which owns file formats) and produce a [`Mesh`] here.

use noxel_core::math::{Aabb, Vec2, Vec3};

/// One vertex: position, normal and texture coordinate.
///
/// `#[repr(C)]` so a GPU backend can upload it verbatim.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    /// Object-space position, metres.
    pub position: Vec3,
    /// Object-space unit normal.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
}

impl Vertex {
    /// Constructs a vertex.
    #[inline]
    #[must_use]
    pub const fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position,
            normal,
            uv,
        }
    }

    /// A vertex with an up normal and zero UV.
    #[inline]
    #[must_use]
    pub const fn from_position(position: Vec3) -> Self {
        Self {
            position,
            normal: Vec3::Y,
            uv: Vec2::ZERO,
        }
    }
}

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// Optional name, for the debug overlay and statistics.
    pub name: String,
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangle indices, three per face.
    pub indices: Vec<u32>,
    /// Object-space bounds, maintained by the constructors.
    pub bounds: Aabb,
    /// True when the geometry should be treated as flat-shaded (the
    /// rasterizer's per-face normals are then used). Pixel-art ground planes and
    /// blocky buildings look better this way.
    pub flat_shaded: bool,
}

impl Mesh {
    /// Builds a mesh from vertices and indices, computing the bounds.
    ///
    /// # Panics
    ///
    /// Panics when an index is out of range. A malformed mesh is a build bug;
    /// failing loudly at construction beats a garbage triangle later.
    #[must_use]
    pub fn new(name: impl Into<String>, vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        for &i in &indices {
            assert!(
                (i as usize) < vertices.len(),
                "mesh `{}` has index {i} but only {} vertices",
                name.into(),
                vertices.len()
            );
        }
        let mut bounds = Aabb::EMPTY;
        for v in &vertices {
            bounds.grow(v.position);
        }
        Self {
            name: name.into(),
            vertices,
            indices,
            bounds,
            flat_shaded: false,
        }
    }

    /// Marks the mesh as flat-shaded.
    #[must_use]
    pub fn with_flat_shading(mut self) -> Self {
        self.flat_shaded = true;
        self
    }

    /// Number of triangles.
    #[inline]
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// True when there is nothing to draw.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Number of vertices.
    #[inline]
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Recomputes [`Mesh::bounds`].
    pub fn recompute_bounds(&mut self) {
        let mut bounds = Aabb::EMPTY;
        for v in &self.vertices {
            bounds.grow(v.position);
        }
        self.bounds = bounds;
    }

    /// The three vertices of triangle `index`.
    ///
    /// # Panics
    ///
    /// Panics when `index` is past the end. The renderers iterate
    /// `0..triangle_count()`.
    #[inline]
    #[must_use]
    pub fn triangle(&self, index: usize) -> [&Vertex; 3] {
        let base = index * 3;
        [
            &self.vertices[self.indices[base] as usize],
            &self.vertices[self.indices[base + 1] as usize],
            &self.vertices[self.indices[base + 2] as usize],
        ]
    }

    /// The geometric normal of triangle `index` (not interpolated).
    #[must_use]
    pub fn triangle_normal(&self, index: usize) -> Vec3 {
        let [a, b, c] = self.triangle(index);
        (b.position - a.position)
            .cross(c.position - a.position)
            .normalize_or_zero()
    }

    /// The area of triangle `index`.
    #[must_use]
    pub fn triangle_area(&self, index: usize) -> f32 {
        let [a, b, c] = self.triangle(index);
        (b.position - a.position)
            .cross(c.position - a.position)
            .length()
            * 0.5
    }

    /// Total surface area.
    #[must_use]
    pub fn surface_area(&self) -> f32 {
        (0..self.triangle_count())
            .map(|i| self.triangle_area(i))
            .sum()
    }

    /// Interpolated surface normal at a barycentric position of triangle
    /// `index`, or the geometric normal for flat-shaded meshes.
    #[must_use]
    pub fn shading_normal(&self, index: usize, u: f32, v: f32) -> Vec3 {
        if self.flat_shaded {
            return self.triangle_normal(index);
        }
        let [a, b, c] = self.triangle(index);
        let w = 1.0 - u - v;
        (a.normal * w + b.normal * u + c.normal * v).normalize_or_zero()
    }

    /// Appends `other`, offsetting its indices. Used by the batching code.
    pub fn append(&mut self, other: &Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
        self.bounds.grow_aabb(&other.bounds);
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.vertices.capacity() * core::mem::size_of::<Vertex>() + self.indices.capacity() * 4
    }

    // ------------------------------------------------------- primitives

    /// A horizontal quad on the XZ plane, centred at the origin, facing `+Y`.
    ///
    /// The ground tile. `size` is the full edge length; `uv_tiles` repeats the
    /// texture across it.
    #[must_use]
    pub fn plane(size: f32, uv_tiles: f32) -> Self {
        let h = size * 0.5;
        let n = Vec3::Y;
        let vertices = vec![
            Vertex::new(Vec3::new(-h, 0.0, -h), n, Vec2::new(0.0, 0.0)),
            Vertex::new(Vec3::new(h, 0.0, -h), n, Vec2::new(uv_tiles, 0.0)),
            Vertex::new(Vec3::new(h, 0.0, h), n, Vec2::new(uv_tiles, uv_tiles)),
            Vertex::new(Vec3::new(-h, 0.0, h), n, Vec2::new(0.0, uv_tiles)),
        ];
        // Counter-clockwise seen from above (+Y) so the front face points up.
        Self::new("plane", vertices, vec![0, 2, 1, 0, 3, 2])
    }

    /// A subdivided horizontal ground patch, for terrain with per-vertex
    /// height. `subdivisions` cells per axis.
    #[must_use]
    pub fn grid(size: f32, subdivisions: u32) -> Self {
        let n = subdivisions.max(1);
        let step = size / n as f32;
        let h = size * 0.5;
        let mut vertices = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
        for z in 0..=n {
            for x in 0..=n {
                let px = -h + x as f32 * step;
                let pz = -h + z as f32 * step;
                vertices.push(Vertex::new(
                    Vec3::new(px, 0.0, pz),
                    Vec3::Y,
                    Vec2::new(x as f32 / n as f32, z as f32 / n as f32),
                ));
            }
        }
        let mut indices = Vec::with_capacity((n * n * 6) as usize);
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                let right = i + 1;
                let down = i + n + 1;
                let diag = down + 1;
                indices.extend_from_slice(&[i, diag, right, i, down, diag]);
            }
        }
        Self::new("grid", vertices, indices)
    }

    /// A vertical quad on the XY plane, centred at the origin, facing `+Z`.
    ///
    /// The sprite/billboard primitive. `size` is the full width and height; the
    /// UV origin is the top-left so it matches image coordinates.
    #[must_use]
    pub fn quad(size: Vec2) -> Self {
        let (hw, hh) = (size.x * 0.5, size.y * 0.5);
        let n = Vec3::Z;
        let vertices = vec![
            Vertex::new(Vec3::new(-hw, hh, 0.0), n, Vec2::new(0.0, 0.0)),
            Vertex::new(Vec3::new(hw, hh, 0.0), n, Vec2::new(1.0, 0.0)),
            Vertex::new(Vec3::new(hw, -hh, 0.0), n, Vec2::new(1.0, 1.0)),
            Vertex::new(Vec3::new(-hw, -hh, 0.0), n, Vec2::new(0.0, 1.0)),
        ];
        Self::new("quad", vertices, vec![0, 1, 2, 0, 2, 3])
    }

    /// A box centred at the origin. `half_extents` are half the side lengths.
    ///
    /// Faces are wound counter-clockwise when seen from outside.
    #[must_use]
    pub fn cuboid(half_extents: Vec3) -> Self {
        let h = half_extents.abs();
        // Each face is given a basis (u, v) chosen so that `u x v == n`. That
        // single invariant makes the shared winding order `[0,1,2],[0,2,3]`
        // produce an outward-facing triangle on all six sides.
        let faces = [
            (Vec3::Y, Vec3::Z, Vec3::X),                   // top
            (Vec3::new(0.0, -1.0, 0.0), Vec3::X, Vec3::Z), // bottom
            (Vec3::X, Vec3::new(0.0, 0.0, -1.0), Vec3::Y), // +X (right)
            (Vec3::new(-1.0, 0.0, 0.0), Vec3::Z, Vec3::Y), // -X (left)
            (Vec3::Z, Vec3::X, Vec3::Y),                   // +Z (back)
            (
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(-1.0, 0.0, 0.0),
                Vec3::Y,
            ), // -Z (front)
        ];
        let mut vertices = Vec::with_capacity(24);
        for (n, u, v) in faces {
            let c = n * h;
            let du = u * h.dot(u.abs());
            let dv = v * h.dot(v.abs());
            vertices.push(Vertex::new(c - du - dv, n, Vec2::new(0.0, 1.0)));
            vertices.push(Vertex::new(c + du - dv, n, Vec2::new(1.0, 1.0)));
            vertices.push(Vertex::new(c + du + dv, n, Vec2::new(1.0, 0.0)));
            vertices.push(Vertex::new(c - du + dv, n, Vec2::new(0.0, 0.0)));
        }
        let mut indices = Vec::with_capacity(36);
        for f in 0..6u32 {
            let b = f * 4;
            indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        Self::new("cuboid", vertices, indices)
    }

    /// A cube with the given edge length.
    #[must_use]
    pub fn cube(size: f32) -> Self {
        Self::cuboid(Vec3::splat(size * 0.5))
    }

    /// A box built to span two corners (world- or object-space).
    #[must_use]
    pub fn from_aabb(bounds: Aabb) -> Self {
        let center = bounds.center();
        let mut mesh = Self::cuboid(bounds.half_extents());
        for v in &mut mesh.vertices {
            v.position += center;
        }
        mesh.bounds = bounds;
        mesh.name = String::from("from_aabb");
        mesh
    }

    /// A Y-aligned cylinder.
    #[must_use]
    pub fn cylinder(radius: f32, height: f32, segments: u32) -> Self {
        let seg = segments.max(3);
        let hh = height * 0.5;
        let mut vertices = Vec::with_capacity((seg * 4 + 2) as usize);
        // Side wall: two rings, with normals pointing outward.
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * noxel_core::math::TAU;
            let (s, c) = a.sin_cos();
            let n = Vec3::new(c, 0.0, s);
            let u = i as f32 / seg as f32;
            vertices.push(Vertex::new(
                Vec3::new(c * radius, hh, s * radius),
                n,
                Vec2::new(u, 0.0),
            ));
            vertices.push(Vertex::new(
                Vec3::new(c * radius, -hh, s * radius),
                n,
                Vec2::new(u, 1.0),
            ));
        }
        let mut indices = Vec::with_capacity((seg * 12) as usize);
        // Ring order is (top_i, bottom_i, top_i+1, bottom_i+1) starting at `b`,
        // so these two triangles wind outward.
        for i in 0..seg {
            let b = i * 2;
            indices.extend_from_slice(&[b, b + 3, b + 1, b, b + 2, b + 3]);
        }
        // Caps.
        let top_center = vertices.len() as u32;
        vertices.push(Vertex::new(
            Vec3::new(0.0, hh, 0.0),
            Vec3::Y,
            Vec2::new(0.5, 0.5),
        ));
        let top_ring = vertices.len() as u32;
        for i in 0..seg {
            let a = i as f32 / seg as f32 * noxel_core::math::TAU;
            let (s, c) = a.sin_cos();
            vertices.push(Vertex::new(
                Vec3::new(c * radius, hh, s * radius),
                Vec3::Y,
                Vec2::new(c * 0.5 + 0.5, s * 0.5 + 0.5),
            ));
        }
        for i in 0..seg {
            let a = top_ring + i;
            let b = top_ring + (i + 1) % seg;
            // Reversed relative to the bottom cap so the normal is +Y.
            indices.extend_from_slice(&[top_center, b, a]);
        }
        let bottom_center = vertices.len() as u32;
        vertices.push(Vertex::new(
            Vec3::new(0.0, -hh, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec2::new(0.5, 0.5),
        ));
        let bottom_ring = vertices.len() as u32;
        for i in 0..seg {
            let a = i as f32 / seg as f32 * noxel_core::math::TAU;
            let (s, c) = a.sin_cos();
            vertices.push(Vertex::new(
                Vec3::new(c * radius, -hh, s * radius),
                Vec3::new(0.0, -1.0, 0.0),
                Vec2::new(c * 0.5 + 0.5, s * 0.5 + 0.5),
            ));
        }
        for i in 0..seg {
            let a = bottom_ring + i;
            let b = bottom_ring + (i + 1) % seg;
            indices.extend_from_slice(&[bottom_center, a, b]);
        }
        Self::new("cylinder", vertices, indices)
    }

    /// A UV sphere.
    #[must_use]
    pub fn sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let seg = segments.max(3);
        let rng = rings.max(2);
        let mut vertices = Vec::with_capacity(((seg + 1) * (rng + 1)) as usize);
        for r in 0..=rng {
            let phi = r as f32 / rng as f32 * core::f32::consts::PI;
            let (sp, cp) = phi.sin_cos();
            for s in 0..=seg {
                let theta = s as f32 / seg as f32 * noxel_core::math::TAU;
                let (st, ct) = theta.sin_cos();
                let n = Vec3::new(sp * ct, cp, sp * st);
                vertices.push(Vertex::new(
                    n * radius,
                    n,
                    Vec2::new(s as f32 / seg as f32, r as f32 / rng as f32),
                ));
            }
        }
        let mut indices = Vec::with_capacity((seg * rng * 6) as usize);
        let row = seg + 1;
        for r in 0..rng {
            for s in 0..seg {
                let a = r * row + s;
                let b = a + row;
                // (a, a+1, b) winds outward: rows advance from the +Y pole to
                // the -Y pole and columns advance counter-clockwise about +Y.
                indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        Self::new("sphere", vertices, indices)
    }

    /// A camera-facing billboard quad of the given world size.
    ///
    /// Sprites in a top-down pixel-art game are conventionally **flat on the
    /// ground** (`up = +Y`) rather than screen-facing: a character drawn on a
    /// quad lying in the XZ plane keeps its pixel grid aligned with the tile
    /// grid, which is what makes the art look crisp. Pass `Vec3::Y` for that
    /// look, or a camera-derived up for a true screen-facing sprite.
    #[must_use]
    pub fn billboard(width: f32, height: f32, up: Vec3, right: Vec3) -> Self {
        let hw = width * 0.5;
        let hh = height * 0.5;
        let n = right.cross(up).normalize_or_zero();
        let r = right.normalize_or_zero() * hw;
        let u = up.normalize_or_zero() * hh;
        let vertices = vec![
            Vertex::new(-r + u, n, Vec2::new(0.0, 0.0)),
            Vertex::new(r + u, n, Vec2::new(1.0, 0.0)),
            Vertex::new(r - u, n, Vec2::new(1.0, 1.0)),
            Vertex::new(-r - u, n, Vec2::new(0.0, 1.0)),
        ];
        Self::new("billboard", vertices, vec![0, 1, 2, 0, 2, 3])
    }

    /// The `n`-gon outline of a regular polygon on the XZ plane, extruded
    /// upwards. Handy for towers and tree trunks.
    #[must_use]
    pub fn prism(sides: u32, radius: f32, height: f32) -> Self {
        Self::cylinder(radius, height, sides)
    }

    /// A four-sided pyramid (roof shape).
    #[must_use]
    pub fn pyramid(base: f32, height: f32) -> Self {
        let h = base * 0.5;
        let apex = Vec3::new(0.0, height, 0.0);
        let corners = [
            Vec3::new(-h, 0.0, -h),
            Vec3::new(h, 0.0, -h),
            Vec3::new(h, 0.0, h),
            Vec3::new(-h, 0.0, h),
        ];
        let mut vertices = Vec::new();
        for i in 0..4 {
            let a = corners[i];
            let b = corners[(i + 1) % 4];
            // Outward face normal; (a, apex, b) is the matching winding.
            let n = (apex - a).cross(b - a).normalize_or_zero();
            vertices.push(Vertex::new(a, n, Vec2::new(0.0, 1.0)));
            vertices.push(Vertex::new(apex, n, Vec2::new(0.5, 0.0)));
            vertices.push(Vertex::new(b, n, Vec2::new(1.0, 1.0)));
        }
        let base_start = vertices.len() as u32;
        for c in corners {
            vertices.push(Vertex::new(
                c,
                Vec3::new(0.0, -1.0, 0.0),
                Vec2::new(c.x * 0.5 + 0.5, c.z * 0.5 + 0.5),
            ));
        }
        let mut indices: Vec<u32> = (0..4u32)
            .flat_map(|i| [i * 3, i * 3 + 1, i * 3 + 2])
            .collect();
        indices.extend_from_slice(&[
            base_start,
            base_start + 1,
            base_start + 2,
            base_start,
            base_start + 2,
            base_start + 3,
        ]);
        Self::new("pyramid", vertices, indices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_has_expected_shape() {
        let m = Mesh::plane(2.0, 1.0);
        assert_eq!(m.triangle_count(), 2);
        assert_eq!(m.vertex_count(), 4);
        assert_eq!(m.bounds.size(), Vec3::new(2.0, 0.0, 2.0));
    }

    #[test]
    fn plane_front_face_points_up() {
        let m = Mesh::plane(1.0, 1.0);
        // With counter-clockwise winding seen from +Y, the geometric normal of
        // every triangle must point up.
        for i in 0..m.triangle_count() {
            assert!(
                m.triangle_normal(i).y > 0.9,
                "tri {i} normal {:?}",
                m.triangle_normal(i)
            );
        }
    }

    #[test]
    fn grid_is_subdivided() {
        let m = Mesh::grid(4.0, 4);
        assert_eq!(m.triangle_count(), 4 * 4 * 2);
        assert_eq!(m.vertex_count(), 25);
        assert_eq!(m.bounds.size(), Vec3::new(4.0, 0.0, 4.0));
        assert!(m.bounds.min.approx_eq(Vec3::new(-2.0, 0.0, -2.0), 1e-5));
    }

    #[test]
    fn cuboid_has_six_faces() {
        let m = Mesh::cuboid(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(m.triangle_count(), 12);
        assert_eq!(m.vertex_count(), 24);
        assert_eq!(m.bounds.size(), Vec3::new(2.0, 4.0, 6.0));
        assert!((m.surface_area() - 2.0 * (2.0 * 4.0 + 4.0 * 6.0 + 2.0 * 6.0)).abs() < 1e-3);
    }

    #[test]
    fn cuboid_faces_wind_outwards() {
        let m = Mesh::cuboid(Vec3::splat(1.0));
        // Every face normal must point away from the centre.
        for i in 0..m.triangle_count() {
            let n = m.triangle_normal(i);
            let center = m.triangle(i).iter().fold(Vec3::ZERO, |a, v| a + v.position) / 3.0;
            assert!(n.dot(center) > 0.0, "tri {i}: normal {n:?} points inward");
        }
    }

    #[test]
    fn cube_is_a_uniform_cuboid() {
        let m = Mesh::cube(2.0);
        assert_eq!(m.bounds.size(), Vec3::splat(2.0));
    }

    #[test]
    fn from_aabb_places_the_box() {
        let b = Aabb::new(Vec3::new(4.0, 1.0, -2.0), Vec3::new(6.0, 3.0, 0.0));
        let m = Mesh::from_aabb(b);
        assert_eq!(m.bounds, b);
        // The geometry must actually sit where the bounds say.
        let mut geo = Aabb::EMPTY;
        for v in &m.vertices {
            geo.grow(v.position);
        }
        assert!(geo.min.approx_eq(b.min, 1e-5));
        assert!(geo.max.approx_eq(b.max, 1e-5));
    }

    #[test]
    fn quad_uv_origin_is_top_left() {
        let m = Mesh::quad(Vec2::new(2.0, 1.0));
        let top_left = m.vertices[0];
        assert_eq!(top_left.uv, Vec2::ZERO);
        assert!(top_left.position.y > 0.0, "first vertex is the top edge");
    }

    #[test]
    fn sphere_radius_and_normals() {
        let m = Mesh::sphere(2.0, 16, 8);
        assert!(!m.is_empty());
        for v in &m.vertices {
            assert!((v.position.length() - 2.0).abs() < 1e-4);
            assert!((v.normal.length() - 1.0).abs() < 1e-4);
            assert!(v.normal.dot(v.position) > 0.0, "normal must point outwards");
        }
        assert!(m.bounds.max.x <= 2.001 && m.bounds.min.x >= -2.001);
    }

    #[test]
    fn cylinder_bounds_and_caps() {
        let m = Mesh::cylinder(1.0, 4.0, 12);
        assert_eq!(m.bounds.size().y, 4.0);
        assert!((m.bounds.size().x - 2.0).abs() < 1e-3);
        assert!(m.triangle_count() >= 12 * 4);
    }

    #[test]
    fn cylinder_faces_wind_outwards() {
        let m = Mesh::cylinder(1.0, 4.0, 16);
        for i in 0..m.triangle_count() {
            let n = m.triangle_normal(i);
            let centroid = m.triangle(i).iter().fold(Vec3::ZERO, |a, v| a + v.position) / 3.0;
            // For a convex solid centred on the origin, an outward normal must
            // have a positive dot product with the centroid direction.
            assert!(
                n.dot(centroid) > 0.0,
                "tri {i}: n={n:?} centroid={centroid:?}"
            );
        }
    }

    #[test]
    fn sphere_faces_wind_outwards() {
        let m = Mesh::sphere(2.0, 12, 8);
        for i in 0..m.triangle_count() {
            // A UV sphere has a degenerate triangle at each pole (the whole top
            // ring collapses to one point); skip those.
            if m.triangle_area(i) < 1e-6 {
                continue;
            }
            let n = m.triangle_normal(i);
            let centroid = m.triangle(i).iter().fold(Vec3::ZERO, |a, v| a + v.position) / 3.0;
            assert!(
                n.dot(centroid) > 0.0,
                "tri {i}: n={n:?} centroid={centroid:?}"
            );
        }
    }

    #[test]
    fn pyramid_faces_wind_outwards() {
        let m = Mesh::pyramid(2.0, 2.0);
        let center = Vec3::new(0.0, 2.0 / 3.0, 0.0);
        for i in 0..m.triangle_count() {
            let n = m.triangle_normal(i);
            let centroid = m.triangle(i).iter().fold(Vec3::ZERO, |a, v| a + v.position) / 3.0;
            assert!(
                n.dot(centroid - center) > 0.0,
                "tri {i}: n={n:?} c={centroid:?}"
            );
        }
    }

    #[test]
    fn prism_degenerates_gracefully() {
        // Fewer than 3 sides must clamp rather than produce a degenerate mesh.
        let m = Mesh::prism(1, 1.0, 1.0);
        assert!(!m.is_empty());
    }

    #[test]
    fn pyramid_bounds() {
        let m = Mesh::pyramid(2.0, 3.0);
        assert_eq!(m.bounds.size(), Vec3::new(2.0, 3.0, 2.0));
        assert!(m.triangle_count() >= 6, "4 sides + base");
    }

    #[test]
    fn billboard_uses_the_supplied_basis() {
        let m = Mesh::billboard(2.0, 1.0, Vec3::Y, Vec3::X);
        assert_eq!(m.triangle_count(), 2);
        assert_eq!(m.bounds.size().x, 2.0);
        assert_eq!(m.bounds.size().y, 1.0);
    }

    #[test]
    fn shading_normal_blends_and_flat_shading_overrides() {
        let mut m = Mesh::plane(1.0, 1.0);
        let n = m.shading_normal(0, 0.3, 0.3);
        assert!(n.y > 0.9);
        m.flat_shaded = true;
        let flat = m.shading_normal(0, 0.0, 0.0);
        assert!(flat.y > 0.9);
    }

    #[test]
    fn append_offsets_indices() {
        let mut a = Mesh::cube(1.0);
        let before = a.triangle_count();
        a.append(&Mesh::plane(1.0, 1.0));
        assert_eq!(a.triangle_count(), before + 2);
        assert!(a.indices.iter().all(|i| (*i as usize) < a.vertices.len()));
    }

    #[test]
    fn empty_mesh_is_safe() {
        let m = Mesh::default();
        assert!(m.is_empty());
        assert_eq!(m.triangle_count(), 0);
        assert_eq!(m.surface_area(), 0.0);
        assert!(m.bounds.is_empty());
    }

    #[test]
    #[should_panic(expected = "has index")]
    fn bad_index_panics_at_construction() {
        let _ = Mesh::new(
            "bad",
            vec![Vertex::from_position(Vec3::ZERO)],
            vec![0, 1, 2],
        );
    }

    #[test]
    fn recompute_bounds_matches_geometry() {
        let mut m = Mesh::cuboid(Vec3::splat(1.0));
        m.bounds = Aabb::EMPTY;
        m.recompute_bounds();
        assert_eq!(m.bounds.size(), Vec3::splat(2.0));
    }
}
