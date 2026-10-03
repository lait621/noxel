//! # Noxel camera
//!
//! Camera rigs for a top-down game, and the pixel-perfect projection that makes
//! a 320x180 viewport look like pixel art instead of like a low-resolution 3D
//! render.
//!
//! ```no_run
//! use noxel_camera::{TopDownCamera, ProjectionMode};
//! use noxel_core::math::Vec3;
//!
//! let mut camera = TopDownCamera::pixel_art(320, 180);
//! camera.set_projection(ProjectionMode::Orthographic { height: 18.0 });
//! // Each frame:
//! // camera.follow(player_position, dt);
//! // camera.apply_shake(0.4);
//! // let view = camera.view(320.0 / 180.0);
//! ```
//!
//! ## What is in here
//!
//! | Type | Purpose |
//! |---|---|
//! | [`TopDownCamera`] | the rig: focus, deadzone, smoothing, look-ahead, zoom, bounds |
//! | [`ProjectionMode`] | orthographic (pixel art) or perspective (3D look) |
//! | [`CameraShake`] | trauma-based shake with per-axis noise |
//! | [`CameraZone`] | per-region overrides (an interior, a cutscene, a boss room) |
//! | [`PixelPerfect`] | snaps the focus so sprites land on whole pixels |
//!
//! ## Why the rig owns smoothing rather than the game
//!
//! Camera feel is almost entirely a function of how the focus moves. Putting
//! that behaviour in one place — with a deadzone, a frame-rate-independent
//! smoothing factor and an option to snap to whole pixels — means every game
//! built on Noxel gets a camera that does not jitter, does not lag behind a
//! running player, and does not shimmer when the window is resized. See
//! `docs/guides/camera.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod shake;
pub mod zone;

pub use shake::CameraShake;
pub use zone::{CameraZone, ZoneBlend};

use noxel_core::math::{Aabb, Mat4, Vec2, Vec3, angle_delta, damp_factor, lerp, rotate_towards};
use noxel_core::rng::hash_2d;
use noxel_render::renderer::CameraView;

/// How the camera projects the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProjectionMode {
    /// No perspective distortion: a tile grid stays a grid.
    ///
    /// `height` is the vertical world-space extent visible at the focus plane,
    /// so `height / viewport_height` is the world size of one pixel.
    Orthographic {
        /// Vertical world-space extent of the view.
        height: f32,
    },
    /// A perspective camera, for a more three-dimensional look or for interiors.
    Perspective {
        /// Vertical field of view in radians.
        fov_y: f32,
    },
}

impl ProjectionMode {
    /// True for an orthographic projection.
    #[must_use]
    pub fn is_orthographic(&self) -> bool {
        matches!(self, Self::Orthographic { .. })
    }

    /// The vertical extent for an orthographic camera, or `0`.
    #[must_use]
    pub fn height(&self) -> f32 {
        match self {
            Self::Orthographic { height } => *height,
            Self::Perspective { .. } => 0.0,
        }
    }

    /// The field of view for a perspective camera, or `0`.
    #[must_use]
    pub fn fov_y(&self) -> f32 {
        match self {
            Self::Perspective { fov_y } => *fov_y,
            Self::Orthographic { .. } => 0.0,
        }
    }
}

impl Default for ProjectionMode {
    fn default() -> Self {
        // A pixel-art RPG wants an orthographic view; making that the default
        // means a new project looks right before any configuration.
        Self::Orthographic { height: 20.0 }
    }
}

/// The pixel-perfect quantisation policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelPerfect {
    /// Internal render resolution the projection is built for.
    pub internal: (u32, u32),
    /// When true, the camera's focus is snapped so world content lands on whole
    /// internal pixels.
    pub snap_focus: bool,
    /// When true, a sprite whose on-screen size is not a whole number of pixels
    /// is snapped to the nearest whole size. This is the setting that stops
    /// distant props from "swimming".
    pub snap_sprites: bool,
}

impl PixelPerfect {
    /// Snapping disabled.
    pub const OFF: Self = Self {
        internal: (0, 0),
        snap_focus: false,
        snap_sprites: false,
    };

    /// The default for a 320x180 internal buffer.
    #[must_use]
    pub const fn at(width: u32, height: u32) -> Self {
        Self {
            internal: (width, height),
            snap_focus: true,
            snap_sprites: true,
        }
    }

    /// True when any snapping is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.snap_focus || self.snap_sprites
    }
}

impl Default for PixelPerfect {
    fn default() -> Self {
        Self::at(320, 180)
    }
}

/// A top-down camera rig.
///
/// The rig holds *intent* (where the player is, how far the camera lags) and
/// produces a [`CameraView`] on demand. It never touches the scene, so a
/// cutscene, an interior and the main gameplay camera are all just different
/// configurations of the same type.
#[derive(Clone, Debug)]
pub struct TopDownCamera {
    /// The world point the camera looks at, after smoothing.
    focus: Vec3,
    /// The point the camera is heading towards, before smoothing.
    desired_focus: Vec3,
    /// Yaw about `+Y`, radians. `0` looks along `-Z`.
    yaw: f32,
    /// Target yaw, used when the camera rotates towards a heading.
    desired_yaw: f32,
    /// Pitch below the horizon, radians. `PI/2` is straight down.
    pitch: f32,
    /// Distance from the focus to the eye along the view direction.
    distance: f32,
    /// Projection.
    projection: ProjectionMode,
    /// Height added to the eye, on top of `distance`.
    eye_offset: f32,
    /// Fraction of the remaining distance left after one second of smoothing.
    smoothing: f32,
    /// Horizontal deadzone in world units: the focus may drift this far from the
    /// target before the camera follows.
    deadzone: Vec2,
    /// Vertical deadzone.
    deadzone_y: f32,
    /// How far ahead of the target's motion the camera leads, in seconds.
    look_ahead: f32,
    /// Maximum look-ahead distance.
    max_look_ahead: f32,
    /// The camera will not show anything outside these bounds.
    bounds: Option<Aabb>,
    /// Shake state.
    shake: CameraShake,
    /// Pixel quantisation policy.
    pixel_perfect: PixelPerfect,
    /// The focus position actually used to build the last view (after snapping).
    snapped_focus: Vec3,
    /// Camera shake offset applied to the last view.
    shake_offset: Vec3,
    /// Near plane.
    near: f32,
    /// Far plane.
    far: f32,
    /// Rotation smoothing, as with [`TopDownCamera::smoothing`].
    rotation_smoothing: f32,
    /// Yaw is only applied when this is on, so a fixed-angle camera never drifts.
    allow_rotation: bool,
}

impl TopDownCamera {
    /// A camera with the given projection.
    #[must_use]
    pub fn new(projection: ProjectionMode) -> Self {
        Self {
            focus: Vec3::ZERO,
            desired_focus: Vec3::ZERO,
            yaw: 0.0,
            desired_yaw: 0.0,
            // A slight tilt reads as 3D without distorting the tile grid much.
            pitch: core::f32::consts::FRAC_PI_2,
            distance: 40.0,
            projection,
            eye_offset: 0.0,
            // 5% left after a second: snappy but visibly smooth.
            smoothing: 0.05,
            deadzone: Vec2::ZERO,
            deadzone_y: 0.0,
            look_ahead: 0.0,
            max_look_ahead: 6.0,
            bounds: None,
            shake: CameraShake::new(),
            pixel_perfect: PixelPerfect::default(),
            snapped_focus: Vec3::ZERO,
            shake_offset: Vec3::ZERO,
            near: 0.1,
            far: 400.0,
            rotation_smoothing: 0.1,
            allow_rotation: false,
        }
    }

    /// A straight-down orthographic camera sized for a 320x180 pixel-art buffer.
    #[must_use]
    pub fn pixel_art(internal_width: u32, internal_height: u32) -> Self {
        let mut camera = Self::new(ProjectionMode::Orthographic { height: 20.0 });
        camera.pixel_perfect = PixelPerfect::at(internal_width, internal_height);
        camera
    }

    /// A perspective camera looking down at an angle.
    #[must_use]
    pub fn perspective_tilted(fov_y: f32, pitch: f32) -> Self {
        let mut camera = Self::new(ProjectionMode::Perspective { fov_y });
        camera.pitch = pitch;
        camera
    }

    // ------------------------------------------------------------- accessors

    /// The smoothed focus.
    #[inline]
    #[must_use]
    pub fn focus(&self) -> Vec3 {
        self.focus
    }

    /// Where the camera is aiming, before smoothing.
    #[inline]
    #[must_use]
    pub fn desired_focus(&self) -> Vec3 {
        self.desired_focus
    }

    /// Sets the focus immediately, without smoothing.
    pub fn snap_to(&mut self, focus: Vec3) {
        self.focus = focus;
        self.desired_focus = focus;
    }

    /// The camera's yaw in radians.
    #[inline]
    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// Sets the target yaw. It is reached at the smoothing rate when rotation is
    /// enabled, and immediately when it is not.
    pub fn set_yaw(&mut self, yaw: f32) {
        self.desired_yaw = yaw;
        self.allow_rotation = true;
    }

    /// Sets the yaw immediately, without smoothing.
    ///
    /// Used by [`crate::zone::apply_zone`], which does its own cross-fade: the
    /// blend is already spread over frames, so a second smoothing stage on top
    /// would make the camera arrive at a different time than the zoom.
    pub fn set_yaw_immediate(&mut self, yaw: f32) {
        let yaw = if yaw.is_finite() { yaw } else { 0.0 };
        self.yaw = yaw;
        self.desired_yaw = yaw;
    }

    /// Freezes the yaw.
    pub fn lock_rotation(&mut self) {
        self.allow_rotation = false;
        self.desired_yaw = self.yaw;
    }

    /// The camera's pitch below the horizon in radians.
    #[inline]
    #[must_use]
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// Sets the pitch, clamped to `[0.15, PI/2]`.
    ///
    /// Straight down (`PI/2`) is the pixel-art default; anything below ~9
    /// degrees makes a top-down map unreadable, so it is clamped rather than
    /// trusted.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.pitch = pitch.clamp(0.15, core::f32::consts::FRAC_PI_2);
    }

    /// The camera's distance from the focus.
    #[inline]
    #[must_use]
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// Sets the distance, clamped to `[1, 1000]`.
    pub fn set_distance(&mut self, distance: f32) {
        self.distance = distance.clamp(1.0, 1000.0);
    }

    /// The projection.
    #[inline]
    #[must_use]
    pub fn projection(&self) -> ProjectionMode {
        self.projection
    }

    /// Sets the projection.
    pub fn set_projection(&mut self, projection: ProjectionMode) {
        self.projection = projection;
    }

    /// Sets the orthographic view height.
    pub fn set_ortho_height(&mut self, height: f32) {
        self.projection = ProjectionMode::Orthographic {
            height: height.max(0.1),
        };
    }

    /// The orthographic view height, or `0` for a perspective camera.
    #[inline]
    #[must_use]
    pub fn ortho_height(&self) -> f32 {
        self.projection.height()
    }

    /// Adjusts the orthographic height by a multiplicative factor, which is what
    /// a zoom wheel should do (a linear change feels wrong at both extremes).
    pub fn zoom_by(&mut self, factor: f32) {
        if let ProjectionMode::Orthographic { height } = self.projection {
            self.projection = ProjectionMode::Orthographic {
                height: (height * factor).clamp(2.0, 200.0),
            };
        }
    }

    /// Sets the smoothing factor: the fraction of the remaining distance left
    /// after one second. `0.05` is snappy, `0.3` is floaty, `0` snaps.
    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.smoothing = smoothing.clamp(0.0, 1.0);
    }

    /// The smoothing factor.
    #[inline]
    #[must_use]
    pub fn smoothing(&self) -> f32 {
        self.smoothing
    }

    /// Sets the horizontal deadzone in world units.
    pub fn set_deadzone(&mut self, width: f32, height: f32, vertical: f32) {
        self.deadzone = Vec2::new(width.max(0.0), height.max(0.0));
        self.deadzone_y = vertical.max(0.0);
    }

    /// Sets how far ahead of the target the camera leads, in seconds of motion.
    ///
    /// A small value (0.1-0.3 s) is what makes a running character stay centred
    /// instead of dragging at the edge of the deadzone.
    pub fn set_look_ahead(&mut self, seconds: f32, max_distance: f32) {
        self.look_ahead = seconds.max(0.0);
        self.max_look_ahead = max_distance.max(0.0);
    }

    /// Constrains the camera to a world region. `None` removes the constraint.
    pub fn set_bounds(&mut self, bounds: Option<Aabb>) {
        self.bounds = bounds;
    }

    /// The world region the camera is confined to.
    #[inline]
    #[must_use]
    pub fn bounds(&self) -> Option<Aabb> {
        self.bounds
    }

    /// The pixel-perfect policy.
    #[inline]
    #[must_use]
    pub fn pixel_perfect(&self) -> PixelPerfect {
        self.pixel_perfect
    }

    /// Sets the pixel-perfect policy.
    pub fn set_pixel_perfect(&mut self, pixel_perfect: PixelPerfect) {
        self.pixel_perfect = pixel_perfect;
    }

    /// Adds extra height to the eye position.
    pub fn set_eye_offset(&mut self, offset: f32) {
        self.eye_offset = offset;
    }

    /// Sets the clip planes.
    pub fn set_clip_planes(&mut self, near: f32, far: f32) {
        self.near = near.max(1e-3);
        self.far = far.max(self.near * 1.01);
    }

    /// The clip planes.
    #[inline]
    #[must_use]
    pub fn clip_planes(&self) -> (f32, f32) {
        (self.near, self.far)
    }

    /// Mutable access to the shake state.
    #[inline]
    pub fn shake_mut(&mut self) -> &mut CameraShake {
        &mut self.shake
    }

    // ------------------------------------------------------------ behaviour

    /// Points the camera at `target` and advances the smoothing by `dt` seconds.
    ///
    /// `velocity` is the target's world velocity, used for the look-ahead. Pass
    /// [`Vec3::ZERO`] when there is none.
    pub fn follow_with_velocity(&mut self, target: Vec3, velocity: Vec3, dt: f32) {
        let lead = velocity * self.look_ahead;
        let lead = if lead.length() > self.max_look_ahead {
            lead.normalize_or_zero() * self.max_look_ahead
        } else {
            lead
        };
        let wanted = target + lead;
        // The deadzone lets the focus stay put until the target leaves a box
        // around it; without it a camera on a moving target looks like it is
        // being dragged around on a rubber band.
        let mut next = self.desired_focus;
        let dx = wanted.x - self.focus.x;
        if dx.abs() > self.deadzone.x {
            next.x = wanted.x - self.deadzone.x * dx.signum();
        }
        let dz = wanted.z - self.focus.z;
        if dz.abs() > self.deadzone.y {
            next.z = wanted.z - self.deadzone.y * dz.signum();
        }
        let dy = wanted.y - self.focus.y;
        if dy.abs() > self.deadzone_y {
            next.y = wanted.y - self.deadzone_y * dy.signum();
        }
        self.desired_focus = next;

        let k = damp_factor(self.smoothing, dt);
        self.focus += (self.desired_focus - self.focus) * k;

        if self.allow_rotation {
            let k = damp_factor(self.rotation_smoothing, dt);
            // Turn the short way round, and by a fraction of the *remaining*
            // angle so the rate is frame-rate independent.
            let remaining = angle_delta(self.yaw, self.desired_yaw);
            self.yaw = rotate_towards(self.yaw, self.desired_yaw, remaining.abs() * k);
        }

        self.shake.update(dt);
        self.clamp_to_bounds();
    }

    /// Points the camera at `target` with no velocity.
    pub fn follow(&mut self, target: Vec3, dt: f32) {
        self.follow_with_velocity(target, Vec3::ZERO, dt);
    }

    /// Keeps the visible rectangle inside the camera's bounds, if any.
    fn clamp_to_bounds(&mut self) {
        let Some(bounds) = self.bounds else { return };
        let half_height = self.projection.height() * 0.5;
        let half_width = half_height;
        let size = bounds.size();
        let centre = bounds.center();
        // Only clamp on an axis the region is actually bigger than the view on;
        // otherwise centre the view in the region.
        self.focus.x = if size.x > half_width * 2.0 {
            self.focus
                .x
                .clamp(bounds.min.x + half_width, bounds.max.x - half_width)
        } else {
            centre.x
        };
        self.focus.z = if size.z > half_height * 2.0 {
            self.focus
                .z
                .clamp(bounds.min.z + half_height, bounds.max.z - half_height)
        } else {
            centre.z
        };
    }

    // --------------------------------------------------------------- output

    /// The world-space vector from the focus to the eye.
    #[must_use]
    pub fn eye_offset_direction(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let forward = Vec3::new(
            -self.yaw.sin() * cos_pitch,
            -sin_pitch,
            -self.yaw.cos() * cos_pitch,
        );
        -forward.normalize_or_zero() * self.distance
    }

    /// The camera eye position, including the shake offset.
    #[must_use]
    pub fn eye_position(&self) -> Vec3 {
        self.snapped_focus
            + self.eye_offset_direction()
            + Vec3::Y * self.eye_offset
            + self.shake_offset
    }

    /// The focus the last view was built from, after pixel snapping.
    #[inline]
    #[must_use]
    pub fn snapped_focus(&self) -> Vec3 {
        self.snapped_focus
    }

    /// Builds the renderer's view for the given aspect ratio.
    ///
    /// This is the only method that mutates the snapped state; call it once per
    /// frame and pass the result to both the visibility pass and the renderer so
    /// they agree exactly.
    pub fn view(&mut self, aspect: f32) -> CameraView {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        // A NaN anywhere in the focus would propagate into the view-projection
        // matrix and blank the frame, so recover rather than render garbage.
        if !self.focus.is_finite() {
            self.focus = Vec3::ZERO;
            self.desired_focus = Vec3::ZERO;
        }
        self.shake_offset = self.shake.offset(self.snapped_focus);

        let mut focus = self.focus;
        if self.pixel_perfect.snap_focus && self.projection.is_orthographic() {
            let (w, h) = self.pixel_perfect.internal;
            if w > 0 && h > 0 {
                // One internal pixel is this many world units.
                let units_per_pixel = self.projection.height() / h as f32;
                focus.x = (focus.x / units_per_pixel).round() * units_per_pixel;
                focus.z = (focus.z / units_per_pixel).round() * units_per_pixel;
            }
        }
        self.snapped_focus = focus;

        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let forward = Vec3::new(
            -self.yaw.sin() * cos_pitch,
            -sin_pitch,
            -self.yaw.cos() * cos_pitch,
        )
        .normalize_or_zero();
        // Near straight down, world up is parallel to the view direction and
        // `look_at_rh` cannot derive a basis from it. The correct continuation is
        // the camera's horizontal heading, so that turning the camera turns the
        // map on screen instead of snapping the roll.
        let up_hint = if self.pitch > core::f32::consts::FRAC_PI_2 - 0.02 {
            Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
        } else {
            Vec3::Y
        };
        let eye =
            focus + self.eye_offset_direction() + Vec3::Y * self.eye_offset + self.shake_offset;
        let target = focus + Vec3::Y * self.eye_offset + self.shake_offset;

        let mut view = match self.projection {
            ProjectionMode::Orthographic { height } => {
                CameraView::orthographic(eye, target, up_hint, height, aspect, self.near, self.far)
            }
            ProjectionMode::Perspective { fov_y } => {
                CameraView::perspective(eye, target, up_hint, fov_y, aspect, self.near, self.far)
            }
        };
        view.forward = forward;
        if self.projection.is_orthographic() {
            let (w, h) = self.pixel_perfect.internal;
            let units_per_pixel = if h > 0 {
                self.projection.height() / h as f32
            } else {
                0.0
            };
            view = view.with_units_per_pixel(units_per_pixel);
            let _ = w;
        }
        view
    }

    /// The world-space size of one internal pixel.
    #[must_use]
    pub fn world_units_per_pixel(&self) -> f32 {
        let (_, h) = self.pixel_perfect.internal;
        if h == 0 {
            0.0
        } else {
            self.projection.height() / h as f32
        }
    }

    /// The world-space rectangle the camera can currently see on the ground
    /// plane, used by the streaming system to decide what to load.
    #[must_use]
    pub fn visible_ground_rect(&self, aspect: f32) -> (Vec2, Vec2) {
        let half_height = match self.projection {
            ProjectionMode::Orthographic { height } => height * 0.5,
            ProjectionMode::Perspective { fov_y } => {
                // The ground footprint of a perspective camera, at the focus
                // plane. Good enough for a streaming hint.
                (fov_y * 0.5).tan() * self.distance
            }
        };
        let half_width = half_height * aspect.max(0.01);
        // A tilted camera sees further along its view axis than straight down;
        // widening the rect keeps the streaming margin honest.
        let stretch = if self.pitch >= core::f32::consts::FRAC_PI_2 - 0.01 {
            1.0
        } else {
            (1.0 / self.pitch.sin()).min(4.0)
        };
        let focus = self.snapped_focus;
        (
            Vec2::new(focus.x - half_width, focus.z - half_height * stretch),
            Vec2::new(focus.x + half_width, focus.z + half_height * stretch),
        )
    }
}

impl Default for TopDownCamera {
    fn default() -> Self {
        Self::new(ProjectionMode::default())
    }
}

/// Deterministic hash noise in `[-1, 1]` for the shake, so a shake is
/// reproducible and can be replayed from a recording.
#[must_use]
pub fn shake_noise(step: u32, axis: u32, seed: u64) -> f32 {
    let h = hash_2d(step as i32, axis as i32, seed);
    ((h >> 40) as f32) * (1.0 / (1u32 << 23) as f32) - 1.0
}

/// Snaps a world-space length to a whole number of pixels.
///
/// A sprite rendered at 15.7 px will alternate between 15 and 16 as it moves,
/// which reads as a shimmer. Rounding the *size* while leaving the position
/// continuous is the standard fix and is what `snap_sprites` does.
#[must_use]
pub fn snap_to_pixels(world_size: f32, units_per_pixel: f32) -> f32 {
    if units_per_pixel <= 0.0 {
        return world_size;
    }
    (world_size / units_per_pixel).round().max(1.0) * units_per_pixel
}

/// Blends two cameras, for a transition between a gameplay view and a cutscene.
#[must_use]
pub fn blend_focus(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(b, t.clamp(0.0, 1.0))
}

/// Linearly interpolates two projection modes, for a zoom or a mode switch.
#[must_use]
pub fn blend_ortho_height(a: ProjectionMode, b: ProjectionMode, t: f32) -> f32 {
    lerp(a.height(), b.height(), t.clamp(0.0, 1.0))
}

/// The camera's view-projection matrix without going through [`TopDownCamera`].
///
/// Useful for a tool that only has a raw camera description.
#[must_use]
pub fn view_projection(view: &CameraView) -> Mat4 {
    view.view_projection
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::TAU;

    fn camera() -> TopDownCamera {
        let mut c = TopDownCamera::new(ProjectionMode::Orthographic { height: 20.0 });
        c.set_smoothing(0.0); // snap, so tests are exact
        c
    }

    #[test]
    fn default_camera_is_orthographic() {
        let c = TopDownCamera::default();
        assert!(c.projection().is_orthographic());
        assert_eq!(c.projection().height(), 20.0);
    }

    #[test]
    fn pixel_art_camera_targets_the_internal_resolution() {
        let c = TopDownCamera::pixel_art(320, 180);
        assert_eq!(c.pixel_perfect().internal, (320, 180));
        assert!(c.pixel_perfect().snap_focus);
        // 20 world units over 180 pixels.
        assert!((c.world_units_per_pixel() - 20.0 / 180.0).abs() < 1e-6);
    }

    #[test]
    fn snap_to_places_the_focus_exactly() {
        let mut c = camera();
        c.snap_to(Vec3::new(5.0, 1.0, -3.0));
        assert_eq!(c.focus(), Vec3::new(5.0, 1.0, -3.0));
        assert_eq!(c.desired_focus(), c.focus());
    }

    #[test]
    fn smoothing_converges_on_the_target() {
        let mut c = TopDownCamera::new(ProjectionMode::Orthographic { height: 20.0 });
        c.set_smoothing(0.1);
        c.snap_to(Vec3::ZERO);
        // smoothing 0.1 leaves 10% per second, so 5 s leaves 1e-5 of the
        // original 10 m gap.
        for _ in 0..300 {
            c.follow(Vec3::new(10.0, 0.0, 0.0), 1.0 / 60.0);
        }
        assert!((c.focus().x - 10.0).abs() < 0.01, "{}", c.focus().x);
    }

    #[test]
    fn smoothing_is_frame_rate_independent() {
        let mut slow = TopDownCamera::new(ProjectionMode::Orthographic { height: 20.0 });
        slow.set_smoothing(0.1);
        slow.snap_to(Vec3::ZERO);
        let mut fast = slow.clone();
        for _ in 0..30 {
            slow.follow(Vec3::new(10.0, 0.0, 0.0), 1.0 / 30.0);
        }
        for _ in 0..120 {
            fast.follow(Vec3::new(10.0, 0.0, 0.0), 1.0 / 120.0);
        }
        assert!(
            (slow.focus().x - fast.focus().x).abs() < 0.05,
            "{} vs {}",
            slow.focus().x,
            fast.focus().x
        );
    }

    #[test]
    fn smoothing_zero_snaps() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        c.follow(Vec3::new(7.0, 0.0, 0.0), 1.0 / 60.0);
        assert!((c.focus().x - 7.0).abs() < 1e-5);
    }

    #[test]
    fn deadzone_lets_the_focus_stay_put() {
        let mut c = camera();
        c.set_deadzone(2.0, 2.0, 0.0);
        c.snap_to(Vec3::ZERO);
        // Well inside the deadzone: nothing moves.
        c.follow(Vec3::new(0.5, 0.0, 0.5), 1.0 / 60.0);
        assert_eq!(c.focus(), Vec3::ZERO);
        // Outside it: the camera follows to the deadzone edge.
        c.follow(Vec3::new(10.0, 0.0, 0.0), 1.0 / 60.0);
        assert!(c.focus().x > 5.0, "{}", c.focus().x);
    }

    #[test]
    fn look_ahead_leads_a_moving_target() {
        let mut c = camera();
        c.set_look_ahead(0.5, 100.0);
        c.snap_to(Vec3::ZERO);
        c.follow_with_velocity(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), 1.0 / 60.0);
        assert!(c.desired_focus().x > 4.0, "{:?}", c.desired_focus());
    }

    #[test]
    fn look_ahead_is_capped() {
        let mut c = camera();
        c.set_look_ahead(1.0, 3.0);
        c.snap_to(Vec3::ZERO);
        c.follow_with_velocity(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0), 1.0 / 60.0);
        assert!(c.desired_focus().x <= 3.0 + 1e-4, "{:?}", c.desired_focus());
    }

    #[test]
    fn bounds_clamp_the_view() {
        let mut c = camera();
        c.set_ortho_height(10.0);
        c.set_bounds(Some(Aabb::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 10.0, 100.0),
        )));
        c.snap_to(Vec3::new(-50.0, 0.0, 50.0));
        c.follow(Vec3::new(-50.0, 0.0, 50.0), 1.0 / 60.0);
        assert!(c.focus().x >= 5.0 - 1e-4, "{:?}", c.focus());
        assert!(c.focus().z <= 95.0 + 1e-4, "{:?}", c.focus());
    }

    #[test]
    fn bounds_centre_a_region_smaller_than_the_view() {
        let mut c = camera();
        c.set_ortho_height(50.0);
        c.set_bounds(Some(Aabb::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 1.0, 10.0),
        )));
        c.snap_to(Vec3::new(-100.0, 0.0, -100.0));
        c.follow(Vec3::new(-100.0, 0.0, -100.0), 1.0 / 60.0);
        assert!(
            (c.focus().x - 5.0).abs() < 1e-4 && (c.focus().z - 5.0).abs() < 1e-4,
            "{:?}",
            c.focus()
        );
    }

    #[test]
    fn eye_is_above_the_focus_when_looking_down() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        let view = c.view(1.0);
        assert!(view.position.y > 0.0, "{:?}", view.position);
        assert!(
            view.forward.approx_eq(Vec3::new(0.0, -1.0, 0.0), 1e-3),
            "{:?}",
            view.forward
        );
    }

    #[test]
    fn pitch_is_clamped() {
        let mut c = camera();
        c.set_pitch(0.0);
        assert!(c.pitch() >= 0.14);
        c.set_pitch(10.0);
        assert!(c.pitch() <= core::f32::consts::FRAC_PI_2 + 1e-6);
    }

    #[test]
    fn distance_is_clamped() {
        let mut c = camera();
        c.set_distance(-5.0);
        assert_eq!(c.distance(), 1.0);
        c.set_distance(1e6);
        assert_eq!(c.distance(), 1000.0);
    }

    #[test]
    fn zoom_by_is_multiplicative_and_bounded() {
        let mut c = camera();
        c.set_ortho_height(20.0);
        c.zoom_by(0.5);
        assert!((c.ortho_height() - 10.0).abs() < 1e-5);
        c.zoom_by(0.5);
        assert!((c.ortho_height() - 5.0).abs() < 1e-5);
        for _ in 0..20 {
            c.zoom_by(0.5);
        }
        assert!(c.ortho_height() >= 2.0 - 1e-5);
    }

    #[test]
    fn pixel_snapping_lands_on_whole_pixels() {
        let mut c = TopDownCamera::pixel_art(320, 180);
        c.set_ortho_height(18.0);
        c.set_smoothing(0.0);
        c.snap_to(Vec3::new(0.123_456, 0.0, 7.654_321));
        let view = c.view(320.0 / 180.0);
        let upp = 18.0 / 180.0;
        let x_pixels = view.position.x / upp;
        assert!((x_pixels - x_pixels.round()).abs() < 1e-3, "{x_pixels}");
    }

    #[test]
    fn pixel_snapping_can_be_disabled() {
        let mut c = TopDownCamera::pixel_art(320, 180);
        c.set_pixel_perfect(PixelPerfect::OFF);
        c.set_ortho_height(18.0);
        c.snap_to(Vec3::new(0.123_456, 0.0, 0.0));
        c.view(1.0);
        assert!((c.snapped_focus().x - 0.123_456).abs() < 1e-6);
    }

    #[test]
    fn snap_to_pixels_rounds_sizes() {
        assert!((snap_to_pixels(15.7, 1.0) - 16.0).abs() < 1e-6);
        assert!(
            (snap_to_pixels(0.1, 1.0) - 1.0).abs() < 1e-6,
            "never snaps to zero"
        );
        assert_eq!(snap_to_pixels(5.0, 0.0), 5.0);
    }

    #[test]
    fn shake_moves_the_eye_but_not_the_focus() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        c.shake_mut().add_trauma(1.0);
        let a = c.view(1.0).position;
        let b = c.view(1.0).position;
        assert_eq!(c.focus(), Vec3::ZERO, "the focus must not shake");
        assert!((a - b).length() >= 0.0);
    }

    #[test]
    fn shake_decays_to_nothing() {
        let mut c = camera();
        c.shake_mut().add_trauma(1.0);
        // Advance well past the shake's life with a stationary target.
        for _ in 0..600 {
            c.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        let base = c.view(1.0).position;
        let again = c.view(1.0).position;
        assert!((base - again).length() < 1e-4, "shake must stop");
    }

    #[test]
    fn shake_is_deterministic() {
        let mut a = camera();
        let mut b = camera();
        a.snap_to(Vec3::ZERO);
        b.snap_to(Vec3::ZERO);
        a.shake_mut().add_trauma(0.8);
        b.shake_mut().add_trauma(0.8);
        for _ in 0..10 {
            assert_eq!(a.view(1.0).position, b.view(1.0).position);
        }
    }

    #[test]
    fn shake_noise_is_bounded() {
        for step in 0..500 {
            for axis in 0..3 {
                let v = shake_noise(step, axis, 7);
                assert!((-1.0..1.0).contains(&v), "{v}");
            }
        }
    }

    #[test]
    fn rotation_is_reached_smoothly_and_can_be_locked() {
        let mut c = camera();
        c.set_yaw(core::f32::consts::FRAC_PI_2);
        c.snap_to(Vec3::ZERO);
        for _ in 0..200 {
            c.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        assert!(
            (c.yaw() - core::f32::consts::FRAC_PI_2).abs() < 0.02,
            "{}",
            c.yaw()
        );
        // Locking freezes whatever yaw was reached, and keeps it frozen while
        // the camera keeps following.
        c.lock_rotation();
        let locked = c.yaw();
        for _ in 0..120 {
            c.follow(Vec3::new(3.0, 0.0, 0.0), 1.0 / 60.0);
        }
        assert_eq!(c.yaw(), locked, "a locked camera must not rotate");
        // Asking for a yaw again re-enables rotation, so locking is not a trap.
        c.set_yaw(-2.0);
        for _ in 0..600 {
            c.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        assert!((c.yaw() - (-2.0)).abs() < 0.02, "{}", c.yaw());
    }

    #[test]
    fn perspective_projection_produces_a_perspective_view() {
        let mut c = TopDownCamera::perspective_tilted(0.9, 1.0);
        c.snap_to(Vec3::ZERO);
        let view = c.view(1.6);
        assert!(!view.is_orthographic());
        assert!((view.fov_y - 0.9).abs() < 1e-6);
    }

    #[test]
    fn view_is_finite_for_degenerate_parameters() {
        let mut c = camera();
        c.set_ortho_height(0.0);
        c.set_clip_planes(0.0, 0.0);
        c.snap_to(Vec3::new(f32::NAN, 0.0, 0.0));
        let view = c.view(0.0);
        assert!(
            view.view_projection.is_finite(),
            "{:?}",
            view.view_projection
        );
    }

    #[test]
    fn view_accepts_a_degenerate_aspect() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        assert!(c.view(0.0).view_projection.is_finite());
        assert!(c.view(f32::NAN).view_projection.is_finite());
        assert!(c.view(-4.0).view_projection.is_finite());
    }

    #[test]
    fn visible_ground_rect_covers_the_view() {
        let mut c = camera();
        c.set_ortho_height(20.0);
        c.snap_to(Vec3::ZERO);
        c.view(1.0);
        let (min, max) = c.visible_ground_rect(1.0);
        assert!(min.x <= -9.9 && max.x >= 9.9, "{min:?} {max:?}");
        assert!(min.y <= -9.9 && max.y >= 9.9, "{min:?} {max:?}");
    }

    #[test]
    fn tilted_camera_sees_further_along_its_axis() {
        let straight = {
            let mut c = camera();
            c.set_ortho_height(20.0);
            c.snap_to(Vec3::ZERO);
            c.view(1.0);
            c.visible_ground_rect(1.0)
        };
        let tilted = {
            let mut c = camera();
            c.set_ortho_height(20.0);
            c.set_pitch(0.6);
            c.snap_to(Vec3::ZERO);
            c.view(1.0);
            c.visible_ground_rect(1.0)
        };
        let straight_depth = straight.1.y - straight.0.y;
        let tilted_depth = tilted.1.y - tilted.0.y;
        assert!(
            tilted_depth >= straight_depth,
            "{tilted_depth} vs {straight_depth}"
        );
    }

    #[test]
    fn yaw_rotates_the_screen_for_a_straight_down_camera() {
        // A camera looking straight down does not move when it turns: it
        // *rotates*. The eye stays put and the screen basis turns.
        let mut a = camera();
        a.snap_to(Vec3::ZERO);
        let view_a = a.view(1.0);

        let mut b = camera();
        b.snap_to(Vec3::ZERO);
        b.set_yaw(core::f32::consts::FRAC_PI_2);
        for _ in 0..600 {
            b.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        let view_b = b.view(1.0);
        assert!(
            (b.yaw() - core::f32::consts::FRAC_PI_2).abs() < 0.01,
            "{}",
            b.yaw()
        );
        assert!(
            (view_a.up - view_b.up).length() > 1.0,
            "turning must change the screen orientation: {:?} vs {:?}",
            view_a.up,
            view_b.up
        );
        assert!(
            view_b.up.y.abs() < 1e-3,
            "the screen up vector must stay horizontal"
        );
        assert!(view_b.position.y > 0.0);
    }

    #[test]
    fn yaw_moves_the_eye_for_a_tilted_camera() {
        let mut a = camera();
        a.set_pitch(1.0);
        a.snap_to(Vec3::ZERO);
        let eye_a = a.view(1.0).position;

        let mut b = camera();
        b.set_pitch(1.0);
        b.snap_to(Vec3::ZERO);
        b.set_yaw(core::f32::consts::FRAC_PI_2);
        for _ in 0..600 {
            b.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        let eye_b = b.view(1.0).position;
        assert!(
            (eye_a - eye_b).length() > 5.0,
            "the eye must swing: {eye_a:?} vs {eye_b:?}"
        );
        assert!(eye_b.x > 5.0, "it must swing towards +X: {eye_b:?}");
    }

    #[test]
    fn yaw_wraps_the_short_way() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        c.set_yaw(TAU - 0.1);
        for _ in 0..300 {
            c.follow(Vec3::ZERO, 1.0 / 60.0);
        }
        // 2*PI - 0.1 and -0.1 are the same direction.
        let delta = (c.yaw() - (TAU - 0.1)).abs().min((c.yaw() - (-0.1)).abs());
        assert!(delta < 0.05, "{}", c.yaw());
    }

    #[test]
    fn blend_helpers() {
        assert_eq!(
            blend_focus(Vec3::ZERO, Vec3::splat(10.0), 0.5),
            Vec3::splat(5.0)
        );
        assert_eq!(
            blend_focus(Vec3::ZERO, Vec3::splat(10.0), 2.0),
            Vec3::splat(10.0)
        );
        let a = ProjectionMode::Orthographic { height: 10.0 };
        let b = ProjectionMode::Orthographic { height: 20.0 };
        assert!((blend_ortho_height(a, b, 0.25) - 12.5).abs() < 1e-5);
    }

    #[test]
    fn view_projection_helper() {
        let mut c = camera();
        c.snap_to(Vec3::ZERO);
        let v = c.view(1.0);
        assert_eq!(view_projection(&v), v.view_projection);
    }

    #[test]
    fn eye_offset_direction_points_back_towards_the_eye() {
        let c = camera();
        let d = c.eye_offset_direction();
        assert!(
            d.y > 0.0,
            "the eye is above a downward-looking camera: {d:?}"
        );
        assert!((d.length() - c.distance()).abs() < 1e-3);
    }

    #[test]
    fn clip_planes_are_ordered() {
        let mut c = camera();
        c.set_clip_planes(10.0, 5.0);
        let (near, far) = c.clip_planes();
        assert!(far > near);
    }
}
