//! The renderer interface shared by the rasterizer and the ray tracer.
//!
//! Both backends consume exactly the same inputs — a [`Scene`], a
//! [`CameraView`], a [`VisibleSet`] and [`RenderSettings`] — and write into a
//! [`Framebuffer`]. That symmetry is the point of `docs/adr/0004-dual-renderer.md`:
//! a game can switch between them at runtime, and a golden-image test can render
//! the same frame both ways and compare silhouettes.

use noxel_core::math::{Frustum, Mat4, Vec2, Vec3};
use noxel_core::time::Stopwatch;

use crate::framebuffer::Framebuffer;
use crate::scene::{InstanceHandle, Scene};

/// Everything the renderer needs to know about the camera, precomputed.
///
/// `noxel-camera` produces this; the renderer never touches a camera rig. A
/// first-person game, a cutscene camera and the top-down rig all reduce to the
/// same struct.
#[derive(Clone, Copy, Debug)]
pub struct CameraView {
    /// Camera position in world space.
    pub position: Vec3,
    /// Unit forward vector (`-Z` of the camera basis).
    pub forward: Vec3,
    /// Unit up vector.
    pub up: Vec3,
    /// Unit right vector.
    pub right: Vec3,
    /// World -> view.
    pub view: Mat4,
    /// View -> clip, depth in `[0, 1]`.
    pub projection: Mat4,
    /// `projection * view`.
    pub view_projection: Mat4,
    /// Near plane distance.
    pub near: f32,
    /// Far plane distance.
    pub far: f32,
    /// Vertical field of view in radians; `0` when the camera is orthographic.
    pub fov_y: f32,
    /// Vertical world-space extent for an orthographic camera.
    pub ortho_height: f32,
    /// The six frustum planes extracted from `view_projection`.
    pub frustum: Frustum,
    /// World units per screen pixel at the camera's focal plane.
    ///
    /// The pixel-art pipeline uses this to keep sprite size quantised to whole
    /// pixels: a sprite whose on-screen size is not a whole number of pixels
    /// will shimmer as the camera moves.
    pub world_units_per_pixel: f32,
}

impl Default for CameraView {
    /// A camera at the origin looking along `-Z` with a unit view volume.
    ///
    /// An app needs *a* view before the first frame is simulated; this is a
    /// valid one rather than a zeroed matrix that would blank the screen.
    fn default() -> Self {
        Self::orthographic(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            100.0,
        )
    }
}

impl CameraView {
    /// Builds a view from an explicit matrix pair.
    ///
    /// `aspect` must match the framebuffer's, otherwise the image is stretched.
    #[must_use]
    pub fn from_matrices(
        view: Mat4,
        projection: Mat4,
        position: Vec3,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let view_projection = projection * view;
        // Pull the basis back out of the view matrix rather than asking the
        // caller for it: it cannot then disagree with `view`.
        let inverse = view.inverse().unwrap_or(Mat4::IDENTITY);
        let right = inverse.transform_vector3(Vec3::X).normalize_or_zero();
        let up = inverse.transform_vector3(Vec3::Y).normalize_or_zero();
        let forward = -inverse.transform_vector3(Vec3::Z).normalize_or_zero();
        let _ = aspect;
        Self {
            position,
            forward,
            up,
            right,
            view,
            projection,
            view_projection,
            near,
            far,
            fov_y: 0.0,
            ortho_height: 0.0,
            frustum: Frustum::from_view_projection(&view_projection),
            world_units_per_pixel: 0.0,
        }
    }

    /// A right-handed orthographic camera looking at `target` from `position`.
    ///
    /// The top-down pixel-art default: no perspective distortion, so a tile grid
    /// stays a grid.
    #[must_use]
    pub fn orthographic(
        position: Vec3,
        target: Vec3,
        up: Vec3,
        height: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let half_width = height * 0.5 * aspect;
        let half_height = height * 0.5;
        let view = Mat4::look_at_rh(position, target, up);
        let projection = Mat4::orthographic_rh(
            -half_width,
            half_width,
            -half_height,
            half_height,
            near,
            far,
        );
        let mut camera = Self::from_matrices(view, projection, position, aspect, near, far);
        camera.ortho_height = height;
        camera.world_units_per_pixel = if height > 0.0 { height / 1000.0 } else { 0.0 };
        camera
    }

    /// A right-handed perspective camera.
    #[must_use]
    pub fn perspective(
        position: Vec3,
        target: Vec3,
        up: Vec3,
        fov_y: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let view = Mat4::look_at_rh(position, target, up);
        let projection = Mat4::perspective_rh(fov_y, aspect, near, far);
        let mut camera = Self::from_matrices(view, projection, position, aspect, near, far);
        camera.fov_y = fov_y;
        camera
    }

    /// True when the camera is orthographic.
    #[must_use]
    pub fn is_orthographic(&self) -> bool {
        self.fov_y <= 0.0
    }

    /// Sets the world-units-per-pixel figure and returns `self`, for the
    /// pixel-perfect pipeline.
    #[must_use]
    pub fn with_units_per_pixel(mut self, units: f32) -> Self {
        self.world_units_per_pixel = units;
        self
    }

    /// The inverse of [`CameraView::view_projection`], for unprojection.
    #[must_use]
    pub fn inverse_view_projection(&self) -> Mat4 {
        self.view_projection.inverse().unwrap_or(Mat4::IDENTITY)
    }

    /// The distance from the camera to a world point along the view direction.
    #[must_use]
    pub fn depth_of(&self, point: Vec3) -> f32 {
        (point - self.position).dot(self.forward)
    }
}

/// Why an instance was culled. The overlay groups culling statistics by reason,
/// which is what turns "the frame is slow" into "the occluder set is wrong".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CullReason {
    /// Never culled; drawn unconditionally.
    None,
    /// Outside the view frustum.
    Frustum,
    /// Behind the camera's near plane.
    NearPlane,
    /// Beyond the camera's far plane, or beyond the layer's max distance.
    Distance,
    /// Smaller than the layer's minimum screen size (LOD drop).
    TooSmall,
    /// Behind an occluder according to the occlusion system.
    Occluded,
    /// Hiding on a layer the camera has switched off.
    LayerDisabled,
    /// Explicitly marked invisible.
    Hidden,
}

impl CullReason {
    /// A short label for the debug overlay.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "drawn",
            Self::Frustum => "frustum",
            Self::NearPlane => "near",
            Self::Distance => "far",
            Self::TooSmall => "small",
            Self::Occluded => "occluded",
            Self::LayerDisabled => "layer",
            Self::Hidden => "hidden",
        }
    }

    /// Every reason, for the overlay's table.
    pub const ALL: [CullReason; 8] = [
        Self::None,
        Self::Frustum,
        Self::NearPlane,
        Self::Distance,
        Self::TooSmall,
        Self::Occluded,
        Self::LayerDisabled,
        Self::Hidden,
    ];
}

/// One instance that survived culling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleItem {
    /// The instance to draw.
    pub instance: InstanceHandle,
    /// Alpha multiplier, `0..=1`. The occlusion system lowers this to fade a
    /// roof or a tree that is in front of the player.
    pub alpha: f32,
    /// Chosen level of detail: `0` is full detail, higher is coarser.
    pub lod: u8,
    /// Distance from the camera, for sorting. Filled in by the visibility pass
    /// because it already computes it.
    pub distance: f32,
}

/// The set of instances a renderer should draw, with the statistics that
/// explain how it was chosen.
#[derive(Clone, Debug, Default)]
pub struct VisibleSet {
    /// Instances to draw, in the order the visibility pass produced them.
    pub items: Vec<VisibleItem>,
    /// Instances that were rejected, with the reason. Only populated when
    /// [`RenderSettings::track_cull_reasons`] is on, because the bookkeeping is
    /// not free.
    pub culled: Vec<(InstanceHandle, CullReason)>,
    /// Counts per reason.
    pub counts: CullCounts,
}

/// How many instances fell into each [`CullReason`] bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullCounts {
    /// Instances evaluated.
    pub considered: usize,
    /// Instances drawn.
    pub drawn: usize,
    /// Rejected by the frustum test.
    pub frustum: usize,
    /// Rejected as behind the near plane.
    pub near_plane: usize,
    /// Rejected as too far.
    pub distance: usize,
    /// Rejected as too small on screen.
    pub too_small: usize,
    /// Rejected as occluded.
    pub occluded: usize,
    /// Rejected by layer.
    pub layer_disabled: usize,
    /// Explicitly hidden.
    pub hidden: usize,
    /// Occluders that were faded because they blocked the camera.
    pub faded: usize,
}

impl CullCounts {
    /// Adds one count for a reason.
    pub fn record(&mut self, reason: CullReason) {
        match reason {
            CullReason::None => self.drawn += 1,
            CullReason::Frustum => self.frustum += 1,
            CullReason::NearPlane => self.near_plane += 1,
            CullReason::Distance => self.distance += 1,
            CullReason::TooSmall => self.too_small += 1,
            CullReason::Occluded => self.occluded += 1,
            CullReason::LayerDisabled => self.layer_disabled += 1,
            CullReason::Hidden => self.hidden += 1,
        }
    }

    /// Total instances rejected.
    #[must_use]
    pub fn rejected(&self) -> usize {
        self.frustum
            + self.near_plane
            + self.distance
            + self.too_small
            + self.occluded
            + self.layer_disabled
            + self.hidden
    }

    /// The share of considered instances that were drawn, `0..=1`.
    #[must_use]
    pub fn draw_ratio(&self) -> f32 {
        if self.considered == 0 {
            1.0
        } else {
            self.drawn as f32 / self.considered as f32
        }
    }

    /// A compact one-line summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} drawn / {} considered (frustum {}, occluded {}, far {}, small {}, layer {}, hidden {})",
            self.drawn,
            self.considered,
            self.frustum,
            self.occluded,
            self.distance,
            self.too_small,
            self.layer_disabled,
            self.hidden
        )
    }
}

/// Which algorithm the renderer uses for primary visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadingMode {
    /// Software rasterizer only. The default: fast, and the right look for
    /// pixel art.
    #[default]
    Raster,
    /// Rasterized primary visibility with ray-traced shadows, ambient occlusion
    /// and reflections on top. The best quality/cost ratio.
    Hybrid,
    /// Fully ray-traced. Slow, used for stills, golden images and comparison
    /// shots.
    Raytrace,
}

/// Quality and feature switches for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    /// Which pipeline to run.
    pub mode: ShadingMode,
    /// Master switch for shadows.
    pub shadows: bool,
    /// Shadow map resolution for the rasterizer (and the shadow-sample budget
    /// for the ray tracer).
    pub shadow_map_size: u32,
    /// Enable screen-space-free ambient occlusion (ray-traced in hybrid mode, a
    /// cheap analytic term otherwise).
    pub ambient_occlusion: bool,
    /// Ambient occlusion strength.
    pub ao_strength: f32,
    /// Ambient occlusion ray length in metres.
    pub ao_radius: f32,
    /// Enable ray-traced reflections in hybrid mode.
    pub reflections: bool,
    /// Maximum reflection bounces.
    pub reflection_bounces: u32,
    /// Enable distance fog.
    pub fog: bool,
    /// Back-face culling.
    pub backface_culling: bool,
    /// Draw the debug overlay (bounds, occlusion rays, statistics).
    pub debug_overlay: bool,
    /// Record per-instance cull reasons.
    pub track_cull_reasons: bool,
    /// Maximum number of primary rays per frame in ray-traced mode; the tracer
    /// degrades quality rather than blowing the frame budget.
    pub ray_budget: u32,
    /// Samples accumulated per pixel in ray-traced mode.
    pub samples_per_pixel: u32,
    /// Depth bias for the shadow lookup.
    pub shadow_bias: f32,
    /// Multiply outgoing light by this before tone mapping.
    pub exposure: f32,
    /// Blend two frames together (`0` = ignore history, `1` = freeze). Used by
    /// the ray-traced mode's temporal accumulation.
    pub temporal_blend: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            mode: ShadingMode::Raster,
            shadows: true,
            shadow_map_size: 1024,
            ambient_occlusion: false,
            ao_strength: 0.6,
            ao_radius: 1.5,
            reflections: false,
            reflection_bounces: 1,
            fog: true,
            backface_culling: true,
            debug_overlay: false,
            track_cull_reasons: true,
            ray_budget: 400_000,
            samples_per_pixel: 1,
            shadow_bias: 0.0015,
            exposure: 1.0,
            temporal_blend: 0.0,
        }
    }
}

impl RenderSettings {
    /// The resolve settings that pair with these render settings.
    ///
    /// The tone curve is chosen from the mode: a fully ray-traced frame carries
    /// HDR values (an emissive can exceed 1.0) and needs a curve, while the
    /// raster mode is authored to land directly in `[0, 1]` so an unlit sprite
    /// round-trips to the exact bytes the artist drew.
    #[must_use]
    pub fn resolve(&self) -> crate::framebuffer::ResolveSettings {
        crate::framebuffer::ResolveSettings {
            exposure: self.exposure,
            tonemap: if self.mode == ShadingMode::Raytrace {
                crate::framebuffer::ToneMap::Aces
            } else {
                crate::framebuffer::ToneMap::None
            },
            ..crate::framebuffer::ResolveSettings::default()
        }
    }

    /// A preset that favours speed on a large world.
    #[must_use]
    pub fn fast() -> Self {
        Self {
            shadows: true,
            shadow_map_size: 512,
            ambient_occlusion: false,
            ..Self::default()
        }
    }

    /// A preset for a still or a golden-image test.
    #[must_use]
    pub fn quality() -> Self {
        Self {
            mode: ShadingMode::Hybrid,
            shadows: true,
            shadow_map_size: 2048,
            ambient_occlusion: true,
            ao_strength: 0.8,
            ao_radius: 2.0,
            reflections: true,
            samples_per_pixel: 4,
            ..Self::default()
        }
    }
}

/// Counters describing what a frame cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderStats {
    /// Which pipeline ran.
    pub mode: ShadingMode,
    /// Instances submitted by the visibility pass.
    pub instances: usize,
    /// Triangles submitted (before culling inside the renderer).
    pub triangles_submitted: usize,
    /// Triangles that actually produced pixels.
    pub triangles_drawn: usize,
    /// Draw calls / batches.
    pub draw_calls: usize,
    /// Pixels that went through the fragment stage.
    pub pixels_shaded: usize,
    /// Pixels that failed the depth test or were back-facing.
    pub pixels_rejected: usize,
    /// Shadow-map resolution actually used.
    pub shadow_map_size: u32,
    /// Primary rays cast (ray-traced modes).
    pub primary_rays: u32,
    /// Secondary rays cast (shadow/AO/reflection).
    pub secondary_rays: u32,
    /// Time spent transforming and binning geometry, milliseconds.
    pub ms_geometry: f32,
    /// Time spent rasterising/shading, milliseconds.
    pub ms_shade: f32,
    /// Time spent in post-processing, milliseconds.
    pub ms_post: f32,
    /// Total milliseconds for the frame.
    pub ms_total: f32,
}

impl RenderStats {
    /// Pixels per second, the headline throughput number.
    #[must_use]
    pub fn pixels_per_second(&self) -> f64 {
        if self.ms_total <= 0.0 {
            return 0.0;
        }
        self.pixels_shaded as f64 / (self.ms_total as f64 / 1000.0)
    }

    /// Rays per second, for the ray-traced modes.
    #[must_use]
    pub fn rays_per_second(&self) -> f64 {
        if self.ms_total <= 0.0 {
            return 0.0;
        }
        (self.primary_rays + self.secondary_rays) as f64 / (self.ms_total as f64 / 1000.0)
    }

    /// A one-line summary for the demo's report.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{:?}: {:.2} ms | {} instances | {} tris drawn / {} submitted | {} px | {} calls",
            self.mode,
            self.ms_total,
            self.instances,
            self.triangles_drawn,
            self.triangles_submitted,
            self.pixels_shaded,
            self.draw_calls
        )
    }
}

/// Anything that can turn a [`Scene`] into pixels.
///
/// Implemented by [`crate::raster::RasterRenderer`] and
/// [`crate::raytrace::RayTracer`]. A future GPU backend implements the same
/// trait (see `docs/guides/gpu-backend.md`), which is why the trait takes
/// references and a mutable framebuffer rather than owning any of them.
pub trait Renderer {
    /// A short name for logs and the debug overlay.
    fn name(&self) -> &'static str;

    /// Which shading mode this renderer implements.
    fn mode(&self) -> ShadingMode;

    /// Renders one frame into `target`.
    ///
    /// `visible` is the output of the visibility pass. Passing `None` means
    /// "draw everything", which is what tests and tools do; a real frame always
    /// passes a set, because that is where the culling savings come from.
    fn render(
        &mut self,
        scene: &Scene,
        camera: &CameraView,
        visible: Option<&VisibleSet>,
        target: &mut Framebuffer,
        settings: &RenderSettings,
    ) -> RenderStats;

    /// Called when the framebuffer size changes, so the renderer can resize its
    /// internal buffers (shadow map, accumulation buffer).
    fn resize(&mut self, width: u32, height: u32);

    /// Releases cached GPU-equivalent resources (BVH, shadow map, buffers).
    fn release_cached(&mut self);

    /// A short description of the renderer's configuration.
    fn describe(&self) -> String {
        format!("{} ({:?})", self.name(), self.mode())
    }
}

/// A stopwatch pair used by both renderers to fill [`RenderStats`] without
/// duplicating the timing plumbing.
#[derive(Debug)]
pub struct FrameTimer {
    /// Whole-frame watch.
    pub total: Stopwatch,
    /// Geometry stage.
    pub geometry: Stopwatch,
    /// Shading stage.
    pub shade: Stopwatch,
    /// Post-processing stage.
    pub post: Stopwatch,
}

impl FrameTimer {
    /// Starts a timer for this frame.
    #[must_use]
    pub fn start() -> Self {
        Self {
            total: Stopwatch::start(),
            geometry: Stopwatch::start(),
            shade: Stopwatch::start(),
            post: Stopwatch::start(),
        }
    }

    /// Writes the accumulated times into `stats` and returns the total.
    pub fn finish(&mut self, stats: &mut RenderStats) -> f32 {
        let geometry = self.geometry.elapsed_ms() as f32;
        let shade = self.shade.elapsed_ms() as f32;
        let post = self.post.elapsed_ms() as f32;
        stats.ms_geometry = geometry;
        stats.ms_shade = shade;
        stats.ms_post = post;
        stats.ms_total = self.total.elapsed_ms() as f32;
        stats.ms_total
    }
}

/// Describes a render target's size for the pixel-art pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The internal resolution the scene is rendered at before upscaling.
    ///
    /// A pixel-art game renders at, say, 320x180 and upscales to the window;
    /// keeping the internal size fixed is what stops sprites from shimmering
    /// when the window is resized.
    pub internal_width: u32,
    /// Internal height.
    pub internal_height: u32,
    /// When true, the upscale uses nearest-neighbour (crisp pixels) rather than
    /// bilinear.
    pub pixel_perfect: bool,
}

impl Viewport {
    /// A viewport that renders at the full size.
    #[must_use]
    pub const fn native(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            internal_width: width,
            internal_height: height,
            pixel_perfect: false,
        }
    }

    /// A pixel-art viewport with a fixed internal resolution.
    #[must_use]
    pub const fn pixel_art(
        width: u32,
        height: u32,
        internal_width: u32,
        internal_height: u32,
    ) -> Self {
        Self {
            width,
            height,
            internal_width,
            internal_height,
            pixel_perfect: true,
        }
    }

    /// The integer scale factor between internal and output resolution.
    ///
    /// Rounded down so the upscale never repeats a pixel twice (which would
    /// produce uneven pixel sizes, the classic "shimmering pixels" artifact).
    #[must_use]
    pub fn integer_scale(&self) -> u32 {
        if self.internal_width == 0 || self.internal_height == 0 {
            return 1;
        }
        (self.width / self.internal_width)
            .min(self.height / self.internal_height)
            .max(1)
    }

    /// The aspect ratio of the internal target.
    #[must_use]
    pub fn internal_aspect(&self) -> f32 {
        self.internal_width as f32 / self.internal_height.max(1) as f32
    }

    /// The size of the letterboxed image after upscaling by
    /// [`Viewport::integer_scale`], in output pixels.
    #[must_use]
    pub fn scaled_size(&self) -> (u32, u32) {
        let s = self.integer_scale();
        (self.internal_width * s, self.internal_height * s)
    }

    /// The offset that centres the scaled image in the output.
    #[must_use]
    pub fn letterbox_offset(&self) -> Vec2 {
        let (w, h) = self.scaled_size();
        Vec2::new(
            (self.width.saturating_sub(w)) as f32 * 0.5,
            (self.height.saturating_sub(h)) as f32 * 0.5,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::Framebuffer;

    #[test]
    fn cull_counts_record_every_reason() {
        let mut c = CullCounts::default();
        for r in CullReason::ALL {
            c.record(r);
        }
        assert_eq!(c.drawn, 1);
        assert_eq!(c.frustum, 1);
        assert_eq!(c.occluded, 1);
        assert_eq!(c.rejected(), 7);
        c.considered = 8;
        assert!((c.draw_ratio() - 0.125).abs() < 1e-6);
        assert!(c.summary().contains("1 drawn / 8 considered"));
    }

    #[test]
    fn cull_reason_labels_are_unique() {
        let mut labels: Vec<&str> = CullReason::ALL.iter().map(|r| r.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), CullReason::ALL.len());
    }

    #[test]
    fn camera_view_orthographic_matrices_agree_with_the_basis() {
        let cam = CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            10.0,
            16.0 / 9.0,
            1.0,
            200.0,
        );
        assert!(cam.is_orthographic());
        // Looking straight down with world up = +Z: forward is -Y, screen up is
        // +Z, and right is forward x up = -X.
        assert!(
            cam.forward.approx_eq(Vec3::new(0.0, -1.0, 0.0), 1e-4),
            "{:?}",
            cam.forward
        );
        assert!(cam.up.approx_eq(Vec3::Z, 1e-4), "{:?}", cam.up);
        assert!(
            cam.right.approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-4),
            "{:?}",
            cam.right
        );
        // Right-handed camera basis: right x up = backward (-forward).
        assert!(
            cam.right.cross(cam.up).approx_eq(-cam.forward, 1e-4),
            "right x up must be -forward: {:?}",
            cam.right.cross(cam.up)
        );
    }

    #[test]
    fn camera_view_perspective_recovers_fov() {
        let cam = CameraView::perspective(
            Vec3::new(0.0, 10.0, 40.0),
            Vec3::ZERO,
            Vec3::Y,
            0.9,
            1.5,
            0.1,
            500.0,
        );
        assert!(!cam.is_orthographic());
        assert!((cam.fov_y - 0.9).abs() < 1e-6);
    }

    #[test]
    fn camera_frustum_culls_correctly() {
        let cam = CameraView::orthographic(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            10.0,
            1.0,
            1.0,
            100.0,
        );
        assert!(cam.frustum.intersects_aabb(&noxel_core::math::Aabb::new(
            Vec3::splat(-1.0),
            Vec3::splat(1.0)
        )));
        assert!(!cam.frustum.intersects_aabb(&noxel_core::math::Aabb::new(
            Vec3::splat(500.0),
            Vec3::splat(501.0)
        )));
    }

    #[test]
    fn camera_depth_of() {
        let cam = CameraView::perspective(
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            100.0,
        );
        assert!((cam.depth_of(Vec3::ZERO) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn inverse_view_projection_round_trips() {
        let cam = CameraView::perspective(
            Vec3::new(3.0, 4.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            1.5,
            0.1,
            100.0,
        );
        let p = Vec3::new(1.0, 0.0, -2.0);
        let clip = cam.view_projection.transform_point4(p.extend(1.0));
        let back = cam
            .inverse_view_projection()
            .transform_point4(clip)
            .perspective_divide()
            .unwrap();
        assert!(back.approx_eq(p, 1e-3), "{back:?}");
    }

    #[test]
    fn viewport_pixel_art_scale() {
        let v = Viewport::pixel_art(1920, 1080, 320, 180);
        assert_eq!(v.integer_scale(), 6);
        assert_eq!(v.scaled_size(), (1920, 1080));
        assert_eq!(v.letterbox_offset(), Vec2::ZERO);
        assert!(v.pixel_perfect);
    }

    #[test]
    fn viewport_letterboxing() {
        let v = Viewport::pixel_art(1000, 1000, 320, 180);
        assert_eq!(v.integer_scale(), 3);
        assert_eq!(v.scaled_size(), (960, 540));
        assert_eq!(v.letterbox_offset(), Vec2::new(20.0, 230.0));
    }

    #[test]
    fn viewport_native_is_one_to_one() {
        let v = Viewport::native(800, 600);
        assert_eq!(v.integer_scale(), 1);
        assert_eq!(v.internal_aspect(), 800.0 / 600.0);
    }

    #[test]
    fn viewport_degenerate_input_is_safe() {
        let v = Viewport {
            width: 10,
            height: 10,
            internal_width: 0,
            internal_height: 0,
            pixel_perfect: false,
        };
        assert_eq!(v.integer_scale(), 1);
        assert_eq!(v.internal_aspect(), 0.0);
        assert_eq!(v.scaled_size(), (0, 0));
    }

    #[test]
    fn settings_presets_differ() {
        let f = RenderSettings::fast();
        let q = RenderSettings::quality();
        assert!(f.shadow_map_size < q.shadow_map_size);
        assert!(!f.ambient_occlusion && q.ambient_occlusion);
        assert_eq!(f.mode, ShadingMode::Raster);
        assert_eq!(q.mode, ShadingMode::Hybrid);
    }

    #[test]
    fn stats_summaries() {
        let s = RenderStats {
            triangles_drawn: 10,
            triangles_submitted: 20,
            instances: 3,
            draw_calls: 4,
            pixels_shaded: 1000,
            ms_total: 2.0,
            ..Default::default()
        };
        assert!(s.pixels_per_second() > 400_000.0);
        assert!(s.summary().contains("10 tris drawn"));
    }

    #[test]
    fn stats_handle_zero_time() {
        let s = RenderStats::default();
        assert_eq!(s.pixels_per_second(), 0.0);
        assert_eq!(s.rays_per_second(), 0.0);
    }

    #[test]
    fn visible_set_default_is_empty() {
        let v = VisibleSet::default();
        assert!(v.items.is_empty());
        assert_eq!(v.counts.drawn, 0);
        assert_eq!(v.counts.draw_ratio(), 1.0);
    }

    #[test]
    fn frame_timer_measures_something() {
        let mut t = FrameTimer::start();
        let mut stats = RenderStats::default();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let total = t.finish(&mut stats);
        assert!(total > 0.0);
        assert_eq!(stats.ms_total, total);
    }

    #[test]
    fn renderer_trait_is_object_safe() {
        // The engine stores renderers behind `Box<dyn Renderer>` so a game can
        // switch pipelines at runtime; this test fails to compile if the trait
        // grows a generic method.
        struct Null;
        impl Renderer for Null {
            fn name(&self) -> &'static str {
                "null"
            }
            fn mode(&self) -> ShadingMode {
                ShadingMode::Raster
            }
            fn render(
                &mut self,
                _scene: &Scene,
                _camera: &CameraView,
                _visible: Option<&VisibleSet>,
                _target: &mut Framebuffer,
                _settings: &RenderSettings,
            ) -> RenderStats {
                RenderStats::default()
            }
            fn resize(&mut self, _w: u32, _h: u32) {}
            fn release_cached(&mut self) {}
        }
        let mut r: Box<dyn Renderer> = Box::new(Null);
        let scene = Scene::new();
        let cam =
            CameraView::orthographic(Vec3::Y * 10.0, Vec3::ZERO, Vec3::Z, 10.0, 1.0, 1.0, 100.0);
        let mut fb = Framebuffer::new(4, 4);
        let stats = r.render(&scene, &cam, None, &mut fb, &RenderSettings::default());
        assert_eq!(stats.mode, ShadingMode::Raster);
        assert!(r.describe().contains("null"));
    }
}
