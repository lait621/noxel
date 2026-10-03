//! Buildings and props drawn from the streamed chunks.
//!
//! The world generator hands each chunk a list of buildings and a list of props,
//! already merged into a handful of boxes. This module turns them into geometry:
//!
//! | Layer | Geometry | Material |
//! |---|---|---|
//! | ground | one quad per tile | unlit, terrain atlas |
//! | buildings | one box per building, roof quad on top | lit, building atlas |
//! | props | a cone for a tree, a box for a rock | lit, flat colour |
//!
//! Buildings and props are **lit** while the ground is **unlit**. That is
//! deliberate: a tile's colour is the artist's choice and must survive to the
//! screen, while a building and a tree benefit enormously from the sun's
//! direction and from the ray-traced ambient occlusion in the hybrid mode. It is
//! also what makes a village read as three-dimensional from directly above.
//!
//! Everything for a chunk goes into two meshes — one for the buildings, one for
//! the props — so a chunk is three draw calls, not three hundred.

use noxel_asset::format::TileSet;
use noxel_core::math::{Vec2, Vec3};
use noxel_render::mesh::{Mesh, Vertex};
use noxel_world::chunk::{Chunk, PropKind};

use crate::terrain::uv_rect;

/// The prop atlas cell for each kind, as `[x, y, w, h]` in pixels.
///
/// The atlas is a single row laid out left to right; see
/// `examples/town-demo/assets/README.md` for the table this mirrors.
#[must_use]
pub fn prop_cell(kind: PropKind, variant: u32) -> [u32; 4] {
    match kind {
        PropKind::Tree => {
            // Three canopy frames, picked deterministically so a forest is not
            // all one shape.
            let frame = variant % 3;
            [frame * 24, 0, 24, 24]
        }
        PropKind::Rock => {
            if variant % 3 == 0 {
                [120, 8, 16, 16]
            } else {
                [104, 8, 16, 16]
            }
        }
        PropKind::Bush => [88, 8, 16, 16],
        PropKind::Post => [200, 8, 16, 16],
        PropKind::Other => [232, 8, 16, 16],
    }
}

/// The height, in metres, of a prop's visual centre above its base.
///
/// Props are drawn as flat quads facing the camera, which for a top-down camera
/// means a horizontal quad at the top of the prop. That is exactly what a tree
/// looks like from above, and it costs two triangles instead of a canopy mesh.
#[must_use]
pub fn prop_height(kind: PropKind) -> f32 {
    match kind {
        PropKind::Tree => 3.4,
        PropKind::Rock => 0.5,
        PropKind::Bush => 0.6,
        PropKind::Post => 1.6,
        PropKind::Other => 0.5,
    }
}

/// The on-screen size, in metres, of a prop's billboard.
#[must_use]
pub fn prop_size(kind: PropKind) -> f32 {
    match kind {
        PropKind::Tree => 3.6,
        PropKind::Rock => 1.1,
        PropKind::Bush => 1.2,
        PropKind::Post => 0.7,
        PropKind::Other => 1.0,
    }
}

/// Builds the render mesh for a chunk's buildings.
///
/// Each building is a box from its world bounds with a roof quad on top. The
/// bounds are exact (every facing is a multiple of 90 degrees), so no rotation
/// is needed here; the roof is inset by a few centimetres so the sun catches a
/// visible lip.
#[must_use]
pub fn build_building_mesh(chunk: &Chunk, set: &TileSet, atlas: (u32, u32)) -> Mesh {
    let mut mesh = Mesh {
        name: format!("buildings_{}_{}", chunk.pos.x, chunk.pos.y),
        ..Mesh::default()
    };
    let wall = set
        .tile_by_name("wall_plaster")
        .or_else(|| set.tile_by_name("wall_stone"))
        .map(|t| uv_rect(t, atlas));
    let roof = set
        .tile_by_name("roof_red")
        .or_else(|| set.tile_by_name("roof_slate"))
        .map(|t| uv_rect(t, atlas));

    for building in &chunk.buildings {
        let bounds = building.bounds;
        if bounds.is_empty() {
            continue;
        }
        append_box(&mut mesh, &bounds, wall, roof);
    }
    mesh.recompute_bounds();
    mesh
}

/// Builds the render mesh for a chunk's props.
#[must_use]
pub fn build_prop_mesh(chunk: &Chunk, atlas: (u32, u32)) -> Mesh {
    let mut mesh = Mesh {
        name: format!("props_{}_{}", chunk.pos.x, chunk.pos.y),
        ..Mesh::default()
    };
    // The prop atlas has its own cell layout; the generator documents 24x24
    // canopy cells in the top row and 16x16 props below it.
    let (atlas_width, atlas_height) = (atlas.0.max(1) as f32, atlas.1.max(1) as f32);

    for (index, prop) in chunk.props.iter().enumerate() {
        let kind = PropKind::from_name(&prop.name);
        let rect = prop_cell(kind, index as u32 + prop.position.x as u32);
        let size = prop_size(kind) * prop.scale.max(0.1);
        let y = prop.position.y + prop_height(kind) * prop.scale.max(0.1);
        let half = size * 0.5;
        let (u0, v0) = (rect[0] as f32 / atlas_width, rect[1] as f32 / atlas_height);
        let (u1, v1) = (
            (rect[0] + rect[2]) as f32 / atlas_width,
            (rect[1] + rect[3]) as f32 / atlas_height,
        );

        // A horizontal quad, rotated by the prop's yaw.
        let (sin, cos) = prop.yaw.sin_cos();
        let right = Vec3::new(cos, 0.0, -sin) * half;
        let forward = Vec3::new(sin, 0.0, cos) * half;
        let centre = Vec3::new(prop.position.x, y, prop.position.z);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(Vertex::new(
            centre - right - forward,
            Vec3::UP,
            Vec2::new(u0, v1),
        ));
        mesh.vertices.push(Vertex::new(
            centre + right - forward,
            Vec3::UP,
            Vec2::new(u1, v1),
        ));
        mesh.vertices.push(Vertex::new(
            centre + right + forward,
            Vec3::UP,
            Vec2::new(u1, v0),
        ));
        mesh.vertices.push(Vertex::new(
            centre - right + forward,
            Vec3::UP,
            Vec2::new(u0, v0),
        ));
        mesh.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    mesh.recompute_bounds();
    mesh
}

/// Appends an axis-aligned box with wall sides and a roof.
fn append_box(
    mesh: &mut Mesh,
    bounds: &noxel_core::math::Aabb,
    wall: Option<(f32, f32, f32, f32)>,
    roof: Option<(f32, f32, f32, f32)>,
) {
    let (w, r) = (
        wall.unwrap_or((0.0, 0.0, 1.0, 1.0)),
        roof.unwrap_or((0.0, 0.0, 1.0, 1.0)),
    );
    let size = bounds.size();
    let base = bounds.min;
    // Four walls, each a quad with the wall texture stretched one tile per 2 m.
    let faces: [(Vec3, Vec3, Vec3); 4] = [
        (
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(size.x, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
        ),
        (
            Vec3::new(size.x, 0.0, 0.0),
            Vec3::new(0.0, 0.0, size.z),
            Vec3::new(1.0, 0.0, 0.0),
        ),
        (
            Vec3::new(size.x, 0.0, size.z),
            Vec3::new(-size.x, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ),
        (
            Vec3::new(0.0, 0.0, size.z),
            Vec3::new(0.0, 0.0, -size.z),
            Vec3::new(-1.0, 0.0, 0.0),
        ),
    ];
    for (corner, edge, normal) in faces {
        let a = base + corner;
        let b = a + edge;
        let top = size.y;
        let start = mesh.vertices.len() as u32;
        let (u0, v0, u1, v1) = w;
        mesh.vertices
            .push(Vertex::new(a, normal, Vec2::new(u0, v1)));
        mesh.vertices
            .push(Vertex::new(b, normal, Vec2::new(u1, v1)));
        mesh.vertices
            .push(Vertex::new(b + Vec3::Y * top, normal, Vec2::new(u1, v0)));
        mesh.vertices
            .push(Vertex::new(a + Vec3::Y * top, normal, Vec2::new(u0, v0)));
        // The two windings differ per face because `edge` runs in different
        // directions, so pick the one whose geometric normal points the way the
        // face should: a wall wound the wrong way is invisible from outside and
        // visible from inside, which is a confusing thing to debug.
        let tri_a = [a, b + Vec3::Y * top, b];
        let geometric = (tri_a[1] - tri_a[0]).cross(tri_a[2] - tri_a[0]);
        if geometric.dot(normal) >= 0.0 {
            mesh.indices.extend_from_slice(&[
                start,
                start + 2,
                start + 1,
                start,
                start + 3,
                start + 2,
            ]);
        } else {
            mesh.indices.extend_from_slice(&[
                start,
                start + 1,
                start + 2,
                start,
                start + 2,
                start + 3,
            ]);
        }
    }
    // The roof: a quad 4 cm above the walls so the sun finds an edge.
    let lip = 0.04;
    let (u0, v0, u1, v1) = r;
    let top = bounds.max.y + lip;
    let start = mesh.vertices.len() as u32;
    let corners = [
        Vec3::new(bounds.min.x, top, bounds.min.z),
        Vec3::new(bounds.max.x, top, bounds.min.z),
        Vec3::new(bounds.max.x, top, bounds.max.z),
        Vec3::new(bounds.min.x, top, bounds.max.z),
    ];
    mesh.vertices
        .push(Vertex::new(corners[0], Vec3::UP, Vec2::new(u0, v1)));
    mesh.vertices
        .push(Vertex::new(corners[1], Vec3::UP, Vec2::new(u1, v1)));
    mesh.vertices
        .push(Vertex::new(corners[2], Vec3::UP, Vec2::new(u1, v0)));
    mesh.vertices
        .push(Vertex::new(corners[3], Vec3::UP, Vec2::new(u0, v0)));
    mesh.indices
        .extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
}

/// Extracts the world-space footprint of a prop, for the debug overlay.
#[must_use]
#[allow(dead_code)]
pub fn prop_bounds(position: Vec3, kind: PropKind, scale: f32) -> noxel_core::math::Aabb {
    let size = prop_size(kind) * scale.max(0.1);
    let height = prop_height(kind) * scale.max(0.1);
    noxel_core::math::Aabb::new(
        Vec3::new(position.x - size * 0.5, position.y, position.z - size * 0.5),
        Vec3::new(
            position.x + size * 0.5,
            position.y + height,
            position.z + size * 0.5,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_asset::format::{TileDef, TileFlags};
    use noxel_core::math::{Aabb, ChunkPos};
    use noxel_world::BiomeId;

    fn set() -> TileSet {
        let tile = |id: u32, name: &str, uv: [u32; 4]| TileDef {
            id,
            name: name.into(),
            texture: "buildings.png".into(),
            uv,
            flags: TileFlags::solid(),
            height: 0.0,
            layer: 0,
        };
        TileSet {
            name: "merged".into(),
            texture: String::new(),
            tile_size: 16,
            tiles: vec![
                tile(16, "wall_plaster", [0, 0, 16, 16]),
                tile(24, "roof_red", [64, 0, 16, 16]),
            ],
        }
    }

    fn chunk_with_building() -> Chunk {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        chunk.buildings.push(noxel_world::town::BuildingInstance {
            prefab: "house_small".into(),
            origin: Vec3::new(1.0, 0.0, 1.0),
            yaw: 0.0,
            size_tiles: (2, 2),
            bounds: Aabb::new(Vec3::new(1.0, 0.0, 1.0), Vec3::new(3.0, 3.0, 3.0)),
            facing: noxel_world::town::Facing::South,
            occluders: Vec::new(),
        });
        chunk
    }

    #[test]
    fn prop_cells_are_inside_the_atlas() {
        for kind in [
            PropKind::Tree,
            PropKind::Rock,
            PropKind::Bush,
            PropKind::Post,
            PropKind::Other,
        ] {
            for variant in 0..6 {
                let rect = prop_cell(kind, variant);
                assert!(rect[0] + rect[2] <= 256, "{kind:?} {variant}");
                assert!(rect[1] + rect[3] <= 256);
            }
        }
    }

    #[test]
    fn trees_have_three_canopy_frames() {
        let frames: Vec<u32> = (0..3).map(|v| prop_cell(PropKind::Tree, v)[0]).collect();
        assert_eq!(frames, vec![0, 24, 48]);
        assert_eq!(prop_cell(PropKind::Tree, 3)[0], 0, "the frames wrap");
    }

    #[test]
    fn prop_heights_are_ordered_sensibly() {
        assert!(prop_height(PropKind::Tree) > prop_height(PropKind::Post));
        assert!(prop_height(PropKind::Post) > prop_height(PropKind::Bush));
        assert!(prop_height(PropKind::Bush) > prop_height(PropKind::Rock));
    }

    #[test]
    fn building_mesh_has_a_box_per_building() {
        let chunk = chunk_with_building();
        let mesh = build_building_mesh(&chunk, &set(), (256, 16));
        // Four walls and a roof, two triangles each.
        assert_eq!(mesh.triangle_count(), 10);
        assert!(!mesh.is_empty());
    }

    #[test]
    fn building_mesh_covers_the_bounds() {
        let chunk = chunk_with_building();
        let mesh = build_building_mesh(&chunk, &set(), (256, 16));
        assert!(
            mesh.bounds.min.x <= 1.0 && mesh.bounds.max.x >= 3.0,
            "{:?}",
            mesh.bounds
        );
        assert!(mesh.bounds.max.y > 3.0, "the roof sits above the walls");
    }

    #[test]
    fn building_faces_point_outwards() {
        let chunk = chunk_with_building();
        let mesh = build_building_mesh(&chunk, &set(), (256, 16));
        let centre = Vec3::new(2.0, 1.5, 2.0);
        for tri in 0..mesh.triangle_count() {
            let n = mesh.triangle_normal(tri);
            let c = mesh
                .triangle(tri)
                .iter()
                .fold(Vec3::ZERO, |a, v| a + v.position)
                / 3.0;
            // Every face points away from the centre, or is the roof.
            assert!(n.dot(c - centre) > -1e-3, "tri {tri}: n={n:?} c={c:?}");
        }
    }

    #[test]
    fn building_mesh_is_empty_without_buildings() {
        let chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        assert!(build_building_mesh(&chunk, &set(), (256, 16)).is_empty());
    }

    #[test]
    fn building_mesh_survives_a_set_without_wall_tiles() {
        let chunk = chunk_with_building();
        let mesh = build_building_mesh(&chunk, &TileSet::default(), (256, 16));
        assert_eq!(
            mesh.triangle_count(),
            10,
            "it must still draw, with flat UVs"
        );
    }

    #[test]
    fn prop_mesh_has_two_triangles_per_prop() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        for i in 0..5 {
            chunk.props.push(noxel_world::chunk::PropInstance::new(
                "tree",
                Vec3::new(i as f32, 0.0, 0.0),
                0.0,
                1.0,
            ));
        }
        let mesh = build_prop_mesh(&chunk, (256, 32));
        assert_eq!(mesh.triangle_count(), 10);
        assert_eq!(mesh.vertex_count(), 20);
    }

    #[test]
    fn prop_quads_face_upwards() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        chunk.props.push(noxel_world::chunk::PropInstance::new(
            "tree",
            Vec3::ZERO,
            0.7,
            1.2,
        ));
        let mesh = build_prop_mesh(&chunk, (256, 32));
        for tri in 0..mesh.triangle_count() {
            assert!(mesh.triangle_normal(tri).y > 0.9, "tri {tri}");
        }
    }

    #[test]
    fn prop_bounds_sit_on_the_ground() {
        let bounds = prop_bounds(Vec3::new(3.0, 1.0, 4.0), PropKind::Tree, 1.0);
        assert_eq!(bounds.min.y, 1.0);
        assert!(bounds.max.y > 1.0);
        assert!(bounds.contains_point(Vec3::new(3.0, 2.0, 4.0)));
    }

    #[test]
    fn empty_chunk_produces_no_prop_geometry() {
        let chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        assert!(build_prop_mesh(&chunk, (256, 32)).is_empty());
    }
}
