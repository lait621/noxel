//! Level-of-detail selection.
//!
//! The renderer draws one mesh per instance, so "LOD" here does not mean
//! swapping meshes; it means telling the rest of the engine how much an instance
//! is worth spending on. Three consumers:
//!
//! * the **NPC system**, which drops distant agents to a cheaper update tier,
//! * the **streaming system**, which uses it to decide what to keep resident,
//! * a **game**, which can swap a detailed mesh for a cheap one on `lod >= 2`.
//!
//! Thresholds are in **screen-space pixels of bounding radius**, not distance,
//! because that is what actually determines whether a swap is visible: a large
//! object far away and a small object nearby can need the same treatment.

use noxel_core::math::Aabb;
use noxel_render::CameraView;

use crate::cull::Culler;

/// Screen-radius thresholds for the four LOD levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodLevels {
    /// `>= full` px: LOD 0 (full detail).
    pub full: f32,
    /// `>= medium` px: LOD 1.
    pub medium: f32,
    /// `>= low` px: LOD 2. Below `low` is LOD 3.
    pub low: f32,
    /// Viewport height used to convert world size to pixels.
    pub viewport_height: f32,
    /// Extra distance added per LOD level, as a fraction of the instance's
    /// screen radius. Zero disables distance biasing.
    pub distance_bias: f32,
}

impl Default for LodLevels {
    fn default() -> Self {
        // At 320x180 a 12 px radius is a substantial object, 3 px is a detail
        // you only notice when it is missing.
        Self {
            full: 12.0,
            medium: 5.0,
            low: 1.5,
            viewport_height: 180.0,
            distance_bias: 0.0,
        }
    }
}

impl LodLevels {
    /// The level an instance should use.
    ///
    /// `0` is full detail, `3` is the cheapest.
    #[must_use]
    pub fn level_for(&self, bounds: &Aabb, camera: &CameraView) -> u8 {
        let distance = camera.position.distance(bounds.center());
        let radius = Culler::screen_radius(camera, bounds, distance, self.viewport_height);
        self.level_for_radius(radius)
    }

    /// The level for an explicit screen radius in pixels.
    #[must_use]
    pub fn level_for_radius(&self, radius: f32) -> u8 {
        if radius >= self.full {
            0
        } else if radius >= self.medium {
            1
        } else if radius >= self.low {
            2
        } else {
            3
        }
    }

    /// True when a level should be drawn at all at the given screen radius.
    #[must_use]
    pub fn is_visible_at(&self, radius: f32) -> bool {
        radius >= self.low * 0.5
    }

    /// The instance's screen radius in pixels.
    #[must_use]
    pub fn screen_radius(&self, bounds: &Aabb, camera: &CameraView) -> f32 {
        let distance = camera.position.distance(bounds.center());
        Culler::screen_radius(camera, bounds, distance, self.viewport_height)
    }
}

/// A concrete LOD decision, carrying the numbers that produced it so the debug
/// overlay can show *why* an object dropped a level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodSelection {
    /// The chosen level, `0..=3`.
    pub level: u8,
    /// The bounding sphere radius in pixels.
    pub screen_radius: f32,
    /// Distance from the camera.
    pub distance: f32,
    /// The fraction of the full update rate this instance should get, `0..=1`.
    ///
    /// The NPC scheduler multiplies its per-tier rate by this, so a distant
    /// crowd updates at a reduced frequency without any per-agent bookkeeping.
    pub update_fraction: f32,
}

impl LodSelection {
    /// The update fraction for a level: full at 0, quarter rate at 3.
    #[must_use]
    pub fn update_fraction_for(level: u8) -> f32 {
        match level {
            0 => 1.0,
            1 => 0.5,
            2 => 0.25,
            _ => 0.1,
        }
    }

    /// Builds a selection from a level and the measurements behind it.
    #[must_use]
    pub fn new(level: u8, screen_radius: f32, distance: f32) -> Self {
        Self {
            level,
            screen_radius,
            distance,
            update_fraction: Self::update_fraction_for(level),
        }
    }

    /// True when this instance should be updated this frame, given a frame
    /// counter.
    ///
    /// Deterministic: the decision depends only on the instance's id and the
    /// frame, so a crowd update never flickers.
    #[must_use]
    pub fn should_update(&self, instance_id: u64, frame: u64) -> bool {
        if self.update_fraction >= 1.0 {
            return true;
        }
        let period = (1.0 / self.update_fraction).round().max(1.0) as u64;
        frame.wrapping_add(instance_id) % period == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Vec3;

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            30.0,
            1.0,
            1.0,
            500.0,
        )
    }

    #[test]
    fn thresholds_map_to_levels() {
        let l = LodLevels::default();
        assert_eq!(l.level_for_radius(100.0), 0);
        assert_eq!(l.level_for_radius(12.0), 0);
        assert_eq!(l.level_for_radius(11.9), 1);
        assert_eq!(l.level_for_radius(5.0), 1);
        assert_eq!(l.level_for_radius(4.9), 2);
        assert_eq!(l.level_for_radius(1.5), 2);
        assert_eq!(l.level_for_radius(1.4), 3);
        assert_eq!(l.level_for_radius(0.0), 3);
    }

    #[test]
    fn a_big_object_nearby_is_full_detail() {
        let l = LodLevels::default();
        let bounds = Aabb::new(Vec3::splat(-5.0), Vec3::splat(5.0));
        assert_eq!(l.level_for(&bounds, &camera()), 0);
    }

    #[test]
    fn a_small_object_far_away_is_the_cheapest() {
        let l = LodLevels::default();
        let bounds = Aabb::new(Vec3::splat(-0.05), Vec3::splat(0.05));
        let far = CameraView::orthographic(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            30.0,
            1.0,
            1.0,
            500.0,
        );
        assert_eq!(l.level_for(&bounds, &far), 3);
    }

    #[test]
    fn screen_radius_shrinks_with_distance_for_perspective() {
        let l = LodLevels::default();
        let bounds = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let near = CameraView::perspective(
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            1000.0,
        );
        let far = CameraView::perspective(
            Vec3::new(0.0, 0.0, 200.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            1000.0,
        );
        let a = l.screen_radius(&bounds, &near);
        let b = l.screen_radius(&bounds, &far);
        assert!(b < a, "{b} vs {a}");
    }

    #[test]
    fn update_fractions_are_ordered() {
        assert!(LodSelection::update_fraction_for(0) > LodSelection::update_fraction_for(1));
        assert!(LodSelection::update_fraction_for(1) > LodSelection::update_fraction_for(2));
        assert!(LodSelection::update_fraction_for(2) > LodSelection::update_fraction_for(3));
        assert_eq!(LodSelection::update_fraction_for(0), 1.0);
        assert_eq!(LodSelection::update_fraction_for(9), 0.1);
    }

    #[test]
    fn selection_reports_its_measurements() {
        let s = LodSelection::new(2, 3.0, 120.0);
        assert_eq!(s.level, 2);
        assert_eq!(s.screen_radius, 3.0);
        assert_eq!(s.distance, 120.0);
        assert!((s.update_fraction - 0.25).abs() < 1e-6);
    }

    #[test]
    fn should_update_is_deterministic_and_uses_the_expected_rate() {
        let full = LodSelection::new(0, 100.0, 1.0);
        for frame in 0..20 {
            assert!(full.should_update(7, frame), "LOD 0 always updates");
        }
        let quarter = LodSelection::new(2, 3.0, 50.0);
        let mut hits = 0;
        for frame in 0..40 {
            if quarter.should_update(3, frame) {
                hits += 1;
            }
        }
        assert_eq!(hits, 10, "a quarter rate must update 1 frame in 4");
    }

    #[test]
    fn different_instances_update_on_different_frames() {
        let s = LodSelection::new(2, 3.0, 50.0);
        let a: Vec<bool> = (0..8).map(|f| s.should_update(0, f)).collect();
        let b: Vec<bool> = (0..8).map(|f| s.should_update(1, f)).collect();
        assert_ne!(a, b, "crowd members must not all update on the same frame");
    }

    #[test]
    fn is_visible_at_boundary() {
        let l = LodLevels::default();
        assert!(l.is_visible_at(10.0));
        assert!(!l.is_visible_at(0.1));
    }

    #[test]
    fn degenerate_viewport_does_not_panic() {
        let l = LodLevels {
            viewport_height: 0.0,
            ..LodLevels::default()
        };
        let bounds = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let r = l.screen_radius(&bounds, &camera());
        assert!(r.is_finite());
    }
}
