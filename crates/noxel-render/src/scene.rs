//! The scene: meshes, materials, textures, instances and lights.
//!
//! A [`Scene`] is purely *description*. It holds no GPU resources, no framebuffer
//! and no per-frame state, so:
//!
//! * the rasterizer and the ray tracer consume the same scene and agree on what
//!   is in the world (see `docs/adr/0004-dual-renderer.md`);
//! * a test can build a scene and assert on it without a renderer;
//! * the demo can serialise it for a golden-image test.
//!
//! Instances are generational [`Handle`]s into a slot map rather than raw `Vec`
//! indices, so removing one instance never silently re-points a reference that
//! the visibility system is still holding.

use noxel_asset::texture::Texture;
use noxel_core::math::{Aabb, Color, Transform, Vec3};
use noxel_core::pool::{Handle, SlotMap};

use crate::light::{Ambient, Fog, Light};
use crate::material::{Material, MaterialHandle, MaterialLibrary, MeshHandle, TextureHandle};
use crate::mesh::Mesh;

/// A handle to an instance in a [`Scene`].
pub type InstanceHandle = Handle<Instance>;

/// Per-instance flags that the visibility and shadow passes care about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceFlags {
    /// The instance can occlude the camera's view of the player, and should be
    /// faded when it does. Roofs, tree canopies, archways.
    pub occluder: bool,
    /// The instance can cast a shadow.
    pub cast_shadow: bool,
    /// The instance receives shadows.
    pub receive_shadow: bool,
    /// Never culled, however far away or however small (the player, a quest
    /// marker, the skybox).
    pub always_visible: bool,
    /// Excluded from the camera-occlusion fade: when this instance blocks the
    /// view the camera moves instead of the instance fading. Used for solid
    /// walls in tight interiors.
    pub never_fade: bool,
    /// The instance participates in the ID buffer, for picking and the debug
    /// overlay.
    pub pickable: bool,
}

impl Default for InstanceFlags {
    fn default() -> Self {
        Self {
            occluder: false,
            cast_shadow: true,
            receive_shadow: true,
            always_visible: false,
            never_fade: false,
            pickable: true,
        }
    }
}

impl InstanceFlags {
    /// An occluder that fades when it blocks the camera.
    #[must_use]
    pub fn roof() -> Self {
        Self {
            occluder: true,
            cast_shadow: true,
            receive_shadow: false,
            ..Self::default()
        }
    }

    /// Foliage: an occluder that also casts a shadow.
    #[must_use]
    pub fn foliage() -> Self {
        Self {
            occluder: true,
            cast_shadow: true,
            receive_shadow: true,
            ..Self::default()
        }
    }

    /// A sprite/character: visible, shadow-casting, never an occluder.
    #[must_use]
    pub fn character() -> Self {
        Self {
            occluder: false,
            cast_shadow: true,
            receive_shadow: true,
            ..Self::default()
        }
    }

    /// A marker that must never be culled or faded.
    #[must_use]
    pub fn marker() -> Self {
        Self {
            always_visible: true,
            never_fade: true,
            cast_shadow: false,
            receive_shadow: false,
            ..Self::default()
        }
    }

    /// A solid wall that the camera should never see through, but which may
    /// still fade (the default is to fade).
    #[must_use]
    pub fn wall() -> Self {
        Self {
            occluder: true,
            never_fade: false,
            ..Self::default()
        }
    }
}

/// One drawable object: a mesh, a material and a transform.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Optional name, for debugging and for the overlay.
    pub name: String,
    /// The mesh to draw.
    pub mesh: MeshHandle,
    /// The material to shade it with.
    pub material: MaterialHandle,
    /// World transform.
    pub transform: Transform,
    /// Extra colour multiplier (team tint, damage flash, fade).
    pub tint: Color,
    /// Render layer bitmask; the camera can hide layers (e.g. the roof layer
    /// when walking indoors).
    pub layer: u32,
    /// Behaviour flags for culling, shadows and fading.
    pub flags: InstanceFlags,
    /// Arbitrary game data, round-tripped through the visibility system so the
    /// game can map a pick back to its entity.
    pub user_data: u64,
    /// Set to `false` to skip this instance without removing it.
    pub visible: bool,
    /// Stable ordering hint for transparent draws with equal depth.
    pub sort_bias: i32,
    /// World-space bounds, refreshed by [`Scene::update_bounds`] after a
    /// transform change.
    bounds: Aabb,
}

impl Instance {
    /// The instance's cached world bounds.
    ///
    /// Cached (rather than recomputed) because the visibility system reads it
    /// several times per frame; call [`Scene::update_bounds`] after moving or
    /// re-meshing an instance.
    #[inline]
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// Recomputes the bounds from the mesh bounds and the transform.
    fn refresh_bounds(&mut self, mesh: &Mesh) {
        self.bounds = mesh.bounds.transform(&self.transform.to_mat4());
    }
}

/// A complete, renderer-agnostic description of what to draw.
#[derive(Debug, Default)]
pub struct Scene {
    meshes: SlotMap<Mesh>,
    materials: SlotMap<Material>,
    textures: SlotMap<Texture>,
    instances: SlotMap<Instance>,
    /// Material lookup by name, so generated content can say `"grass"`.
    pub material_library: MaterialLibrary,
    /// Every light in the scene.
    pub lights: Vec<Light>,
    /// Ambient terms applied to every shaded surface.
    pub ambient: Ambient,
    /// Optional distance fog.
    pub fog: Option<Fog>,
    /// The colour used where nothing is drawn (and as the ray tracer's sky).
    pub background: Color,
    /// A monotonically increasing revision, bumped by every mutation. The
    /// renderers use it to invalidate their cached acceleration structures
    /// instead of rebuilding them every frame.
    revision: u64,
}

impl Scene {
    /// An empty scene with the default ambient.
    #[must_use]
    pub fn new() -> Self {
        Self {
            meshes: SlotMap::new(),
            materials: SlotMap::new(),
            textures: SlotMap::new(),
            instances: SlotMap::new(),
            material_library: MaterialLibrary::new(),
            lights: Vec::new(),
            ambient: Ambient::default(),
            fog: None,
            background: Color::rgb(0.09, 0.11, 0.15),
            revision: 1,
        }
    }

    /// The revision counter; changes whenever the scene is mutated.
    #[inline]
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Marks the scene dirty. Called by every mutator.
    #[inline]
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    // ------------------------------------------------------------- meshes

    /// Adds a mesh.
    pub fn add_mesh(&mut self, mesh: Mesh) -> MeshHandle {
        self.touch();
        self.meshes.insert(mesh)
    }

    /// A mesh by handle.
    #[must_use]
    pub fn mesh(&self, handle: MeshHandle) -> Option<&Mesh> {
        self.meshes.get(handle)
    }

    /// Total number of meshes.
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Every mesh with its handle.
    pub fn meshes(&self) -> impl Iterator<Item = (MeshHandle, &Mesh)> {
        self.meshes.iter()
    }

    // --------------------------------------------------------- materials

    /// Adds a material and registers its name in the library.
    pub fn add_material(&mut self, material: Material) -> MaterialHandle {
        self.touch();
        let name = material.name.clone();
        let handle = self.materials.insert(material);
        if !name.is_empty() {
            self.material_library.insert(name, handle);
        }
        handle
    }

    /// A material by handle.
    #[must_use]
    pub fn material(&self, handle: MaterialHandle) -> Option<&Material> {
        self.materials.get(handle)
    }

    /// A material by handle, mutably. Bumps the revision.
    pub fn material_mut(&mut self, handle: MaterialHandle) -> Option<&mut Material> {
        self.touch();
        self.materials.get_mut(handle)
    }

    /// A material by registered name.
    #[must_use]
    pub fn material_by_name(&self, name: &str) -> Option<&Material> {
        self.materials.get(self.material_library.get(name)?)
    }

    /// A material handle by registered name.
    #[must_use]
    pub fn material_handle(&self, name: &str) -> Option<MaterialHandle> {
        self.material_library.get(name)
    }

    /// Total number of materials.
    #[must_use]
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    // ---------------------------------------------------------- textures

    /// Adds a texture.
    pub fn add_texture(&mut self, texture: Texture) -> TextureHandle {
        self.touch();
        self.textures.insert(texture)
    }

    /// A texture by handle.
    #[must_use]
    pub fn texture(&self, handle: TextureHandle) -> Option<&Texture> {
        self.textures.get(handle)
    }

    /// Total number of textures.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    // --------------------------------------------------------- instances

    /// Adds an instance, computing its world bounds.
    pub fn add_instance(&mut self, instance: Instance) -> InstanceHandle {
        self.touch();
        let mut instance = instance;
        if let Some(mesh) = self.meshes.get(instance.mesh) {
            instance.refresh_bounds(mesh);
        }
        self.instances.insert(instance)
    }

    /// A convenience constructor: mesh + material + transform.
    pub fn spawn(
        &mut self,
        name: impl Into<String>,
        mesh: MeshHandle,
        material: MaterialHandle,
        transform: Transform,
    ) -> InstanceHandle {
        self.add_instance(Instance {
            name: name.into(),
            mesh,
            material,
            transform,
            tint: Color::WHITE,
            layer: 1,
            flags: InstanceFlags::default(),
            user_data: 0,
            visible: true,
            sort_bias: 0,
            bounds: Aabb::EMPTY,
        })
    }

    /// Removes an instance, returning it.
    pub fn remove_instance(&mut self, handle: InstanceHandle) -> Option<Instance> {
        self.touch();
        self.instances.remove(handle)
    }

    /// An instance by handle.
    #[must_use]
    pub fn instance(&self, handle: InstanceHandle) -> Option<&Instance> {
        self.instances.get(handle)
    }

    /// An instance by handle, mutably. Bumps the revision.
    pub fn instance_mut(&mut self, handle: InstanceHandle) -> Option<&mut Instance> {
        self.touch();
        self.instances.get_mut(handle)
    }

    /// Sets an instance's transform and refreshes its bounds.
    ///
    /// Prefer this over mutating [`Instance::transform`] directly: it is the
    /// only way the cached bounds stay correct, and a stale AABB makes the
    /// visibility system cull something it should not.
    /// Replaces an instance's flags. Returns false when the handle is stale.
    pub fn set_flags(&mut self, handle: InstanceHandle, flags: InstanceFlags) -> bool {
        match self.instances.get_mut(handle) {
            Some(instance) => {
                instance.flags = flags;
                self.touch();
                true
            }
            None => false,
        }
    }

    /// Removes a mesh and returns whether it was there.
    ///
    /// Instances referencing it keep a stale handle and are skipped when drawn,
    /// which is what makes a streaming system's teardown order irrelevant.
    pub fn remove_mesh(&mut self, handle: MeshHandle) -> bool {
        let removed = self.meshes.remove(handle).is_some();
        if removed {
            self.touch();
        }
        removed
    }

    /// Removes a material.
    pub fn remove_material(&mut self, handle: MaterialHandle) -> bool {
        let removed = self.materials.remove(handle).is_some();
        if removed {
            self.touch();
        }
        removed
    }

    /// Removes a texture.
    pub fn remove_texture(&mut self, handle: TextureHandle) -> bool {
        let removed = self.textures.remove(handle).is_some();
        if removed {
            self.touch();
        }
        removed
    }

    /// Moves and rotates an instance, refreshing its cached world bounds.
    ///
    /// Returns false when the handle is stale. Refreshing the bounds here (rather
    /// than leaving it to the caller) is what keeps culling correct for anything
    /// that moves — a culler reading stale bounds will happily draw an object
    /// that has gone off-screen, or cull one that has come back.
    pub fn set_transform(&mut self, handle: InstanceHandle, transform: Transform) -> bool {
        self.touch();
        let mesh_bounds = match self.instances.get(handle) {
            Some(i) => self.meshes.get(i.mesh).map(|m| m.bounds),
            None => return false,
        };
        let Some(instance) = self.instances.get_mut(handle) else {
            return false;
        };
        instance.transform = transform;
        if let Some(b) = mesh_bounds {
            instance.bounds = b.transform(&transform.to_mat4());
        }
        true
    }

    /// Recomputes an instance's bounds (after swapping its mesh).
    pub fn update_bounds(&mut self, handle: InstanceHandle) -> bool {
        let mesh_bounds = match self.instances.get(handle) {
            Some(i) => self.meshes.get(i.mesh).map(|m| m.bounds),
            None => return false,
        };
        let Some(instance) = self.instances.get_mut(handle) else {
            return false;
        };
        if let Some(b) = mesh_bounds {
            instance.bounds = b.transform(&instance.transform.to_mat4());
        }
        true
    }

    /// Recomputes every instance's bounds. Use after moving many things at once.
    pub fn update_all_bounds(&mut self) {
        let handles: Vec<InstanceHandle> = self.instances.keys().collect();
        for h in handles {
            self.update_bounds(h);
        }
    }

    /// Number of instances.
    #[must_use]
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Every instance with its handle.
    pub fn instances(&self) -> impl Iterator<Item = (InstanceHandle, &Instance)> {
        self.instances.iter()
    }

    /// Every instance handle.
    pub fn instance_handles(&self) -> impl Iterator<Item = InstanceHandle> + '_ {
        self.instances.keys()
    }

    /// The joint bounds of every visible instance.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for (_, i) in self.instances.iter() {
            if i.visible {
                b.grow_aabb(&i.bounds);
            }
        }
        b
    }

    /// The bounds of every instance flagged as an occluder.
    #[must_use]
    pub fn occluder_bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for (_, i) in self.instances.iter() {
            if i.visible && i.flags.occluder {
                b.grow_aabb(&i.bounds);
            }
        }
        b
    }

    // ------------------------------------------------------------- lights

    /// Adds a light.
    pub fn add_light(&mut self, light: Light) {
        self.touch();
        self.lights.push(light);
    }

    /// Number of lights.
    #[must_use]
    pub fn light_count(&self) -> usize {
        self.lights.len()
    }

    /// The first directional light, which the rasterizer uses for shadows.
    #[must_use]
    pub fn primary_sun(&self) -> Option<&Light> {
        self.lights
            .iter()
            .find(|l| matches!(l, Light::Directional { .. }))
    }

    /// Removes every light.
    pub fn clear_lights(&mut self) {
        self.touch();
        self.lights.clear();
    }

    // ------------------------------------------------------------ summary

    /// A one-line description, printed by the demo at startup.
    #[must_use]
    pub fn summary(&self) -> String {
        let triangles: usize = self
            .instances
            .values()
            .filter(|i| i.visible)
            .filter_map(|i| self.meshes.get(i.mesh))
            .map(Mesh::triangle_count)
            .sum();
        format!(
            "scene: {} instances, {} meshes, {} materials, {} textures, {} lights, {} triangles",
            self.instances.len(),
            self.meshes.len(),
            self.materials.len(),
            self.textures.len(),
            self.lights.len(),
            triangles
        )
    }

    /// Aggregate statistics.
    #[must_use]
    pub fn stats(&self) -> SceneStats {
        let mut triangles = 0usize;
        let mut bytes = 0usize;
        for (_, m) in self.meshes.iter() {
            triangles += m.triangle_count();
            bytes += m.memory_bytes();
        }
        SceneStats {
            meshes: self.meshes.len(),
            materials: self.materials.len(),
            textures: self.textures.len(),
            instances: self.instances.len(),
            visible_instances: self.instances.values().filter(|i| i.visible).count(),
            occluders: self.instances.values().filter(|i| i.flags.occluder).count(),
            lights: self.lights.len(),
            triangles,
            memory_bytes: bytes,
            revision: self.revision,
        }
    }
}

/// Aggregate scene statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    /// Number of meshes.
    pub meshes: usize,
    /// Number of materials.
    pub materials: usize,
    /// Number of textures.
    pub textures: usize,
    /// Number of instances.
    pub instances: usize,
    /// Instances with `visible == true`.
    pub visible_instances: usize,
    /// Instances flagged as occluders.
    pub occluders: usize,
    /// Number of lights.
    pub lights: usize,
    /// Total triangles across every mesh.
    pub triangles: usize,
    /// Mesh heap usage in bytes.
    pub memory_bytes: usize,
    /// Scene revision.
    pub revision: u64,
}

/// A convenient world-space axis-aligned box instance, used for collision
/// proxies and debug visualisation.
pub fn box_instance(
    mesh: MeshHandle,
    material: MaterialHandle,
    center: Vec3,
    size: Vec3,
) -> Instance {
    let mut instance = Instance {
        name: String::from("box"),
        mesh,
        material,
        transform: Transform::IDENTITY,
        tint: Color::WHITE,
        layer: 1,
        flags: InstanceFlags::default(),
        user_data: 0,
        visible: true,
        sort_bias: 0,
        bounds: Aabb::from_center_half_extents(center, size * 0.5),
    };
    instance.transform.translation = center;
    instance.transform.scale = size * 0.5;
    instance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Material;

    fn setup() -> (Scene, MeshHandle, MaterialHandle) {
        let mut scene = Scene::new();
        let mesh = scene.add_mesh(Mesh::cube(1.0));
        let mat = scene.add_material(Material::unlit("stone", Color::WHITE));
        (scene, mesh, mat)
    }

    #[test]
    fn add_and_look_up_resources() {
        let (scene, mesh, mat) = setup();
        assert_eq!(scene.mesh_count(), 1);
        assert_eq!(scene.material_count(), 1);
        assert!(scene.mesh(mesh).is_some());
        assert!(scene.material(mat).is_some());
        assert_eq!(
            scene.material_by_name("stone").map(|m| m.name.as_str()),
            Some("stone")
        );
        assert_eq!(scene.material_handle("stone"), Some(mat));
        assert_eq!(scene.material_handle("missing"), None);
    }

    #[test]
    fn instance_bounds_track_the_transform() {
        let (mut scene, mesh, mat) = setup();
        let h = scene.spawn(
            "a",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        );
        let b = scene.instance(h).unwrap().bounds();
        assert!(
            b.center().approx_eq(Vec3::new(10.0, 0.0, 0.0), 1e-4),
            "{b:?}"
        );

        scene.set_transform(h, Transform::from_translation(Vec3::new(-5.0, 2.0, 0.0)));
        let b = scene.instance(h).unwrap().bounds();
        assert!(b.center().approx_eq(Vec3::new(-5.0, 2.0, 0.0), 1e-4));
    }

    #[test]
    fn instance_bounds_respect_scale() {
        let (mut scene, mesh, mat) = setup();
        let h = scene.spawn("a", mesh, mat, Transform::from_scale(4.0));
        let b = scene.instance(h).unwrap().bounds();
        assert!((b.size().x - 4.0).abs() < 1e-4, "{b:?}");
    }

    #[test]
    fn removing_an_instance_keeps_handles_valid() {
        let (mut scene, mesh, mat) = setup();
        let a = scene.spawn("a", mesh, mat, Transform::IDENTITY);
        let b = scene.spawn("b", mesh, mat, Transform::IDENTITY);
        assert!(scene.remove_instance(a).is_some());
        assert!(
            scene.instance(a).is_none(),
            "a stale handle must not resolve"
        );
        assert!(scene.instance(b).is_some());
        assert_eq!(scene.instance_count(), 1);
    }

    #[test]
    fn revision_changes_on_mutation() {
        let (mut scene, mesh, mat) = setup();
        let r0 = scene.revision();
        let h = scene.spawn("a", mesh, mat, Transform::IDENTITY);
        let r1 = scene.revision();
        assert!(r1 > r0);
        scene.set_transform(h, Transform::from_translation(Vec3::ONE));
        assert!(scene.revision() > r1);
    }

    #[test]
    fn scene_bounds_cover_visible_instances_only() {
        let (mut scene, mesh, mat) = setup();
        scene.spawn("near", mesh, mat, Transform::from_translation(Vec3::ZERO));
        let far = scene.spawn(
            "far",
            mesh,
            mat,
            Transform::from_translation(Vec3::splat(100.0)),
        );
        scene.instance_mut(far).unwrap().visible = false;
        let b = scene.bounds();
        assert!(b.max.x < 50.0, "{b:?}");

        // Once the far instance is visible again it must be included.
        scene.instance_mut(far).unwrap().visible = true;
        let b = scene.bounds();
        assert!(b.max.x > 99.0, "{b:?}");
    }

    #[test]
    fn occluder_bounds_only_include_occluders() {
        let (mut scene, mesh, mat) = setup();
        let a = scene.spawn("wall", mesh, mat, Transform::from_translation(Vec3::ZERO));
        scene.instance_mut(a).unwrap().flags = InstanceFlags::roof();
        let b = scene.spawn(
            "prop",
            mesh,
            mat,
            Transform::from_translation(Vec3::splat(50.0)),
        );
        let _ = b;
        let ob = scene.occluder_bounds();
        assert!(ob.max.x < 5.0, "{ob:?}");
    }

    #[test]
    fn lights_are_managed() {
        let mut scene = Scene::new();
        scene.add_light(Light::sun());
        scene.add_light(Light::point(Vec3::ZERO, Color::WHITE, 1.0, 10.0));
        assert_eq!(scene.light_count(), 2);
        assert!(matches!(
            scene.primary_sun(),
            Some(Light::Directional { .. })
        ));
        scene.clear_lights();
        assert_eq!(scene.light_count(), 0);
        assert!(scene.primary_sun().is_none());
    }

    #[test]
    fn stats_are_consistent() {
        let (mut scene, mesh, mat) = setup();
        scene.spawn("a", mesh, mat, Transform::IDENTITY);
        let h = scene.spawn("b", mesh, mat, Transform::IDENTITY);
        scene.instance_mut(h).unwrap().visible = false;
        scene.instance_mut(h).unwrap().flags = InstanceFlags::roof();
        let s = scene.stats();
        assert_eq!(s.meshes, 1);
        assert_eq!(s.instances, 2);
        assert_eq!(s.visible_instances, 1);
        assert_eq!(s.occluders, 1);
        assert_eq!(s.triangles, 12);
        assert!(s.memory_bytes > 0);
    }

    #[test]
    fn summary_mentions_everything() {
        let (mut scene, mesh, mat) = setup();
        scene.spawn("a", mesh, mat, Transform::IDENTITY);
        let s = scene.summary();
        assert!(
            s.contains("1 instances") && s.contains("12 triangles"),
            "{s}"
        );
    }

    #[test]
    fn updating_a_mesh_bounds_after_a_mesh_swap() {
        let (mut scene, mesh, mat) = setup();
        let h = scene.spawn("a", mesh, mat, Transform::IDENTITY);
        let bigger = scene.add_mesh(Mesh::cube(10.0));
        scene.instance_mut(h).unwrap().mesh = bigger;
        scene.update_bounds(h);
        assert!((scene.instance(h).unwrap().bounds().size().x - 10.0).abs() < 1e-4);
    }

    #[test]
    fn update_all_bounds_matches_individual() {
        let (mut scene, mesh, mat) = setup();
        for i in 0..5 {
            scene.spawn(
                format!("i{i}"),
                mesh,
                mat,
                Transform::from_translation(Vec3::splat(i as f32)),
            );
        }
        let before: Vec<Aabb> = scene.instances().map(|(_, i)| i.bounds()).collect();
        scene.update_all_bounds();
        let after: Vec<Aabb> = scene.instances().map(|(_, i)| i.bounds()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn box_instance_helper() {
        let (scene, mesh, mat) = setup();
        let _ = scene;
        let inst = box_instance(
            mesh,
            mat,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(2.0, 4.0, 6.0),
        );
        assert_eq!(inst.bounds.size(), Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(inst.bounds.center(), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn flag_constructors_are_sane() {
        assert!(InstanceFlags::roof().occluder);
        assert!(!InstanceFlags::roof().receive_shadow);
        assert!(InstanceFlags::foliage().occluder);
        assert!(!InstanceFlags::character().occluder);
        assert!(InstanceFlags::marker().always_visible);
        assert!(InstanceFlags::wall().occluder);
    }

    #[test]
    fn transform_on_a_missing_instance_is_reported() {
        let (mut scene, mesh, mat) = setup();
        let h = scene.spawn("a", mesh, mat, Transform::IDENTITY);
        scene.remove_instance(h);
        assert!(!scene.set_transform(h, Transform::IDENTITY));
        assert!(!scene.update_bounds(h));
    }
}
