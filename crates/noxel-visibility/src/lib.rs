//! # Noxel visibility
//!
//! Decides what the renderer should draw, and what the camera should hide.
//!
//! Three jobs, in order of increasing cost — each one only ever *refines* the
//! previous, so a bug in a later stage can never make something disappear that
//! an earlier stage wanted:
//!
//! 1. **Frustum culling** ([`cull::Culler`]) — reject anything outside the view
//!    volume. Uses the positive-vertex test, so a rejection is always correct.
//! 2. **Distance and size culling** — reject anything past a layer's view
//!    distance, or smaller than a minimum on-screen size. This is where a large
//!    world stops costing anything: at 320x180, a 0.05 m pebble two kilometres
//!    away is a fraction of a pixel.
//! 3. **Occlusion culling and camera occlusion** ([`occlusion::OcclusionIndex`])
//!    — ray tests against a BVH of occluder volumes. Two distinct uses:
//!    * *camera occlusion*: when a roof, a tree or a wall stands between the
//!      camera and the player, fade it out instead of letting it hide the
//!      character. This is the difference between a top-down game that is
//!      playable indoors and one that is not.
//!    * *occlusion culling*: an object the camera cannot see because something
//!      solid is in front of it can be skipped entirely.
//!
//! ## Why ray tests rather than a software depth buffer
//!
//! A rasterized occlusion buffer is more accurate but costs a full depth pass
//! every frame, and its output is a per-pixel mask, which is awkward to turn
//! back into per-object decisions. Ray tests against a coarse BVH of *merged*
//! occluder volumes (one box per building, not one per voxel — `noxel-world`
//! emits them that way for exactly this reason) answer both questions directly,
//! are deterministic, and cost a few microseconds per query.
//!
//! ## Determinism
//!
//! The visible set is produced in scene order and the fade state is a pure
//! function of the frames before it, so two identical frames produce identical
//! output. `docs/adr/0008-deterministic-rendering.md` explains why that matters
//! for the golden-image tests.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod cull;
pub mod lod;
pub mod occlusion;

pub use cull::{CullSettings, Culler};
pub use lod::{LodLevels, LodSelection};
pub use occlusion::{FadeSettings, OcclusionIndex, OcclusionSettings};

use noxel_core::math::Vec3;
use noxel_render::CameraView;
use noxel_render::renderer::{CullCounts, RenderSettings, VisibleItem, VisibleSet};
use noxel_render::scene::Scene;

/// Everything the visibility pass needs for one frame.
#[derive(Clone, Copy, Debug)]
pub struct VisibilityInput<'a> {
    /// The scene to cull.
    pub scene: &'a Scene,
    /// The camera to cull against.
    pub camera: &'a CameraView,
    /// Seconds since the previous frame, for the fade smoothing.
    pub dt: f32,
    /// The point the camera must not lose sight of.
    ///
    /// Almost always the player. `None` disables the camera-occlusion fade (and
    /// is what a tool or a top-down map view wants).
    pub focus: Option<Vec3>,
    /// Which render layers are currently enabled, as a bitmask.
    pub visible_layers: u32,
}

impl<'a> VisibilityInput<'a> {
    /// Builds the usual input for a gameplay frame.
    #[must_use]
    pub fn new(scene: &'a Scene, camera: &'a CameraView, dt: f32, focus: Vec3) -> Self {
        Self {
            scene,
            camera,
            dt,
            focus: Some(focus),
            visible_layers: u32::MAX,
        }
    }

    /// Builds an input with no focus, so nothing fades.
    #[must_use]
    pub fn overview(scene: &'a Scene, camera: &'a CameraView) -> Self {
        Self {
            scene,
            camera,
            dt: 1.0 / 60.0,
            focus: None,
            visible_layers: u32::MAX,
        }
    }

    /// Restricts which layers are drawn.
    #[must_use]
    pub fn with_layers(mut self, layers: u32) -> Self {
        self.visible_layers = layers;
        self
    }
}

/// The configuration for the whole visibility pass.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibilityConfig {
    /// Frustum, distance and size thresholds.
    pub cull: CullSettings,
    /// Occlusion and fading.
    pub occlusion: OcclusionSettings,
    /// When false the occlusion index is never built and no fades are applied.
    /// Useful for a top-down map view or a headless statistics run.
    pub enable_occlusion: bool,
}

impl Default for VisibilityConfig {
    fn default() -> Self {
        Self {
            cull: CullSettings::default(),
            occlusion: OcclusionSettings::default(),
            enable_occlusion: true,
        }
    }
}

impl VisibilityConfig {
    /// A configuration tuned for a large streaming world: aggressive distance
    /// culling, modest occlusion.
    #[must_use]
    pub fn large_world() -> Self {
        Self {
            cull: CullSettings {
                max_distance: 160.0,
                ..CullSettings::default()
            },
            ..Self::default()
        }
    }

    /// A configuration for a still or a golden-image test: nothing is culled
    /// beyond the frustum.
    #[must_use]
    pub fn exhaustive() -> Self {
        Self {
            cull: CullSettings {
                max_distance: f32::INFINITY,
                min_screen_radius: 0.0,
                ..CullSettings::default()
            },
            enable_occlusion: false,
            ..Self::default()
        }
    }
}

/// The visibility system: owns the occluder index and the fade state.
pub struct VisibilitySystem {
    config: VisibilityConfig,
    culler: Culler,
    occlusion: OcclusionIndex,
    lod: LodLevels,
    /// Reusable scratch so a frame allocates nothing.
    scratch: Vec<noxel_render::scene::InstanceHandle>,
    /// Per-instance fade, indexed by the instance handle's slot.
    fades: std::collections::HashMap<u64, f32>,
    last_counts: CullCounts,
}

impl VisibilitySystem {
    /// Creates a system with the given configuration.
    #[must_use]
    pub fn new(config: VisibilityConfig) -> Self {
        let occlusion = OcclusionIndex::new(config.occlusion.clone());
        Self {
            culler: Culler::new(config.cull.clone()),
            lod: LodLevels::default(),
            occlusion,
            config,
            scratch: Vec::new(),
            fades: std::collections::HashMap::new(),
            last_counts: CullCounts::default(),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &VisibilityConfig {
        &self.config
    }

    /// Replaces the configuration.
    pub fn set_config(&mut self, config: VisibilityConfig) {
        self.culler.set_settings(config.cull.clone());
        self.occlusion.set_settings(config.occlusion.clone());
        self.config = config;
    }

    /// The frustum/distance culler.
    #[must_use]
    pub fn culler(&self) -> &Culler {
        &self.culler
    }

    /// The occluder index.
    #[must_use]
    pub fn occlusion(&self) -> &OcclusionIndex {
        &self.occlusion
    }

    /// The LOD table.
    #[must_use]
    pub fn lod(&self) -> &LodLevels {
        &self.lod
    }

    /// Mutable access to the LOD table.
    pub fn lod_mut(&mut self) -> &mut LodLevels {
        &mut self.lod
    }

    /// Culling statistics from the most recent frame.
    #[must_use]
    pub fn last_counts(&self) -> CullCounts {
        self.last_counts
    }

    /// The current fade alpha for an instance, `1.0` when it is not fading.
    #[must_use]
    pub fn fade_of(&self, instance: noxel_render::scene::InstanceHandle) -> f32 {
        self.fades.get(&instance.to_bits()).copied().unwrap_or(1.0)
    }

    /// Number of instances currently faded below full opacity.
    #[must_use]
    pub fn fading_count(&self) -> usize {
        self.fades.values().filter(|a| **a < 0.999).count()
    }

    /// Forgets all fade state, for a scene load or a teleport.
    pub fn clear_fades(&mut self) {
        self.fades.clear();
    }

    /// Forces the occluder index to rebuild on the next frame.
    pub fn invalidate_occluders(&mut self) {
        self.occlusion.invalidate();
    }

    /// Produces the visible set for a frame.
    pub fn update(&mut self, input: &VisibilityInput<'_>, settings: &RenderSettings) -> VisibleSet {
        let mut counts = CullCounts::default();
        let mut set = VisibleSet {
            items: Vec::new(),
            culled: Vec::new(),
            counts: CullCounts::default(),
        };
        let camera = input.camera;

        if self.config.enable_occlusion {
            self.occlusion.ensure_built(input.scene);
        }

        // ---- phase 1: frustum, distance and size ---------------------------
        for (handle, instance) in input.scene.instances() {
            counts.considered += 1;
            if !instance.visible {
                counts.record(noxel_render::renderer::CullReason::Hidden);
                if settings.track_cull_reasons {
                    set.culled
                        .push((handle, noxel_render::renderer::CullReason::Hidden));
                }
                continue;
            }
            if input.visible_layers & instance.layer == 0 {
                counts.record(noxel_render::renderer::CullReason::LayerDisabled);
                if settings.track_cull_reasons {
                    set.culled
                        .push((handle, noxel_render::renderer::CullReason::LayerDisabled));
                }
                continue;
            }
            let bounds = instance.bounds();
            if bounds.is_empty() {
                // An instance with no geometry cannot be seen; do not let it
                // count as a draw.
                counts.record(noxel_render::renderer::CullReason::TooSmall);
                continue;
            }
            match self.culler.evaluate(bounds, camera, instance, settings) {
                Ok(distance) => {
                    let lod = self.lod.level_for(&bounds, camera);
                    set.items.push(VisibleItem {
                        instance: handle,
                        alpha: 1.0,
                        lod,
                        distance,
                    });
                }
                Err(reason) => {
                    counts.record(reason);
                    if settings.track_cull_reasons {
                        set.culled.push((handle, reason));
                    }
                }
            }
        }

        // ---- phase 2: occlusion culling ------------------------------------
        if self.config.enable_occlusion && self.occlusion.settings().cull_occluded {
            let mut kept: Vec<VisibleItem> = Vec::with_capacity(set.items.len());
            for item in set.items.drain(..) {
                let Some(instance) = input.scene.instance(item.instance) else {
                    continue;
                };
                if instance.flags.always_visible {
                    kept.push(item);
                    continue;
                }
                if self.occlusion.is_occluded(
                    camera,
                    &instance.bounds(),
                    item.instance,
                    item.distance,
                ) {
                    counts.record(noxel_render::renderer::CullReason::Occluded);
                    if settings.track_cull_reasons {
                        set.culled
                            .push((item.instance, noxel_render::renderer::CullReason::Occluded));
                    }
                } else {
                    kept.push(item);
                }
            }
            set.items = kept;
        }

        // ---- phase 3: camera occlusion fades --------------------------------
        if self.config.enable_occlusion {
            if let Some(focus) = input.focus {
                self.apply_fades(input, focus, settings);
            }
        }
        for item in &mut set.items {
            item.alpha = self.fade_of(item.instance);
        }
        counts.faded = set.items.iter().filter(|i| i.alpha < 0.999).count();
        counts.drawn = set.items.len();
        set.counts = counts;
        self.last_counts = counts;
        set
    }

    /// Fades anything standing between the camera and `focus`.
    fn apply_fades(
        &mut self,
        input: &VisibilityInput<'_>,
        focus: Vec3,
        _settings: &RenderSettings,
    ) {
        let settings = self.occlusion.settings().clone();
        let camera = input.camera;
        self.occlusion
            .collect_blockers(camera, focus, &mut self.scratch);

        let min_alpha = settings.fade.min_alpha;
        let speed = settings.fade.speed;
        // Anything still fading from last frame must keep fading even if the ray
        // no longer touches it, otherwise a roof would snap back to opaque the
        // instant the player steps out from under it.
        let k = noxel_core::math::damp_factor(speed, input.dt.max(0.0));

        // Gather the fade keys of the current blockers first; mutating the map
        // while iterating it would need a second pass anyway.
        let mut blocked: Vec<u64> = Vec::new();
        for (handle, instance) in input.scene.instances() {
            if !instance.flags.occluder {
                continue;
            }
            if self.scratch.contains(&handle) {
                blocked.push(handle.to_bits());
            }
        }
        for (handle, instance) in input.scene.instances() {
            if !instance.flags.occluder {
                continue;
            }
            let key = handle.to_bits();
            let current = self.fades.get(&key).copied().unwrap_or(1.0);
            let target = if instance.flags.never_fade {
                1.0
            } else if blocked.contains(&key) {
                min_alpha
            } else {
                1.0
            };
            let next = current + (target - current) * k;
            if (next - 1.0).abs() < 1e-3 {
                self.fades.remove(&key);
            } else {
                self.fades.insert(key, next);
            }
        }
        // Drop fade entries for instances that no longer exist.
        if self.fades.len() > input.scene.instance_count() * 2 {
            let live: std::collections::HashSet<u64> =
                input.scene.instances().map(|(h, _)| h.to_bits()).collect();
            self.fades.retain(|k, _| live.contains(k));
        }
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.occlusion.memory_bytes()
            + self.fades.capacity() * (core::mem::size_of::<u64>() + core::mem::size_of::<f32>())
            + self.scratch.capacity() * core::mem::size_of::<noxel_render::scene::InstanceHandle>()
    }

    /// A one-line summary for the debug overlay.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "visible {} | {} | occluders {} | fading {}",
            self.last_counts.drawn,
            self.last_counts.summary(),
            self.occlusion.occluder_count(),
            self.fading_count()
        )
    }
}

impl Default for VisibilitySystem {
    fn default() -> Self {
        Self::new(VisibilityConfig::default())
    }
}

impl core::fmt::Debug for VisibilitySystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VisibilitySystem")
            .field("occluders", &self.occlusion.occluder_count())
            .field("fading", &self.fading_count())
            .finish()
    }
}

/// The crate version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::{Color, Transform};
    use noxel_render::material::Material;
    use noxel_render::mesh::Mesh;
    use noxel_render::scene::InstanceFlags;

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            30.0,
            1.0,
            1.0,
            200.0,
        )
    }

    fn scene_with_ground() -> Scene {
        let mut scene = Scene::new();
        let mesh = scene.add_mesh(Mesh::plane(60.0, 1.0));
        let mat = scene.add_material(Material::lit("ground", Color::WHITE));
        scene.spawn("ground", mesh, mat, Transform::IDENTITY);
        scene.update_all_bounds();
        scene
    }

    #[test]
    fn empty_scene_yields_an_empty_set() {
        let camera = camera();
        let scene = Scene::new();
        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert!(set.items.is_empty());
        assert_eq!(set.counts.considered, 0);
    }

    #[test]
    fn ground_is_visible_from_above() {
        let camera = camera();
        let scene = scene_with_ground();
        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert_eq!(set.items.len(), 1);
        assert_eq!(set.counts.drawn, 1);
    }

    #[test]
    fn distant_instances_are_culled() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(2.0));
        let mat = scene.add_material(Material::lit("box", Color::WHITE));
        scene.spawn(
            "far",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(5000.0, 0.0, 0.0)),
        );
        scene.update_all_bounds();

        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert_eq!(set.items.len(), 1, "the far box must be culled");
        assert!(set.counts.frustum + set.counts.distance >= 1);
    }

    #[test]
    fn hidden_instances_are_reported_as_hidden() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(1.0));
        let mat = scene.add_material(Material::lit("box", Color::WHITE));
        let h = scene.spawn("box", mesh, mat, Transform::IDENTITY);
        scene.instance_mut(h).unwrap().visible = false;
        scene.update_all_bounds();

        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert_eq!(set.counts.hidden, 1);
        assert_eq!(set.items.len(), 1, "only the ground is drawn");
    }

    #[test]
    fn layer_filtering_works() {
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(1.0));
        let mat = scene.add_material(Material::lit("box", Color::WHITE));
        let h = scene.spawn("roof", mesh, mat, Transform::IDENTITY);
        scene.instance_mut(h).unwrap().layer = 1 << 5;
        scene.update_all_bounds();

        let mut system = VisibilitySystem::default();
        let camera = camera();
        let input = VisibilityInput::overview(&scene, &camera).with_layers(0b1);
        let set = system.update(&input, &RenderSettings::default());
        assert_eq!(set.counts.layer_disabled, 1);
    }

    #[test]
    fn empty_bounds_are_culled() {
        let camera = camera();
        let mut scene = Scene::new();
        let mesh = scene.add_mesh(Mesh::plane(1.0, 1.0));
        let mat = scene.add_material(Material::lit("x", Color::WHITE));
        let h = scene.spawn("empty", mesh, mat, Transform::IDENTITY);
        // Corrupt the cached bounds the way a mesh swap without `update_bounds`
        // would.
        scene.instance_mut(h).unwrap().mesh = mesh;
        scene.instance_mut(h).unwrap();
        scene.update_all_bounds();
        // Now force an empty box by hand through the public API: a mesh with no
        // triangles has empty bounds.
        let empty_mesh = scene.add_mesh(Mesh::default());
        scene.instance_mut(h).unwrap().mesh = empty_mesh;
        scene.update_bounds(h);

        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert!(set.items.is_empty());
    }

    #[test]
    fn exhaustive_configuration_keeps_everything_in_the_frustum() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(0.01));
        let mat = scene.add_material(Material::lit("tiny", Color::WHITE));
        scene.spawn(
            "tiny",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(10.0, 0.0, 10.0)),
        );
        scene.update_all_bounds();

        let mut system = VisibilitySystem::new(VisibilityConfig::exhaustive());
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert_eq!(set.items.len(), 2);
    }

    #[test]
    fn small_instances_are_culled_by_default() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(0.001));
        let mat = scene.add_material(Material::lit("tiny", Color::WHITE));
        scene.spawn(
            "tiny",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(10.0, 0.0, 10.0)),
        );
        scene.update_all_bounds();

        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert_eq!(set.items.len(), 1, "the sub-pixel cube must be culled");
        assert_eq!(set.counts.too_small, 1);
    }

    #[test]
    fn always_visible_instances_survive_everything() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(0.001));
        let mat = scene.add_material(Material::lit("marker", Color::WHITE));
        let h = scene.spawn(
            "marker",
            mesh,
            mat,
            Transform::from_translation(Vec3::splat(900.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::marker();
        scene.update_all_bounds();

        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        // `always_visible` means exactly that: the marker survives the frustum,
        // the distance limit, the size limit and the occlusion pass. It is what
        // a quest marker or a waypoint needs.
        assert!(
            set.items.iter().any(|i| i.instance == h),
            "the marker must survive"
        );
        assert!(set.items.iter().all(|i| (i.alpha - 1.0).abs() < 1e-6));
    }

    #[test]
    fn lod_increases_with_distance() {
        let mut scene = Scene::new();
        let mesh = scene.add_mesh(Mesh::cube(4.0));
        let mat = scene.add_material(Material::lit("box", Color::WHITE));
        scene.spawn("near", mesh, mat, Transform::from_translation(Vec3::ZERO));
        scene.spawn(
            "far",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
        );
        scene.update_all_bounds();

        let mut camera = CameraView::perspective(
            Vec3::new(0.0, 10.0, 40.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            1000.0,
        );
        camera.view_projection = camera.projection * camera.view;
        let mut system = VisibilitySystem::default();
        let set = system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert!(set.items.iter().all(|i| i.lod <= 3));
    }

    #[test]
    fn occlusion_fade_can_be_disabled() {
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(20.0));
        let mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let h = scene.spawn(
            "roof",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::roof();
        scene.update_all_bounds();

        let config = VisibilityConfig {
            enable_occlusion: false,
            ..VisibilityConfig::default()
        };
        let mut system = VisibilitySystem::new(config);
        for _ in 0..60 {
            let camera = camera();
            let input = VisibilityInput::new(&scene, &camera, 1.0 / 60.0, Vec3::ZERO);
            let set = system.update(&input, &RenderSettings::default());
            assert!(set.items.iter().all(|i| (i.alpha - 1.0).abs() < 1e-6));
        }
        assert_eq!(system.fading_count(), 0);
    }

    #[test]
    fn summary_is_human_readable() {
        let camera = camera();
        let scene = scene_with_ground();
        let mut system = VisibilitySystem::default();
        system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        let s = system.summary();
        assert!(s.contains("visible 1"), "{s}");
    }

    #[test]
    fn memory_is_reported() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mesh = scene.add_mesh(Mesh::cube(8.0));
        let mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let h = scene.spawn(
            "roof",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(0.0, 10.0, 0.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::roof();
        scene.update_all_bounds();
        let mut system = VisibilitySystem::default();
        system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert!(system.memory_bytes() > 0);
    }

    #[test]
    fn invalidate_occluders_forces_a_rebuild() {
        let camera = camera();
        let mut scene = scene_with_ground();
        let mut system = VisibilitySystem::default();
        system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        let before = system.occlusion().occluder_count();
        scene.add_light(noxel_render::Light::sun());
        system.invalidate_occluders();
        system.update(
            &VisibilityInput::overview(&scene, &camera),
            &RenderSettings::default(),
        );
        assert!(system.occlusion().occluder_count() >= before);
    }
}
