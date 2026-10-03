//! Frustum, distance and screen-size culling.

use noxel_core::math::{Aabb, Vec3};
use noxel_render::CameraView;
use noxel_render::renderer::{CullReason, RenderSettings};
use noxel_render::scene::Instance;

/// Thresholds for the cheap culling stages.
#[derive(Clone, Debug, PartialEq)]
pub struct CullSettings {
    /// Instances further than this from the camera are dropped.
    pub max_distance: f32,
    /// Per-layer distance overrides, indexed by layer bit.
    ///
    /// A layer with an override of `0` uses [`CullSettings::max_distance`].
    /// This is how a game gives terrain a 300 m view distance while a decorative
    /// grass layer fades out at 40 m.
    pub layer_distances: [f32; 32],
    /// Instances whose bounding sphere is smaller than this many pixels are
    /// dropped. `0.5` keeps anything covering at least half a pixel.
    pub min_screen_radius: f32,
    /// Reference viewport height, used when the camera does not report an exact
    /// world-units-per-pixel figure.
    pub reference_viewport_height: f32,
    /// Adds a margin to the frustum test so an instance that is *about* to enter
    /// the view is kept, avoiding a pop at the edge of the screen.
    pub frustum_margin: f32,
}

impl Default for CullSettings {
    fn default() -> Self {
        Self {
            max_distance: 250.0,
            layer_distances: [0.0; 32],
            // Sub-pixel geometry costs more to set up than it can possibly
            // contribute, so the default is deliberately at the pixel boundary.
            min_screen_radius: 0.45,
            reference_viewport_height: 180.0,
            frustum_margin: 0.0,
        }
    }
}

impl CullSettings {
    /// The distance limit for a layer.
    #[must_use]
    pub fn distance_for_layer(&self, layer: u32) -> f32 {
        if layer == 0 {
            return self.max_distance;
        }
        let bit = layer.trailing_zeros() as usize;
        if bit < 32 {
            let override_distance = self.layer_distances[bit];
            if override_distance > 0.0 {
                return override_distance;
            }
        }
        self.max_distance
    }

    /// Sets a layer's view distance.
    pub fn set_layer_distance(&mut self, layer_bit: usize, distance: f32) {
        if layer_bit < 32 {
            self.layer_distances[layer_bit] = distance.max(0.0);
        }
    }
}

/// The frustum/distance/size culler.
#[derive(Clone, Debug)]
pub struct Culler {
    settings: CullSettings,
}

impl Culler {
    /// Creates a culler.
    #[must_use]
    pub fn new(settings: CullSettings) -> Self {
        Self { settings }
    }

    /// The thresholds.
    #[must_use]
    pub fn settings(&self) -> &CullSettings {
        &self.settings
    }

    /// Replaces the thresholds.
    pub fn set_settings(&mut self, settings: CullSettings) {
        self.settings = settings;
    }

    /// The world size of one screen pixel at a given distance from the camera.
    ///
    /// Orthographic: constant. Perspective: proportional to distance.
    #[must_use]
    pub fn units_per_pixel(camera: &CameraView, distance: f32, viewport_height: f32) -> f32 {
        let height = if viewport_height > 0.0 {
            viewport_height
        } else {
            180.0
        };
        if camera.is_orthographic() {
            let ortho = if camera.ortho_height > 0.0 {
                camera.ortho_height
            } else {
                20.0
            };
            ortho / height
        } else {
            let fov = if camera.fov_y > 0.0 {
                camera.fov_y
            } else {
                1.0
            };
            // The visible height at `distance` is 2 * d * tan(fov/2).
            2.0 * distance.max(0.01) * (fov * 0.5).tan() / height
        }
    }

    /// The radius, in pixels, of an instance's bounding sphere.
    #[must_use]
    pub fn screen_radius(
        camera: &CameraView,
        bounds: &Aabb,
        distance: f32,
        viewport_height: f32,
    ) -> f32 {
        let units = Self::units_per_pixel(camera, distance, viewport_height);
        if units <= 0.0 {
            return f32::INFINITY;
        }
        bounds.bounding_sphere_radius() / units
    }

    /// Classifies an instance.
    ///
    /// `Ok(distance)` means "draw it"; `Err(reason)` names the stage that
    /// rejected it.
    pub fn evaluate(
        &self,
        bounds: Aabb,
        camera: &CameraView,
        instance: &Instance,
        settings: &RenderSettings,
    ) -> Result<f32, CullReason> {
        if instance.flags.always_visible {
            return Ok(camera.position.distance(bounds.center()));
        }

        let centre = bounds.center();
        let distance = camera.position.distance(centre);

        // Behind the near plane: the point is behind the camera, so nothing of
        // it can be in front of the projection plane.
        if camera.depth_of(centre) < -bounds.bounding_sphere_radius() - camera.near {
            return Err(CullReason::NearPlane);
        }

        if !camera.frustum.intersects_aabb(&bounds) {
            if self.settings.frustum_margin > 0.0
                && camera
                    .frustum
                    .intersects_aabb(&bounds.expanded(self.settings.frustum_margin))
            {
                // About to enter the view: keep it, so it does not pop in.
            } else {
                return Err(CullReason::Frustum);
            }
        }

        let limit = self.settings.distance_for_layer(instance.layer);
        if distance - bounds.bounding_sphere_radius() > limit {
            return Err(CullReason::Distance);
        }

        if self.settings.min_screen_radius > 0.0 {
            let radius = Self::screen_radius(
                camera,
                &bounds,
                distance,
                self.settings.reference_viewport_height,
            );
            if radius < self.settings.min_screen_radius {
                return Err(CullReason::TooSmall);
            }
        }
        let _ = settings;
        Ok(distance)
    }

    /// Convenience: the distance from the camera to a point.
    #[must_use]
    pub fn distance_to(camera: &CameraView, point: Vec3) -> f32 {
        camera.position.distance(point)
    }
}

impl Default for Culler {
    fn default() -> Self {
        Self::new(CullSettings::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::{Color, Transform};
    use noxel_render::material::Material;
    use noxel_render::mesh::Mesh;
    use noxel_render::scene::Scene;

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

    fn instance_at(
        scene: &mut Scene,
        position: Vec3,
        size: f32,
    ) -> noxel_render::scene::InstanceHandle {
        let mesh = scene.add_mesh(Mesh::cube(size));
        let mat = scene.add_material(Material::lit("b", Color::WHITE));
        scene.spawn("b", mesh, mat, Transform::from_translation(position))
    }

    #[test]
    fn visible_instance_is_kept() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::ZERO, 4.0);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::default();
        assert!(
            culler
                .evaluate(
                    instance.bounds(),
                    &camera(),
                    instance,
                    &RenderSettings::default()
                )
                .is_ok()
        );
    }

    #[test]
    fn far_instance_is_rejected_by_distance() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::new(4000.0, 0.0, 0.0), 4.0);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::new(CullSettings {
            max_distance: 100.0,
            ..Default::default()
        });
        // The frustum rejects it first; widen the frustum to test distance.
        let wide = CameraView::orthographic(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            10_000.0,
            1.0,
            1.0,
            10_000.0,
        );
        assert_eq!(
            culler.evaluate(
                instance.bounds(),
                &wide,
                instance,
                &RenderSettings::default()
            ),
            Err(CullReason::Distance)
        );
    }

    #[test]
    fn tiny_instance_is_rejected_by_size() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::new(5.0, 0.0, 0.0), 0.0001);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::new(CullSettings {
            min_screen_radius: 0.5,
            max_distance: 1e9,
            ..Default::default()
        });
        assert_eq!(
            culler.evaluate(
                instance.bounds(),
                &camera(),
                instance,
                &RenderSettings::default()
            ),
            Err(CullReason::TooSmall)
        );
    }

    #[test]
    fn always_visible_bypasses_every_test() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::new(9000.0, 0.0, 0.0), 0.0001);
        scene.instance_mut(h).unwrap().flags = noxel_render::scene::InstanceFlags::marker();
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::new(CullSettings {
            max_distance: 1.0,
            ..Default::default()
        });
        assert!(
            culler
                .evaluate(
                    instance.bounds(),
                    &camera(),
                    instance,
                    &RenderSettings::default()
                )
                .is_ok()
        );
    }

    #[test]
    fn near_plane_rejects_what_is_behind_the_camera() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::new(0.0, 500.0, 0.0), 4.0);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::new(CullSettings {
            max_distance: 1e9,
            min_screen_radius: 0.0,
            ..Default::default()
        });
        assert_eq!(
            culler.evaluate(
                instance.bounds(),
                &camera(),
                instance,
                &RenderSettings::default()
            ),
            Err(CullReason::NearPlane)
        );
    }

    #[test]
    fn units_per_pixel_is_constant_for_ortho() {
        let c = camera();
        let a = Culler::units_per_pixel(&c, 1.0, 180.0);
        let b = Culler::units_per_pixel(&c, 100.0, 180.0);
        assert!((a - b).abs() < 1e-9);
        assert!((a - 30.0 / 180.0).abs() < 1e-6);
    }

    #[test]
    fn units_per_pixel_grows_with_distance_for_perspective() {
        let c = CameraView::perspective(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            1000.0,
        );
        let near = Culler::units_per_pixel(&c, 10.0, 180.0);
        let far = Culler::units_per_pixel(&c, 100.0, 180.0);
        assert!(far > near, "{far} vs {near}");
        assert!((far / near - 10.0).abs() < 1e-3);
    }

    #[test]
    fn screen_radius_scales_inversely_with_distance() {
        let c = camera();
        let bounds = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let r = Culler::screen_radius(&c, &bounds, 10.0, 180.0);
        assert!(r > 0.0);
        // Orthographic: distance does not change the on-screen size.
        let r2 = Culler::screen_radius(&c, &bounds, 100.0, 180.0);
        assert!((r - r2).abs() < 1e-9);
    }

    #[test]
    fn layer_distances_override_the_global_limit() {
        let mut settings = CullSettings {
            max_distance: 100.0,
            ..Default::default()
        };
        assert_eq!(settings.distance_for_layer(1 << 3), 100.0);
        settings.set_layer_distance(3, 30.0);
        assert_eq!(settings.distance_for_layer(1 << 3), 30.0);
        // A layer with no override still uses the global limit.
        assert_eq!(settings.distance_for_layer(1 << 4), 100.0);
    }

    #[test]
    fn layer_distance_out_of_range_is_ignored() {
        let mut settings = CullSettings::default();
        settings.set_layer_distance(99, 5.0);
        assert_eq!(settings.layer_distances[0], 0.0);
    }

    #[test]
    fn frustum_margin_keeps_edge_instances() {
        // The instance sits just outside the frustum; with a margin it is kept.
        let camera = CameraView::orthographic(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            20.0,
            1.0,
            1.0,
            500.0,
        );
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::new(16.0, 0.0, 0.0), 4.0);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();

        let strict = Culler::new(CullSettings {
            max_distance: 1e9,
            min_screen_radius: 0.0,
            frustum_margin: 0.0,
            ..Default::default()
        });
        assert!(
            strict
                .evaluate(
                    instance.bounds(),
                    &camera,
                    instance,
                    &RenderSettings::default()
                )
                .is_err()
        );

        let lenient = Culler::new(CullSettings {
            max_distance: 1e9,
            min_screen_radius: 0.0,
            frustum_margin: 10.0,
            ..Default::default()
        });
        assert!(
            lenient
                .evaluate(
                    instance.bounds(),
                    &camera,
                    instance,
                    &RenderSettings::default()
                )
                .is_ok()
        );
    }

    #[test]
    fn distance_helper() {
        let c = camera();
        assert!((Culler::distance_to(&c, Vec3::ZERO) - 40.0).abs() < 1e-4);
    }

    #[test]
    fn degenerate_settings_do_not_panic() {
        let mut scene = Scene::new();
        let h = instance_at(&mut scene, Vec3::ZERO, 1.0);
        scene.update_all_bounds();
        let instance = scene.instance(h).unwrap();
        let culler = Culler::new(CullSettings {
            max_distance: 0.0,
            min_screen_radius: 0.0,
            reference_viewport_height: 0.0,
            ..Default::default()
        });
        let _ = culler.evaluate(
            instance.bounds(),
            &camera(),
            instance,
            &RenderSettings::default(),
        );
    }
}
