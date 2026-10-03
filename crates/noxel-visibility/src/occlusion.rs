//! Occlusion: the occluder index, the camera-view fade, and occlusion culling.
//!
//! # The two questions
//!
//! *Does this object hide the player?* — cast a thin bundle of rays from the
//! camera to the player and collect the occluder volumes they cross. Those get
//! faded out.
//!
//! *Is this object hidden from the camera?* — cast a ray from the camera to the
//! object's centre. If something solid is in the way, the object can be skipped.
//!
//! Both run against the same triangle-BVH-style structure: a
//! [`noxel_core::spatial::Bvh`] over **merged occluder volumes**, one box per
//! building or tree rather than one per triangle. That keeps the query cost flat
//! as the art gets more detailed, which is the whole point — a town of 400
//! buildings is 400 boxes, and a ray against them takes a couple of
//! microseconds.
//!
//! # Why merged volumes rather than triangles
//!
//! Fading a *triangle* is meaningless: you would get half a roof. The unit of
//! fade has to be the object, so the acceleration structure has to be indexed by
//! object. `noxel-world` emits one merged AABB per building storey-run and per
//! tree canopy specifically so this stays cheap.
//!
//! # Pop-free fading
//!
//! The fade is a first-order smoothing towards the target alpha, so a roof that
//! stops blocking does not snap back to opaque; it fades in over a few frames.
//! A hard cut is the most noticeable artefact a top-down camera can have.

use std::collections::HashMap;

use noxel_core::math::{Aabb, Ray, Vec3};
use noxel_core::spatial::Bvh;
use noxel_render::CameraView;
use noxel_render::scene::{InstanceHandle, Scene};

/// One occluder volume in the index.
#[derive(Clone, Copy, Debug)]
pub struct OccluderEntry {
    /// Slot index of the instance this box belongs to.
    pub instance_index: u32,
    /// The instance's stable bit pattern, for the fade map.
    pub instance_bits: u64,
    /// The volume.
    pub bounds: Aabb,
    /// True when this occluder must never be faded (a solid wall in a tight
    /// interior, where the camera moves instead).
    pub never_fade: bool,
}

/// Configuration for the occlusion pass.
#[derive(Clone, Debug, PartialEq)]
pub struct OcclusionSettings {
    /// Skip building the index and answering any query.
    pub enabled: bool,
    /// Radius, in metres, of the ray bundle cast at the focus.
    ///
    /// A single ray is enough for a point, but a character is a box: with a
    /// radius of half the player's width, a roof that covers only one shoulder
    /// still fades.
    pub focus_radius: f32,
    /// How many rays the bundle uses: one centre ray plus four at the corners
    /// of a square of side `focus_radius`.
    pub focus_rays: u32,
    /// True to cull instances that something solid hides.
    pub cull_occluded: bool,
    /// Only instances whose bounding sphere is smaller than this many pixels are
    /// considered for occlusion culling. Large objects are cheap to draw
    /// relative to their screen coverage and popping them out is very visible.
    pub cull_max_screen_radius: f32,
    /// Viewport height used to convert world size to pixels.
    pub viewport_height: f32,
    /// Fade behaviour.
    pub fade: FadeSettings,
    /// Extra distance added to a ray's length so an occluder exactly at the
    /// focus does not flicker in and out.
    pub ray_margin: f32,
}

impl Default for OcclusionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            focus_radius: 0.45,
            focus_rays: 5,
            cull_occluded: true,
            cull_max_screen_radius: 24.0,
            viewport_height: 180.0,
            fade: FadeSettings::default(),
            ray_margin: 0.05,
        }
    }
}

impl OcclusionSettings {
    /// A configuration that only fades, and never culls.
    ///
    /// The safe default for a game that is still being built: occlusion culling
    /// is the stage most likely to produce a "why is that missing?" bug, so it is
    /// worth turning on deliberately once the world is stable.
    #[must_use]
    pub fn fade_only() -> Self {
        Self {
            cull_occluded: false,
            ..Self::default()
        }
    }
}

/// How the camera-view fade behaves.
#[derive(Clone, Debug, PartialEq)]
pub struct FadeSettings {
    /// The alpha an occluding object fades down to.
    pub min_alpha: f32,
    /// Fraction of the remaining alpha distance left after one second.
    pub speed: f32,
    /// Fade the object rather than making it fully invisible.
    pub enabled: bool,
}

impl Default for FadeSettings {
    fn default() -> Self {
        Self {
            // 0.25 rather than 0: a faint outline of the roof still reads as a
            // roof, which keeps the player oriented. Fully hidden geometry in a
            // top-down game is disorienting.
            min_alpha: 0.25,
            speed: 0.02,
            enabled: true,
        }
    }
}

/// The occluder index and the queries built on it.
pub struct OcclusionIndex {
    settings: OcclusionSettings,
    bvh: Bvh<OccluderEntry>,
    built_revision: u64,
    built_count: usize,
    /// Scratch for the ray bundle's hits, reused between queries. Stores the
    /// full instance bit pattern so the caller gets an exact handle back.
    hit_bits: Vec<u64>,
}

impl OcclusionIndex {
    /// Creates an empty index.
    #[must_use]
    pub fn new(settings: OcclusionSettings) -> Self {
        Self {
            settings,
            bvh: Bvh::empty(),
            built_revision: 0,
            built_count: 0,
            hit_bits: Vec::new(),
        }
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> &OcclusionSettings {
        &self.settings
    }

    /// Replaces the settings.
    pub fn set_settings(&mut self, settings: OcclusionSettings) {
        self.settings = settings;
    }

    /// Forces a rebuild on the next query.
    pub fn invalidate(&mut self) {
        self.built_revision = u64::MAX;
    }

    /// Number of occluder volumes.
    #[must_use]
    pub fn occluder_count(&self) -> usize {
        self.built_count
    }

    /// True when the index was built from `scene`'s current revision.
    #[must_use]
    pub fn is_current(&self, scene: &Scene) -> bool {
        self.built_revision == scene.revision()
    }

    /// Rebuilds the index when the scene changed.
    ///
    /// Rebuilding is `O(n log n)` over the occluder count and only happens when
    /// the scene revision moves, so a static world pays it once.
    pub fn ensure_built(&mut self, scene: &Scene) {
        if !self.settings.enabled {
            return;
        }
        if self.built_revision == scene.revision() {
            return;
        }
        let mut entries: Vec<OccluderEntry> = Vec::new();
        for (handle, instance) in scene.instances() {
            if !instance.visible || !instance.flags.occluder {
                continue;
            }
            let bounds = instance.bounds();
            if bounds.is_empty() {
                continue;
            }
            entries.push(OccluderEntry {
                instance_index: handle.index(),
                instance_bits: handle.to_bits(),
                bounds,
                never_fade: instance.flags.never_fade,
            });
        }
        self.built_count = entries.len();
        self.bvh = Bvh::build(entries.into_iter().map(|e| (e.bounds, e)));
        self.built_revision = scene.revision();
    }

    /// Collects the slot indices of every occluder standing between the camera
    /// and `focus`.
    ///
    /// The result is written into `out` (cleared first) as instance slot
    /// indices, sorted and deduplicated so the fade pass is deterministic.
    pub fn collect_blockers(
        &mut self,
        camera: &CameraView,
        focus: Vec3,
        out: &mut Vec<InstanceHandle>,
    ) {
        // Reuse `blockers` internally and translate at the end, so the public
        // signature does not force the caller to know about slots.
        out.clear();
        if !self.settings.enabled || self.bvh.is_empty() {
            return;
        }
        let origin = camera.position;
        // Aim slightly above the focus so a low wall does not fade for a tall
        // character, and so the ray does not graze the ground.
        let target = focus + Vec3::Y * 0.4;
        let margin = self.settings.ray_margin;

        let hit_bits = &mut self.hit_bits;
        hit_bits.clear();
        let radius = self.settings.focus_radius.max(0.0);
        let axis_a = camera.right.normalize_or_zero() * radius;
        let axis_b = camera.up.normalize_or_zero() * radius;
        let rays = self.settings.focus_rays.clamp(1, 9);
        let offsets: [(f32, f32); 5] = [
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
        ];
        for (i, (a, b)) in offsets.iter().enumerate() {
            if i as u32 >= rays {
                break;
            }
            let from = origin + axis_a * *a + axis_b * *b;
            let to = target + axis_a * *a * 0.25 + axis_b * *b * 0.25;
            let direction = to - from;
            let length = direction.length() + margin;
            if length < 1e-4 {
                continue;
            }
            let ray = Ray::with_max_t(from, direction, length);
            let mut hits: Vec<noxel_core::spatial::BvhRayHit<'_, OccluderEntry>> = Vec::new();
            self.bvh.query_ray(&ray, &mut hits);
            for hit in hits {
                hit_bits.push(hit.value.instance_bits);
            }
        }
        hit_bits.sort_unstable();
        hit_bits.dedup();
        for bits in hit_bits.iter().copied() {
            out.push(InstanceHandle::from_bits(bits));
        }
    }

    /// True when something solid hides `bounds` from the camera.
    ///
    /// `self_index` is excluded so an object cannot occlude itself.
    #[must_use]
    pub fn is_occluded(
        &self,
        camera: &CameraView,
        bounds: &Aabb,
        self_index: InstanceHandle,
        distance: f32,
    ) -> bool {
        if !self.settings.enabled || !self.settings.cull_occluded || self.bvh.is_empty() {
            return false;
        }
        // Only bother with small objects: culling a large one saves little and
        // pops visibly.
        if self.settings.cull_max_screen_radius > 0.0 {
            let radius = crate::cull::Culler::screen_radius(
                camera,
                bounds,
                distance,
                self.settings.viewport_height,
            );
            if radius > self.settings.cull_max_screen_radius {
                return false;
            }
        }
        let centre = bounds.center();
        let target = self_index.index();
        // Stop just short of the object's near surface, so a box touching a wall
        // does not occlude itself through the shared boundary.
        let direction = centre - camera.position;
        let length = direction.length();
        if length < 1e-4 {
            return false;
        }
        let reach = (length - bounds.bounding_sphere_radius() * 0.5).max(0.0);
        if reach < 1e-3 {
            return false;
        }
        let ray = Ray::with_max_t(camera.position, direction, reach);
        self.bvh
            .any_hit_segment(ray.origin, ray.at(reach), |entry| {
                entry.instance_index == target
            })
    }

    /// Every occluder volume overlapping `region`.
    pub fn occluders_in(&self, region: &Aabb, out: &mut Vec<OccluderEntry>) {
        out.clear();
        let mut hits: Vec<&OccluderEntry> = Vec::new();
        self.bvh.query_aabb(region, &mut hits);
        out.extend(hits.into_iter().copied());
    }

    /// The world bounds of every occluder, for the debug overlay.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.bvh.bounds()
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.bvh.memory_bytes()
    }

    /// Reads the internal hit scratch list, for tests.
    #[must_use]
    pub fn debug_hit_bits(&self) -> &[u64] {
        &self.hit_bits
    }
}

impl core::fmt::Debug for OcclusionIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OcclusionIndex")
            .field("occluders", &self.built_count)
            .field("built_revision", &self.built_revision)
            .field("cull_occluded", &self.settings.cull_occluded)
            .finish()
    }
}

impl Default for OcclusionIndex {
    fn default() -> Self {
        Self::new(OcclusionSettings::default())
    }
}

/// A per-instance fade table.
///
/// Kept separate from [`OcclusionIndex`] so a game can drive fades from its own
/// logic (a scripted cutscene, a stealth effect) without going through the ray
/// tests.
#[derive(Clone, Debug, Default)]
pub struct FadeTable {
    alphas: HashMap<u64, f32>,
}

impl FadeTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The alpha for an instance, `1.0` when it is not fading.
    #[must_use]
    pub fn alpha(&self, instance: InstanceHandle) -> f32 {
        self.alphas.get(&instance.to_bits()).copied().unwrap_or(1.0)
    }

    /// Sets a target alpha and advances the smoothing by `dt`.
    pub fn step(
        &mut self,
        instance: InstanceHandle,
        target: f32,
        dt: f32,
        settings: &FadeSettings,
    ) {
        let key = instance.to_bits();
        let current = self.alphas.get(&key).copied().unwrap_or(1.0);
        let target = if settings.enabled {
            target.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let k = noxel_core::math::damp_factor(settings.speed, dt.max(0.0));
        let next = current + (target - current) * k;
        if (next - 1.0).abs() < 1e-3 {
            self.alphas.remove(&key);
        } else {
            self.alphas.insert(key, next);
        }
    }

    /// Number of instances currently below full opacity.
    #[must_use]
    pub fn len(&self) -> usize {
        self.alphas.len()
    }

    /// True when nothing is fading.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.alphas.is_empty()
    }

    /// Clears every fade.
    pub fn clear(&mut self) {
        self.alphas.clear();
    }
}

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

    fn scene_with_roof() -> (Scene, InstanceHandle) {
        let mut scene = Scene::new();
        let ground = scene.add_mesh(Mesh::plane(60.0, 1.0));
        let ground_mat = scene.add_material(Material::lit("ground", Color::WHITE));
        scene.spawn("ground", ground, ground_mat, Transform::IDENTITY);

        // A roof between the camera (directly above) and the focus.
        let roof = scene.add_mesh(Mesh::cuboid(Vec3::new(6.0, 0.3, 6.0)));
        let roof_mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let h = scene.spawn(
            "roof",
            roof,
            roof_mat,
            Transform::from_translation(Vec3::new(0.0, 12.0, 0.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::roof();
        scene.update_all_bounds();
        (scene, h)
    }

    #[test]
    fn index_is_built_only_from_occluders() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        assert_eq!(index.occluder_count(), 1, "the ground is not an occluder");
        assert!(!index.bvh.is_empty());
    }

    #[test]
    fn index_is_reused_when_the_scene_does_not_change() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        assert!(index.is_current(&scene));
        let before = index.memory_bytes();
        index.ensure_built(&scene);
        assert_eq!(index.memory_bytes(), before);
    }

    #[test]
    fn index_rebuilds_after_a_change() {
        let (mut scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let mesh = scene.add_mesh(Mesh::cuboid(Vec3::splat(2.0)));
        let mat = scene.add_material(Material::lit("tree", Color::WHITE));
        let h = scene.spawn(
            "tree",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(10.0, 3.0, 0.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::foliage();
        scene.update_all_bounds();
        index.ensure_built(&scene);
        assert_eq!(index.occluder_count(), 2);
    }

    #[test]
    fn invalidate_forces_a_rebuild() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        index.invalidate();
        assert!(!index.is_current(&scene));
    }

    #[test]
    fn roof_between_camera_and_focus_is_collected() {
        let (scene, roof) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let mut blockers = Vec::new();
        index.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(blockers.contains(&roof), "{blockers:?}");
    }

    #[test]
    fn nothing_is_collected_without_an_occluder_in_the_way() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let mut blockers = Vec::new();
        // Far away from the roof.
        index.collect_blockers(&camera(), Vec3::new(25.0, 0.0, 25.0), &mut blockers);
        assert!(blockers.is_empty(), "{blockers:?}");
    }

    #[test]
    fn blockers_are_deduplicated() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let mut blockers = Vec::new();
        index.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        let mut sorted = blockers.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            blockers.len(),
            sorted.len(),
            "a five-ray bundle must not report duplicates"
        );
    }

    #[test]
    fn a_single_ray_bundle_still_works() {
        let (scene, roof) = scene_with_roof();
        let mut index = OcclusionIndex::new(OcclusionSettings {
            focus_rays: 1,
            ..Default::default()
        });
        index.ensure_built(&scene);
        let mut blockers = Vec::new();
        index.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(blockers.contains(&roof));
    }

    #[test]
    fn ray_radius_widens_the_bundle() {
        // An occluder covering only one side of the focus: a wide bundle finds
        // it, a zero-radius one does not.
        let mut scene = Scene::new();
        let mesh = scene.add_mesh(Mesh::cuboid(Vec3::new(2.25, 0.3, 2.0)));
        let mat = scene.add_material(Material::lit("roof", noxel_core::math::Color::WHITE));
        // The canopy spans x in [0.5, 5.0]. The centre ray runs straight down
        // x = 0 and misses it; the bundle's corner rays start 2 m to either side
        // and converge towards the focus at 0.25 of that offset, so at canopy
        // height they pass through x ~ +-0.94 - inside the box.
        let h = scene.spawn(
            "roof",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(2.75, 12.0, 0.0)),
        );
        scene.instance_mut(h).unwrap().flags = InstanceFlags::roof();
        scene.update_all_bounds();

        let mut wide = OcclusionIndex::new(OcclusionSettings {
            focus_radius: 2.0,
            ..Default::default()
        });
        wide.ensure_built(&scene);
        let mut blockers = Vec::new();
        wide.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(
            blockers.contains(&h),
            "a wide bundle must catch an off-centre occluder"
        );

        let mut narrow = OcclusionIndex::new(OcclusionSettings {
            focus_radius: 0.0,
            ..Default::default()
        });
        narrow.ensure_built(&scene);
        blockers.clear();
        narrow.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(!blockers.contains(&h), "a zero-radius bundle must miss it");
    }

    #[test]
    fn disabled_index_reports_nothing() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::new(OcclusionSettings {
            enabled: false,
            ..Default::default()
        });
        index.ensure_built(&scene);
        assert_eq!(index.occluder_count(), 0);
        let mut blockers = Vec::new();
        index.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(blockers.is_empty());
    }

    #[test]
    fn is_occluded_detects_a_hidden_small_object() {
        let mut scene = Scene::new();
        // A big roof slab with a small prop directly beneath it.
        let roof = scene.add_mesh(Mesh::cuboid(Vec3::new(10.0, 0.2, 10.0)));
        let mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let roof_h = scene.spawn(
            "roof",
            roof,
            mat,
            Transform::from_translation(Vec3::new(0.0, 12.0, 0.0)),
        );
        scene.instance_mut(roof_h).unwrap().flags = InstanceFlags::roof();

        let prop = scene.add_mesh(Mesh::cube(0.4));
        let prop_mat = scene.add_material(Material::lit("prop", Color::WHITE));
        let prop_h = scene.spawn(
            "prop",
            prop,
            prop_mat,
            Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
        );
        scene.update_all_bounds();

        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let bounds = scene.instance(prop_h).unwrap().bounds();
        let distance = camera().position.distance(bounds.center());
        assert!(index.is_occluded(&camera(), &bounds, prop_h, distance));
    }

    #[test]
    fn is_occluded_is_false_without_a_blocker() {
        let mut scene = Scene::new();
        let prop = scene.add_mesh(Mesh::cube(0.4));
        let mat = scene.add_material(Material::lit("prop", Color::WHITE));
        let h = scene.spawn(
            "prop",
            prop,
            mat,
            Transform::from_translation(Vec3::new(20.0, 0.5, 20.0)),
        );
        scene.update_all_bounds();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let bounds = scene.instance(h).unwrap().bounds();
        assert!(!index.is_occluded(&camera(), &bounds, h, 40.0));
    }

    #[test]
    fn is_occluded_never_reports_self_occlusion() {
        let (scene, roof) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let bounds = scene.instance(roof).unwrap().bounds();
        assert!(
            !index.is_occluded(&camera(), &bounds, roof, 28.0),
            "an object must not occlude itself"
        );
        // Even with a large threshold, so the screen-size guard is bypassed.
        let mut index = OcclusionIndex::new(OcclusionSettings {
            cull_max_screen_radius: f32::INFINITY,
            ..Default::default()
        });
        index.ensure_built(&scene);
        assert!(!index.is_occluded(&camera(), &bounds, roof, 28.0));
    }

    #[test]
    fn large_objects_are_not_occlusion_culled() {
        let mut scene = Scene::new();
        let roof = scene.add_mesh(Mesh::cuboid(Vec3::new(20.0, 0.2, 20.0)));
        let mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let roof_h = scene.spawn(
            "roof",
            roof,
            mat,
            Transform::from_translation(Vec3::new(0.0, 14.0, 0.0)),
        );
        scene.instance_mut(roof_h).unwrap().flags = InstanceFlags::roof();

        let big = scene.add_mesh(Mesh::cube(6.0));
        let big_mat = scene.add_material(Material::lit("big", Color::WHITE));
        let big_h = scene.spawn(
            "big",
            big,
            big_mat,
            Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
        );
        scene.update_all_bounds();

        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let bounds = scene.instance(big_h).unwrap().bounds();
        assert!(
            !index.is_occluded(&camera(), &bounds, big_h, 40.0),
            "a large object must not pop out"
        );
    }

    #[test]
    fn occlusion_culling_can_be_disabled() {
        let mut scene = Scene::new();
        let roof = scene.add_mesh(Mesh::cuboid(Vec3::new(10.0, 0.2, 10.0)));
        let mat = scene.add_material(Material::lit("roof", Color::WHITE));
        let roof_h = scene.spawn(
            "roof",
            roof,
            mat,
            Transform::from_translation(Vec3::new(0.0, 12.0, 0.0)),
        );
        scene.instance_mut(roof_h).unwrap().flags = InstanceFlags::roof();
        let prop = scene.add_mesh(Mesh::cube(0.4));
        let prop_mat = scene.add_material(Material::lit("prop", Color::WHITE));
        let prop_h = scene.spawn(
            "prop",
            prop,
            prop_mat,
            Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
        );
        scene.update_all_bounds();

        let mut index = OcclusionIndex::new(OcclusionSettings::fade_only());
        index.ensure_built(&scene);
        let bounds = scene.instance(prop_h).unwrap().bounds();
        assert!(!index.is_occluded(&camera(), &bounds, prop_h, 40.0));
    }

    #[test]
    fn occluders_in_a_region() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let mut out = Vec::new();
        index.occluders_in(&Aabb::new(Vec3::splat(-20.0), Vec3::splat(20.0)), &mut out);
        assert_eq!(out.len(), 1);
        index.occluders_in(&Aabb::new(Vec3::splat(200.0), Vec3::splat(210.0)), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn index_bounds_cover_the_occluders() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        let b = index.bounds();
        assert!(b.contains_point(Vec3::new(0.0, 12.0, 0.0)));
    }

    #[test]
    fn empty_scene_is_safe() {
        let scene = Scene::new();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        assert_eq!(index.occluder_count(), 0);
        let mut blockers = Vec::new();
        index.collect_blockers(&camera(), Vec3::ZERO, &mut blockers);
        assert!(blockers.is_empty());
        assert!(!index.is_occluded(
            &camera(),
            &Aabb::new(Vec3::ZERO, Vec3::ONE),
            InstanceHandle::INVALID,
            1.0
        ));
    }

    #[test]
    fn fade_table_smooths_towards_the_target() {
        let settings = FadeSettings {
            min_alpha: 0.25,
            speed: 0.02,
            enabled: true,
        };
        let mut table = FadeTable::new();
        let h = InstanceHandle::from_bits(1);
        assert_eq!(table.alpha(h), 1.0);
        for _ in 0..180 {
            table.step(h, 0.25, 1.0 / 60.0, &settings);
        }
        assert!((table.alpha(h) - 0.25).abs() < 0.02, "{}", table.alpha(h));
        assert_eq!(table.len(), 1);
        // And it recovers.
        for _ in 0..180 {
            table.step(h, 1.0, 1.0 / 60.0, &settings);
        }
        assert!(
            table.is_empty(),
            "a fully opaque instance must not occupy the table"
        );
    }

    #[test]
    fn fade_is_frame_rate_independent() {
        let settings = FadeSettings {
            min_alpha: 0.0,
            speed: 0.05,
            enabled: true,
        };
        let h = InstanceHandle::from_bits(2);
        let mut slow = FadeTable::new();
        let mut fast = FadeTable::new();
        for _ in 0..30 {
            slow.step(h, 0.0, 1.0 / 30.0, &settings);
        }
        for _ in 0..60 {
            fast.step(h, 0.0, 1.0 / 60.0, &settings);
        }
        assert!(
            (slow.alpha(h) - fast.alpha(h)).abs() < 1e-3,
            "{} vs {}",
            slow.alpha(h),
            fast.alpha(h)
        );
    }

    #[test]
    fn fade_disabled_stays_opaque() {
        let settings = FadeSettings {
            enabled: false,
            ..Default::default()
        };
        let mut table = FadeTable::new();
        let h = InstanceHandle::from_bits(3);
        for _ in 0..60 {
            table.step(h, 0.0, 1.0 / 60.0, &settings);
        }
        assert_eq!(table.alpha(h), 1.0);
        assert!(table.is_empty());
    }

    #[test]
    fn fade_clear_resets() {
        let mut table = FadeTable::new();
        let h = InstanceHandle::from_bits(4);
        table.step(h, 0.0, 1.0, &FadeSettings::default());
        assert!(!table.is_empty());
        table.clear();
        assert!(table.is_empty());
        assert_eq!(table.alpha(h), 1.0);
    }

    #[test]
    fn fade_target_is_clamped() {
        let mut table = FadeTable::new();
        let h = InstanceHandle::from_bits(5);
        table.step(h, 5.0, 1.0, &FadeSettings::default());
        assert_eq!(table.alpha(h), 1.0);
        table.step(h, -3.0, 1.0, &FadeSettings::default());
        assert!(table.alpha(h) >= 0.0);
    }

    #[test]
    fn index_memory_is_reported() {
        let (scene, _) = scene_with_roof();
        let mut index = OcclusionIndex::default();
        index.ensure_built(&scene);
        assert!(index.memory_bytes() > 0);
        assert!(format!("{index:?}").contains("occluders"));
    }

    #[test]
    fn debug_hit_bits_is_exposed_for_tests() {
        let index = OcclusionIndex::default();
        assert!(index.debug_hit_bits().is_empty());
    }
}
