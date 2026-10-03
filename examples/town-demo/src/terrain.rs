//! Turns streamed world chunks into scene geometry and physics colliders.
//!
//! One chunk becomes one mesh: a ground quad per tile, at the tile's height, with
//! a UV region from the texture atlas. That is deliberately coarse — the
//! renderer's tile binner handles a few thousand tiny triangles well, and one
//! instance per chunk means frustum and distance culling work at chunk
//! granularity, which is exactly the granularity the streaming system already
//! thinks in.
//!
//! **Terrain is drawn unlit.** The whole point of pixel art is that a tile's
//! colour is the colour the artist chose, and a lit surface would multiply it by
//! the sun. Depth cues come from the ray-traced ambient occlusion in the hybrid
//! mode and from the props, which *are* lit. That is the look this engine exists
//! to produce.

use std::collections::HashMap;

use noxel_app::App;
use noxel_asset::format::TileDef;
use noxel_asset::format::TileSet;
use noxel_asset::image::Image;
use noxel_asset::texture::Texture;
use noxel_core::math::{Aabb, Ray, Transform, Vec2, Vec3};
use noxel_physics::{LAYER_WORLD, QueryFilter};
use noxel_render::material::Material;
use noxel_render::mesh::{Mesh, Vertex};
use noxel_render::scene::InstanceHandle;
use noxel_world::WorldChunkPos;
use noxel_world::chunk::Chunk;

/// How far beyond the visible rectangle chunks are kept as geometry.
const MARGIN_CHUNKS: i32 = 1;

/// The terrain renderer.
pub struct TerrainPlugin {
    /// The material every chunk's ground shares.
    material: Option<noxel_render::material::MaterialHandle>,
    /// The material chunk buildings share.
    building_material: Option<noxel_render::material::MaterialHandle>,
    /// The material chunk props share.
    prop_material: Option<noxel_render::material::MaterialHandle>,
    /// Building and prop instances, keyed by chunk position.
    detail_instances: HashMap<WorldChunkPos, Vec<InstanceHandle>>,
    /// Building and prop meshes, so a despawn can release them.
    detail_meshes: HashMap<WorldChunkPos, Vec<noxel_render::material::MeshHandle>>,
    /// Chunk instances currently in the scene, keyed by chunk position.
    instances: HashMap<WorldChunkPos, InstanceHandle>,
    /// Chunk meshes, so a despawn can release them.
    meshes: HashMap<WorldChunkPos, noxel_render::material::MeshHandle>,
    /// Collider bodies inserted for the chunks currently resident.
    bodies: HashMap<WorldChunkPos, Vec<noxel_physics::BodyHandle>>,
    /// Chunks whose geometry has been built since startup.
    pub chunks_built: u64,
    /// Triangles submitted across all built chunks.
    pub triangles_built: u64,
    /// Static bodies currently in the physics world.
    pub static_bodies: usize,
    /// The texture the terrain samples.
    pub texture_name: String,
    /// Terrain atlas size in pixels, needed to map a tile's pixel `uv` to
    /// texture coordinates.
    pub terrain_atlas: (u32, u32),
    /// Building atlas size.
    pub building_atlas: (u32, u32),
    /// Prop atlas size.
    pub prop_atlas: (u32, u32),
}

impl Default for TerrainPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TerrainPlugin {
    /// An empty terrain renderer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            material: None,
            building_material: None,
            prop_material: None,
            detail_instances: HashMap::new(),
            detail_meshes: HashMap::new(),
            instances: HashMap::new(),
            meshes: HashMap::new(),
            bodies: HashMap::new(),
            chunks_built: 0,
            triangles_built: 0,
            static_bodies: 0,
            texture_name: String::new(),
            terrain_atlas: (256, 16),
            building_atlas: (256, 16),
            prop_atlas: (256, 32),
        }
    }

    #[allow(dead_code)]
    /// Number of chunks currently drawn.
    #[must_use]
    pub fn resident(&self) -> usize {
        self.instances.len()
    }

    #[allow(dead_code)]
    /// Number of building and prop instances currently drawn.
    #[must_use]
    pub fn detail_instances(&self) -> usize {
        self.detail_instances.values().map(Vec::len).sum()
    }

    /// Builds the shared material, loading the terrain texture if it exists.
    fn ensure_material(&mut self, app: &mut App) {
        if self.material.is_some() {
            return;
        }
        let name = texture_for(&app.context.tile_set, "grass")
            .or_else(|| {
                app.context
                    .tile_set
                    .tiles
                    .first()
                    .map(|t| t.texture.clone())
            })
            .unwrap_or_default();
        let texture = load_texture(app, &name);
        if let Some(handle) = texture {
            if let Some(tex) = app.scene().texture(handle) {
                self.terrain_atlas = (tex.width(), tex.height());
            }
        }
        self.texture_name = name;
        let material = match texture {
            Some(handle) => {
                Material::unlit("terrain", noxel_core::math::Color::WHITE).with_texture(handle)
            }
            None => Material::unlit("terrain", noxel_core::math::Color::WHITE),
        };
        self.material = Some(app.scene_mut().add_material(material));

        // Buildings sample the building atlas; props are flat-coloured, because
        // a 24-pixel canopy drawn as a billboard in a busy atlas reads worse
        // from directly above than a solid cone of the right green does.
        let buildings_texture = load_texture(app, "buildings.png");
        if let Some(handle) = buildings_texture {
            if let Some(tex) = app.scene().texture(handle) {
                self.building_atlas = (tex.width(), tex.height());
            }
        }
        let building_material = match buildings_texture {
            Some(handle) => {
                Material::lit("buildings", noxel_core::math::Color::WHITE).with_texture(handle)
            }
            None => Material::lit("buildings", noxel_core::math::Color::rgb(0.78, 0.72, 0.62)),
        };
        self.building_material = Some(app.scene_mut().add_material(building_material));
        self.prop_material = Some(app.scene_mut().add_material(Material::lit(
            "props",
            noxel_core::math::Color::rgb(0.30, 0.46, 0.24),
        )));
    }

    /// Syncs the scene and the physics world to the chunks around the camera.
    pub fn sync(&mut self, app: &mut App) {
        self.ensure_material(app);
        let Some(material) = self.material else {
            return;
        };

        let keep = app.context.keep_bounds();
        let loaded: Vec<WorldChunkPos> = app
            .context
            .streamer
            .iter_loaded()
            .filter(|(_, chunk)| overlaps(&chunk.bounds(), &keep, MARGIN_CHUNKS))
            .map(|(pos, _)| pos)
            .collect();

        // Drop geometry for chunks that left the view.
        let stale: Vec<WorldChunkPos> = self
            .instances
            .keys()
            .copied()
            .filter(|pos| !loaded.contains(pos))
            .collect();
        for pos in stale {
            if let Some(handle) = self.instances.remove(&pos) {
                app.scene_mut().remove_instance(handle);
            }
            if let Some(mesh) = self.meshes.remove(&pos) {
                app.scene_mut().remove_mesh(mesh);
            }
            if let Some(bodies) = self.bodies.remove(&pos) {
                for body in bodies {
                    app.physics_mut().remove(body);
                }
            }
            if let Some(details) = self.detail_instances.remove(&pos) {
                for handle in details {
                    app.scene_mut().remove_instance(handle);
                }
            }
            if let Some(meshes) = self.detail_meshes.remove(&pos) {
                for mesh in meshes {
                    app.scene_mut().remove_mesh(mesh);
                }
            }
        }
        self.static_bodies = self.bodies.values().map(Vec::len).sum();

        // Build the ones that arrived.
        for pos in loaded {
            if self.instances.contains_key(&pos) {
                continue;
            }
            // Everything read from the streamer is copied out first: the chunk
            // borrows `app` immutably and the scene needs it mutably.
            let tile_set = std::sync::Arc::clone(&app.context.tile_set);
            let terrain_atlas = self.terrain_atlas;
            let building_atlas = self.building_atlas;
            let (mesh, building_mesh, prop_mesh, collider_bounds) = {
                let Some(chunk) = app.context.streamer.chunk(pos) else {
                    continue;
                };
                (
                    build_chunk_mesh(chunk, &tile_set, terrain_atlas),
                    crate::village::build_building_mesh(chunk, &tile_set, building_atlas),
                    crate::village::build_prop_mesh(chunk, self.prop_atlas),
                    chunk.colliders.clone(),
                )
            };
            self.triangles_built += (mesh.indices.len() / 3) as u64;
            let handle = app.scene_mut().add_mesh(mesh);
            let instance = app.scene_mut().spawn(
                format!("chunk_{}_{}", pos.x, pos.y),
                handle,
                material,
                Transform::IDENTITY,
            );
            app.scene_mut()
                .set_flags(instance, noxel_render::scene::InstanceFlags::default());
            self.meshes.insert(pos, handle);
            self.instances.insert(pos, instance);
            self.chunks_built += 1;

            // Buildings and props: two extra instances per chunk.
            let mut details = Vec::new();
            let mut detail_meshes = Vec::new();
            if let Some(material) = self.building_material {
                if !building_mesh.is_empty() {
                    self.triangles_built += (building_mesh.indices.len() / 3) as u64;
                    let handle = app.scene_mut().add_mesh(building_mesh);
                    let instance = app.scene_mut().spawn(
                        format!("buildings_{}_{}", pos.x, pos.y),
                        handle,
                        material,
                        Transform::IDENTITY,
                    );
                    details.push(instance);
                    detail_meshes.push(handle);
                }
            }
            if let Some(material) = self.prop_material {
                if !prop_mesh.is_empty() {
                    self.triangles_built += (prop_mesh.indices.len() / 3) as u64;
                    let handle = app.scene_mut().add_mesh(prop_mesh);
                    let instance = app.scene_mut().spawn(
                        format!("props_{}_{}", pos.x, pos.y),
                        handle,
                        material,
                        Transform::IDENTITY,
                    );
                    details.push(instance);
                    detail_meshes.push(handle);
                }
            }
            if !details.is_empty() {
                self.detail_instances.insert(pos, details);
                self.detail_meshes.insert(pos, detail_meshes);
            }

            // Static collision for everything solid in the chunk.
            let mut bodies = Vec::new();
            for (bounds, user_data) in collider_bounds.iter() {
                if bounds.is_empty() {
                    continue;
                }
                bodies.push(app.physics_mut().insert_static_aabb(*bounds, *user_data));
            }
            self.bodies.insert(pos, bodies);
        }
        self.static_bodies = self.bodies.values().map(Vec::len).sum();
    }

    #[allow(dead_code)]
    /// The height of the ground at a world position, from the streamed world.
    #[must_use]
    pub fn ground_height(app: &App, position: Vec3) -> f32 {
        app.context.streamer.height_at(position)
    }

    /// True when a world position is on walkable ground.
    #[must_use]
    pub fn is_walkable(app: &App, position: Vec3) -> bool {
        app.context.streamer.is_walkable(position)
    }

    #[allow(dead_code)]
    /// Casts a ray downwards and returns the ground height under `position`.
    #[must_use]
    pub fn sample_ground(app: &App, position: Vec3) -> Option<f32> {
        let from = position + Vec3::Y * 8.0;
        let ray = Ray::with_max_t(from, Vec3::DOWN, 40.0);
        let filter = QueryFilter {
            mask: LAYER_WORLD,
            ignore: None,
            include_sensors: false,
            include_static: true,
        };
        app.physics().raycast(&ray, filter).map(|hit| hit.point.y)
    }
}

/// Builds one chunk's ground mesh.
///
/// Every tile becomes a quad. Adjacent tiles at the same height share their
/// corner positions but **not** their vertices: merging them would need a
/// topologically consistent index buffer and buys little at this scale, while
/// duplicating them keeps the UV mapping trivially correct per tile.
#[must_use]
pub fn build_chunk_mesh(chunk: &Chunk, set: &TileSet, atlas: (u32, u32)) -> Mesh {
    let tiles = chunk.tiles();
    let tile_size = 1.0f32;
    let origin = Vec3::new(
        chunk.pos.x as f32 * tiles as f32 * tile_size,
        0.0,
        chunk.pos.y as f32 * tiles as f32 * tile_size,
    );

    let mut mesh = Mesh {
        name: format!("chunk_{}_{}", chunk.pos.x, chunk.pos.y),
        ..Mesh::default()
    };

    for ty in 0..tiles {
        for tx in 0..tiles {
            let id = chunk.tile(tx, ty);
            let Some(def) = set.tile(id) else { continue };
            let height = chunk.height(tx, ty);
            let x0 = origin.x + tx as f32 * tile_size;
            let z0 = origin.z + ty as f32 * tile_size;
            let (u0, v0, u1, v1) = uv_rect(def, atlas);
            let base = mesh.vertices.len() as u32;

            // Slight per-tile height variation makes the ground read as
            // uneven under a low sun without a normal map.
            let n = Vec3::UP;
            mesh.vertices
                .push(Vertex::new(Vec3::new(x0, height, z0), n, Vec2::new(u0, v0)));
            mesh.vertices.push(Vertex::new(
                Vec3::new(x0 + tile_size, height, z0),
                n,
                Vec2::new(u1, v0),
            ));
            mesh.vertices.push(Vertex::new(
                Vec3::new(x0 + tile_size, height, z0 + tile_size),
                n,
                Vec2::new(u1, v1),
            ));
            mesh.vertices.push(Vertex::new(
                Vec3::new(x0, height, z0 + tile_size),
                n,
                Vec2::new(u0, v1),
            ));
            // Counter-clockwise seen from above (ADR 0001).
            mesh.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    mesh.recompute_bounds();
    mesh
}

/// The `[u0, v0, u1, v1]` texture rectangle for a tile, given the atlas size in
/// pixels.
///
/// The half-texel inset is what stops a neighbouring tile bleeding into this one
/// when the texture is sampled bilinearly or when the atlas has mips.
#[must_use]
pub fn uv_rect(def: &TileDef, atlas: (u32, u32)) -> (f32, f32, f32, f32) {
    let (w, h) = (atlas.0.max(1) as f32, atlas.1.max(1) as f32);
    let (du, dv) = (0.5 / w, 0.5 / h);
    let u0 = def.uv[0] as f32 / w + du;
    let v0 = def.uv[1] as f32 / h + dv;
    let u1 = (def.uv[0] + def.uv[2]) as f32 / w - du;
    let v1 = (def.uv[1] + def.uv[3]) as f32 / h - dv;
    (u0, v0, u1, v1)
}

/// True when a chunk's bounds overlap the keep region, expanded by `margin` in
/// chunk units.
fn overlaps(bounds: &Aabb, keep: &Aabb, margin: i32) -> bool {
    let pad = margin as f32 * 32.0;
    bounds.max.x + pad >= keep.min.x
        && bounds.min.x - pad <= keep.max.x
        && bounds.max.z + pad >= keep.min.z
        && bounds.min.z - pad <= keep.max.z
}

/// The texture named by a tile, falling back to the tile set's own texture.
fn texture_for(set: &TileSet, tile_name: &str) -> Option<String> {
    let def = set.tile_by_name(tile_name)?;
    let name = set.texture_of(def);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Loads a texture from the asset database, or builds a placeholder.
fn load_texture(app: &mut App, name: &str) -> Option<noxel_render::material::TextureHandle> {
    if name.is_empty() {
        return None;
    }
    if let Ok(texture) = app.context.assets.load_texture(name) {
        return Some(app.scene_mut().add_texture((*texture).clone()));
    }
    // A missing texture must not stop the demo: build a visible placeholder so a
    // fresh checkout without generated assets still shows a world.
    let image = placeholder_texture();
    let texture = Texture::from_image(image);
    Some(app.scene_mut().add_texture(texture))
}

/// A 4x4 checkerboard, used when no terrain texture is available.
#[must_use]
pub fn placeholder_texture() -> Image {
    use noxel_core::math::Color8;
    let mut image = Image::new(4, 4, Color8::new(120, 160, 90, 255));
    for y in 0..4 {
        for x in 0..4 {
            if (x + y) % 2 == 0 {
                image.set(x, y, Color8::new(96, 132, 74, 255));
            }
        }
    }
    image
}

/// A helper the HUD uses to describe the terrain state.
#[must_use]
#[allow(dead_code)]
pub fn describe(plugin: &TerrainPlugin) -> String {
    format!(
        "terrain {} chunks {} detail {} tris {} static {}",
        plugin.resident(),
        plugin.detail_instances(),
        plugin.triangles_built,
        plugin.static_bodies,
        plugin.texture_name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::ChunkPos;
    use noxel_world::BiomeId;

    fn set() -> TileSet {
        TileSet {
            name: "t".into(),
            texture: "t.png".into(),
            tile_size: 16,
            tiles: vec![TileDef {
                id: 0,
                name: "grass".into(),
                texture: String::new(),
                uv: [0, 0, 16, 16],
                flags: noxel_asset::format::TileFlags::walkable(),
                height: 0.0,
                layer: 0,
            }],
        }
    }

    #[test]
    fn uv_rect_is_inset_by_half_a_texel() {
        let def = &set().tiles[0];
        let (u0, v0, u1, v1) = uv_rect(def, (16, 16));
        assert!((u0 - 0.5 / 16.0).abs() < 1e-6, "{u0}");
        assert!((u1 - (1.0 - 0.5 / 16.0)).abs() < 1e-6, "{u1}");
        assert!(v0 > 0.0 && v1 < 1.0);
    }

    #[test]
    fn chunk_mesh_has_four_vertices_per_tile() {
        let set = set();
        let chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        let mesh = build_chunk_mesh(&chunk, &set, (256, 16));
        assert_eq!(mesh.vertices.len(), 4 * 4 * 4);
        assert_eq!(mesh.indices.len(), 4 * 4 * 6);
    }

    #[test]
    fn chunk_mesh_winds_upward() {
        let set = set();
        let chunk = Chunk::empty(ChunkPos::new(0, 0), 2, 1.0, BiomeId::PLAINS);
        let mesh = build_chunk_mesh(&chunk, &set, (256, 16));
        for tri in 0..mesh.triangle_count() {
            let n = mesh.triangle_normal(tri);
            assert!(n.y > 0.9, "tri {tri} faces {n:?}");
        }
    }

    #[test]
    fn chunk_mesh_is_positioned_by_chunk_coordinate() {
        let set = set();
        let chunk = Chunk::empty(ChunkPos::new(2, -1), 4, 1.0, BiomeId::PLAINS);
        let mesh = build_chunk_mesh(&chunk, &set, (256, 16));
        assert!(mesh.bounds.min.x > 7.0, "{:?}", mesh.bounds);
        assert!(mesh.bounds.max.z <= 0.0, "{:?}", mesh.bounds);
    }

    #[test]
    fn chunk_mesh_is_empty_without_tile_definitions() {
        let empty = TileSet {
            tiles: Vec::new(),
            ..set()
        };
        let chunk = Chunk::empty(ChunkPos::new(0, 0), 4, 1.0, BiomeId::PLAINS);
        let mesh = build_chunk_mesh(&chunk, &empty, (256, 16));
        assert!(mesh.is_empty());
    }

    #[test]
    fn placeholder_texture_is_a_checkerboard() {
        let image = placeholder_texture();
        assert_eq!((image.width, image.height), (4, 4));
        assert_ne!(image.get(0, 0), image.get(1, 0));
        assert_eq!(image.get(0, 0), image.get(2, 0));
    }

    #[test]
    fn overlap_check_expands_by_the_margin() {
        let bounds = Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(32.0, 4.0, 32.0));
        // 63.0 is inside one chunk of margin (32 m) from the first box's max of
        // 32.0; 96.0 would not be.
        let far = Aabb::new(Vec3::new(63.0, 0.0, 0.0), Vec3::new(96.0, 4.0, 32.0));
        assert!(!overlaps(&bounds, &far, 0));
        assert!(
            overlaps(&bounds, &far, 1),
            "one chunk of margin must reach it"
        );
    }

    #[test]
    fn texture_for_resolves_the_tile_set_texture() {
        let set = set();
        assert_eq!(texture_for(&set, "grass").as_deref(), Some("t.png"));
        assert!(texture_for(&set, "nope").is_none());
    }

    #[test]
    fn describe_mentions_the_counters() {
        let text = describe(&TerrainPlugin::new());
        assert!(text.contains("terrain"));
        assert!(text.contains("chunks"));
    }

    #[test]
    fn new_plugin_is_empty() {
        let plugin = TerrainPlugin::new();
        assert_eq!(plugin.resident(), 0);
        assert_eq!(plugin.chunks_built, 0);
        assert!(plugin.material.is_none());
    }
}
