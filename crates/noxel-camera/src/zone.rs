//! Camera zones: per-region overrides.
//!
//! A town, an interior, a boss arena and a cutscene all want slightly different
//! camera behaviour, and threading a branch through the gameplay code for each
//! is how camera logic becomes unmaintainable. Instead, a zone is an axis-aligned
//! region plus the settings it overrides; the camera picks the highest-priority
//! zone containing its focus and blends towards it.
//!
//! Blending is deliberate: snapping between an outdoor zoom and an indoor zoom
//! is the single most jarring thing a top-down camera can do.

use noxel_core::math::{Aabb, Vec3, damp_factor, lerp};

use crate::{ProjectionMode, TopDownCamera};

/// What a zone does when the camera enters it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ZoneKind {
    /// Ordinary outdoor follow.
    #[default]
    Follow,
    /// The camera stops moving and frames a fixed point.
    Fixed,
    /// The camera follows but its yaw is pinned.
    LockedYaw,
    /// An interior: tighter framing, usually a lower ceiling.
    Interior,
    /// A cutscene: the game drives the camera and the zone only sets the look.
    Cutscene,
}

impl ZoneKind {
    /// A short label for the debug overlay.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Fixed => "fixed",
            Self::LockedYaw => "locked",
            Self::Interior => "interior",
            Self::Cutscene => "cutscene",
        }
    }
}

/// A region with camera overrides.
#[derive(Clone, Debug)]
pub struct CameraZone {
    /// Name, for the overlay and for scripting.
    pub name: String,
    /// The region the zone applies to, in world space.
    pub bounds: Aabb,
    /// What the zone does.
    pub kind: ZoneKind,
    /// Higher wins when zones overlap.
    pub priority: i32,
    /// Overrides the projection.
    pub projection: Option<ProjectionMode>,
    /// Pins the yaw.
    pub yaw: Option<f32>,
    /// Overrides the pitch.
    pub pitch: Option<f32>,
    /// Overrides the follow smoothing.
    pub smoothing: Option<f32>,
    /// Multiplies the orthographic height (or the distance) on entry.
    pub zoom: Option<f32>,
    /// Overrides the camera's world bounds while inside.
    pub camera_bounds: Option<Aabb>,
    /// When set, the camera focuses here instead of on the follow target.
    pub fixed_focus: Option<Vec3>,
}

impl CameraZone {
    /// A zone with default settings.
    #[must_use]
    pub fn new(name: impl Into<String>, bounds: Aabb) -> Self {
        Self {
            name: name.into(),
            bounds,
            kind: ZoneKind::Follow,
            priority: 0,
            projection: None,
            yaw: None,
            pitch: None,
            smoothing: None,
            zoom: None,
            camera_bounds: None,
            fixed_focus: None,
        }
    }

    /// An interior zone: tighter framing and a fixed yaw.
    #[must_use]
    pub fn interior(name: impl Into<String>, bounds: Aabb, yaw: f32) -> Self {
        Self {
            kind: ZoneKind::Interior,
            priority: 10,
            yaw: Some(yaw),
            zoom: Some(0.75),
            ..Self::new(name, bounds)
        }
    }

    /// A cutscene zone that frames a fixed point.
    #[must_use]
    pub fn cutscene(name: impl Into<String>, bounds: Aabb, focus: Vec3) -> Self {
        Self {
            kind: ZoneKind::Cutscene,
            priority: 100,
            fixed_focus: Some(focus),
            ..Self::new(name, bounds)
        }
    }

    /// Sets the priority.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Sets a zoom multiplier.
    #[must_use]
    pub fn with_zoom(mut self, zoom: f32) -> Self {
        self.zoom = Some(zoom.max(0.05));
        self
    }

    /// True when `point` is inside the zone.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        self.bounds.contains_point(point)
    }

    /// A one-line description for the debug overlay.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} [{}] priority {}",
            self.name,
            self.kind.label(),
            self.priority
        )
    }
}

/// The zone the camera is currently in, and how far it has blended in.
#[derive(Clone, Debug, Default)]
pub struct ZoneBlend {
    /// Index into the zone list, or `None` when outdoors.
    active: Option<usize>,
    /// Blend in `[0, 1]`.
    blend: f32,
    /// How fast to blend, in units of "fraction remaining after one second".
    speed: f32,
    /// Scratch: the zone being blended out of.
    previous: Option<usize>,
}

impl ZoneBlend {
    /// Creates a blend tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: None,
            blend: 0.0,
            speed: 0.1,
            previous: None,
        }
    }

    /// Sets the blend rate.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.0, 1.0);
    }

    /// The index of the active zone.
    #[inline]
    #[must_use]
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// The blend amount, `1` when fully inside the active zone.
    #[inline]
    #[must_use]
    pub fn amount(&self) -> f32 {
        self.blend
    }

    /// True when the camera has fully settled into a zone.
    #[inline]
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.blend >= 0.999
    }

    /// Updates which zone is active for `focus` and advances the blend.
    pub fn update(&mut self, zones: &[CameraZone], focus: Vec3, dt: f32) {
        let found = select_zone(zones, focus).map(|(i, _)| i);
        if found != self.active {
            self.previous = self.active;
            self.active = found;
            // Restart the blend when moving between zones, but keep the current
            // amount so an A -> B -> A walk does not flash.
            self.blend = if found.is_none() { self.blend } else { 0.0 };
        }
        let target = if self.active.is_some() { 1.0 } else { 0.0 };
        let k = damp_factor(self.speed, dt.max(0.0));
        self.blend += (target - self.blend) * k;
        // Snap at a tenth of a percent: the remaining difference is far below
        // one alpha unit, and a blend that never quite settles keeps the camera
        // doing work forever.
        if self.blend < 1e-3 {
            self.blend = 0.0;
        }
        if self.blend > 1.0 - 1e-3 {
            self.blend = 1.0;
            self.previous = None;
        }
    }

    /// Forces the blend state, for a teleport or a scene load.
    pub fn reset(&mut self, zones: &[CameraZone], focus: Vec3) {
        self.active = select_zone(zones, focus).map(|(i, _)| i);
        self.blend = if self.active.is_some() { 1.0 } else { 0.0 };
        self.previous = None;
    }
}

/// Picks the highest-priority zone containing `point`.
///
/// Ties are broken by the smaller region, which is what you want when a small
/// "doorway" zone sits inside a large "town" zone at the same priority.
#[must_use]
pub fn select_zone(zones: &[CameraZone], point: Vec3) -> Option<(usize, &CameraZone)> {
    zones
        .iter()
        .enumerate()
        .filter(|(_, z)| z.contains(point))
        .min_by(|(_, a), (_, b)| {
            b.priority.cmp(&a.priority).then_with(|| {
                a.bounds
                    .volume()
                    .partial_cmp(&b.bounds.volume())
                    .unwrap_or(core::cmp::Ordering::Equal)
            })
        })
}

/// Applies a zone's overrides to a camera, blended by `t`.
///
/// `t = 0` leaves the camera untouched, `t = 1` fully applies the zone.
pub fn apply_zone(camera: &mut TopDownCamera, zone: &CameraZone, t: f32) {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return;
    }
    if let Some(projection) = zone.projection {
        camera.set_projection(match (camera.projection(), projection) {
            (
                ProjectionMode::Orthographic { height: from },
                ProjectionMode::Orthographic { height: to },
            ) => ProjectionMode::Orthographic {
                height: lerp(from, to, t),
            },
            _ => projection,
        });
    }
    if let Some(zoom) = zone.zoom {
        let factor = lerp(1.0, zoom, t);
        if let ProjectionMode::Orthographic { height } = camera.projection() {
            camera.set_ortho_height(height * factor);
        }
    }
    if let Some(yaw) = zone.yaw {
        // Blending a yaw needs the short way round, not a raw lerp. The blend is
        // spread over frames by the caller, so the value is written directly and
        // the camera's own rotation smoothing is not applied on top of it.
        let current = camera.yaw();
        let delta = noxel_core::math::angle_delta(current, yaw);
        camera.set_yaw_immediate(current + delta * t);
    }
    if let Some(pitch) = zone.pitch {
        camera.set_pitch(lerp(camera.pitch(), pitch, t));
    }
    if let Some(smoothing) = zone.smoothing {
        camera.set_smoothing(lerp(camera.smoothing(), smoothing, t));
    }
    if let Some(bounds) = zone.camera_bounds {
        camera.set_bounds(Some(bounds));
    }
    if let Some(focus) = zone.fixed_focus {
        let target = camera.focus().lerp(focus, t);
        camera.snap_to(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(min: Vec3, max: Vec3) -> Aabb {
        Aabb::new(min, max)
    }

    fn town() -> CameraZone {
        CameraZone::new("town", aabb(Vec3::splat(-100.0), Vec3::splat(100.0))).with_priority(1)
    }

    #[test]
    fn zone_contains_its_bounds() {
        let z = town();
        assert!(z.contains(Vec3::ZERO));
        assert!(!z.contains(Vec3::splat(500.0)));
    }

    #[test]
    fn selects_the_only_matching_zone() {
        let zones = vec![town()];
        let (i, z) = select_zone(&zones, Vec3::ZERO).unwrap();
        assert_eq!(i, 0);
        assert_eq!(z.name, "town");
        assert!(select_zone(&zones, Vec3::splat(500.0)).is_none());
    }

    #[test]
    fn higher_priority_wins() {
        let small =
            CameraZone::new("shop", aabb(Vec3::splat(-5.0), Vec3::splat(5.0))).with_priority(10);
        let zones = vec![town(), small];
        let (i, _) = select_zone(&zones, Vec3::ZERO).unwrap();
        assert_eq!(i, 1);
        // Outside the shop, the town takes over.
        let (i, _) = select_zone(&zones, Vec3::new(50.0, 0.0, 0.0)).unwrap();
        assert_eq!(i, 0);
    }

    #[test]
    fn ties_break_towards_the_smaller_region() {
        let big = CameraZone::new("big", aabb(Vec3::splat(-100.0), Vec3::splat(100.0)));
        let small = CameraZone::new("small", aabb(Vec3::splat(-1.0), Vec3::splat(1.0)));
        let zones = vec![big, small];
        let (i, _) = select_zone(&zones, Vec3::ZERO).unwrap();
        assert_eq!(i, 1);
    }

    #[test]
    fn blend_advances_towards_the_active_zone() {
        let zones = vec![town()];
        let mut blend = ZoneBlend::new();
        blend.set_speed(0.05);
        for _ in 0..300 {
            blend.update(&zones, Vec3::ZERO, 1.0 / 60.0);
        }
        assert_eq!(blend.active(), Some(0));
        assert!(blend.is_settled(), "{}", blend.amount());
    }

    #[test]
    fn blend_returns_to_zero_outside() {
        let zones = vec![town()];
        let mut blend = ZoneBlend::new();
        blend.reset(&zones, Vec3::ZERO);
        assert_eq!(blend.amount(), 1.0);
        for _ in 0..300 {
            blend.update(&zones, Vec3::splat(500.0), 1.0 / 60.0);
        }
        assert_eq!(blend.active(), None);
        assert_eq!(blend.amount(), 0.0);
    }

    #[test]
    fn blend_is_frame_rate_independent() {
        let zones = vec![town()];
        let mut slow = ZoneBlend::new();
        let mut fast = ZoneBlend::new();
        slow.set_speed(0.1);
        fast.set_speed(0.1);
        for _ in 0..30 {
            slow.update(&zones, Vec3::ZERO, 1.0 / 30.0);
        }
        for _ in 0..60 {
            fast.update(&zones, Vec3::ZERO, 1.0 / 60.0);
        }
        assert!(
            (slow.amount() - fast.amount()).abs() < 0.02,
            "{} vs {}",
            slow.amount(),
            fast.amount()
        );
    }

    #[test]
    fn reset_snaps_the_blend() {
        let zones = vec![town()];
        let mut blend = ZoneBlend::new();
        blend.reset(&zones, Vec3::ZERO);
        assert_eq!(blend.active(), Some(0));
        assert_eq!(blend.amount(), 1.0);
        blend.reset(&zones, Vec3::splat(500.0));
        assert_eq!(blend.active(), None);
        assert_eq!(blend.amount(), 0.0);
    }

    #[test]
    fn apply_zone_at_zero_changes_nothing() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.set_ortho_height(20.0);
        camera.set_smoothing(0.05);
        let zone = CameraZone::interior("shop", aabb(Vec3::splat(-5.0), Vec3::splat(5.0)), 1.0);
        apply_zone(&mut camera, &zone, 0.0);
        assert_eq!(camera.ortho_height(), 20.0);
        assert_eq!(camera.smoothing(), 0.05);
    }

    #[test]
    fn apply_zone_at_one_applies_everything() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.set_ortho_height(20.0);
        camera.lock_rotation();
        let zone = CameraZone::interior("shop", aabb(Vec3::splat(-5.0), Vec3::splat(5.0)), 1.0);
        apply_zone(&mut camera, &zone, 1.0);
        assert!(
            (camera.ortho_height() - 15.0).abs() < 1e-4,
            "{}",
            camera.ortho_height()
        );
        assert!((camera.yaw() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn apply_zone_blends_the_zoom() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.set_ortho_height(20.0);
        let zone = CameraZone::new("z", aabb(Vec3::splat(-5.0), Vec3::splat(5.0))).with_zoom(0.5);
        apply_zone(&mut camera, &zone, 0.5);
        assert!(
            (camera.ortho_height() - 15.0).abs() < 1e-4,
            "{}",
            camera.ortho_height()
        );
    }

    #[test]
    fn apply_zone_takes_the_short_way_round_a_yaw() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.set_yaw(0.1);
        let zone = CameraZone {
            yaw: Some(-0.1),
            ..CameraZone::new("z", aabb(Vec3::splat(-5.0), Vec3::splat(5.0)))
        };
        apply_zone(&mut camera, &zone, 1.0);
        assert!((camera.yaw() - (-0.1)).abs() < 1e-4, "{}", camera.yaw());

        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.set_yaw(3.0);
        let zone = CameraZone {
            yaw: Some(-3.0),
            ..CameraZone::new("z", aabb(Vec3::splat(-5.0), Vec3::splat(5.0)))
        };
        apply_zone(&mut camera, &zone, 1.0);
        // The short way from +3 to -3 goes through PI, not through zero.
        assert!(camera.yaw().abs() > 2.9, "{}", camera.yaw());
    }

    #[test]
    fn cutscene_zone_frames_a_fixed_point() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        camera.snap_to(Vec3::ZERO);
        let zone = CameraZone::cutscene(
            "intro",
            aabb(Vec3::splat(-50.0), Vec3::splat(50.0)),
            Vec3::new(10.0, 0.0, 10.0),
        );
        apply_zone(&mut camera, &zone, 1.0);
        assert!(
            camera.focus().approx_eq(Vec3::new(10.0, 0.0, 10.0), 1e-4),
            "{:?}",
            camera.focus()
        );
    }

    #[test]
    fn zone_camera_bounds_are_applied() {
        let mut camera = TopDownCamera::pixel_art(320, 180);
        let zone = CameraZone {
            camera_bounds: Some(aabb(Vec3::ZERO, Vec3::splat(10.0))),
            ..CameraZone::new("z", aabb(Vec3::splat(-5.0), Vec3::splat(5.0)))
        };
        apply_zone(&mut camera, &zone, 1.0);
        assert_eq!(camera.bounds(), Some(aabb(Vec3::ZERO, Vec3::splat(10.0))));
    }

    #[test]
    fn describe_mentions_the_kind() {
        let z = CameraZone::interior("shop", aabb(Vec3::ZERO, Vec3::ONE), 0.0);
        let d = z.describe();
        assert!(d.contains("shop") && d.contains("interior"));
        assert_eq!(z.kind.label(), "interior");
    }

    #[test]
    fn interior_zone_defaults() {
        let z = CameraZone::interior("shop", aabb(Vec3::ZERO, Vec3::ONE), 1.5);
        assert_eq!(z.kind, ZoneKind::Interior);
        assert!(z.priority >= 10);
        assert_eq!(z.yaw, Some(1.5));
        assert!(z.zoom.is_some_and(|v| v < 1.0));
    }

    #[test]
    fn empty_zone_list_is_handled() {
        let mut blend = ZoneBlend::new();
        blend.update(&[], Vec3::ZERO, 0.016);
        assert_eq!(blend.active(), None);
        assert!(select_zone(&[], Vec3::ZERO).is_none());
    }
}
