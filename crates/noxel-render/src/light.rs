//! Lights.
//!
//! The rasterizer evaluates lights per fragment (forward shading) with a single
//! shadow-casting directional light; the ray tracer evaluates every light per
//! ray. Both read the same list, so switching renderers does not change the
//! scene.
//!
//! # Why one shadow-casting sun
//!
//! A top-down pixel-art game is lit by a day/night cycle and a handful of
//! local sources. Shadow maps cost a full depth pass each; one map at a time is
//! what keeps the frame budget predictable. Point-light shadows go through the
//! ray tracer instead, where the cost is per-sample and can be budgeted.
//! See `docs/guides/lighting.md`.

use noxel_core::math::{Color, Vec3};

/// How much a light's contribution falls off with distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Falloff {
    /// `1 / (1 + d^2)` — physical, never reaches zero. The ray tracer's default.
    #[default]
    InverseSquare,
    /// Smooth `1 - (d/range)^2` clamped at zero, so a light can be excluded by
    /// a range query and its influence truly stops. The rasterizer's default.
    ///
    /// A hard cut-off is visible on a smooth surface unless the falloff is
    /// smooth, which is why the quadratic form is used rather than a linear
    /// ramp.
    SmoothRange,
    /// No falloff at all (a directional light, or an ambient fill).
    None,
}

/// A light source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// An infinitely distant light: the sun or moon.
    Directional {
        /// Direction the light travels **towards** the scene (points down at
        /// midday: `Vec3::new(-0.4, -1.0, -0.3)`).
        direction: Vec3,
        /// Linear colour.
        color: Color,
        /// Intensity multiplier.
        intensity: f32,
        /// Whether this light casts a shadow map in the rasterizer.
        cast_shadow: bool,
        /// Depth-buffer bias, in world units, to suppress shadow acne.
        shadow_bias: f32,
        /// Normal-offset applied along the surface normal before the shadow
        /// lookup. Larger values remove acne at the cost of peter-panning.
        normal_bias: f32,
        /// Half-extent of the orthographic shadow volume that follows the
        /// camera, in metres. Smaller is crisper.
        shadow_extent: f32,
    },
    /// A point light with no directionality.
    Point {
        /// World position.
        position: Vec3,
        /// Linear colour.
        color: Color,
        /// Intensity multiplier.
        intensity: f32,
        /// Effective range in metres; zero means unbounded.
        range: f32,
        /// Distance falloff curve.
        falloff: Falloff,
        /// Whether the ray tracer should test visibility for this light.
        cast_shadow: bool,
    },
    /// A cone light (torch, street lamp, spotlight).
    Spot {
        /// World position.
        position: Vec3,
        /// Direction the cone points.
        direction: Vec3,
        /// Linear colour.
        color: Color,
        /// Intensity multiplier.
        intensity: f32,
        /// Effective range in metres.
        range: f32,
        /// Cosine of the inner cone angle, inside which the light is at full
        /// strength.
        cos_inner: f32,
        /// Cosine of the outer cone angle, beyond which there is no light.
        cos_outer: f32,
        /// Distance falloff curve.
        falloff: Falloff,
        /// Whether the ray tracer should test visibility for this light.
        cast_shadow: bool,
    },
}

impl Light {
    /// A sun pointing down and slightly to one side, the canonical top-down
    /// key light.
    #[must_use]
    pub fn sun() -> Self {
        Self::Directional {
            direction: Vec3::new(-0.35, -1.0, -0.25).normalize_or_zero(),
            color: Color::rgb(1.0, 0.97, 0.9),
            intensity: 1.0,
            cast_shadow: true,
            shadow_bias: 0.0015,
            normal_bias: 0.02,
            shadow_extent: 40.0,
        }
    }

    /// A point light with the given range.
    #[must_use]
    pub fn point(position: Vec3, color: Color, intensity: f32, range: f32) -> Self {
        Self::Point {
            position,
            color,
            intensity,
            range,
            falloff: Falloff::SmoothRange,
            cast_shadow: false,
        }
    }

    /// A torch: a warm cone with a wide outer angle.
    #[must_use]
    pub fn torch(
        position: Vec3,
        direction: Vec3,
        color: Color,
        intensity: f32,
        range: f32,
    ) -> Self {
        Self::Spot {
            position,
            direction: direction.normalize_or_zero(),
            color,
            intensity,
            range,
            cos_inner: 0.85,
            cos_outer: 0.55,
            falloff: Falloff::SmoothRange,
            cast_shadow: true,
        }
    }

    /// Whether this light should be considered for shadows.
    #[must_use]
    pub fn casts_shadow(&self) -> bool {
        match self {
            Self::Directional { cast_shadow, .. }
            | Self::Point { cast_shadow, .. }
            | Self::Spot { cast_shadow, .. } => *cast_shadow,
        }
    }

    /// The light's linear colour multiplied by its intensity.
    #[must_use]
    pub fn radiant_color(&self) -> Color {
        let (color, intensity) = match *self {
            Self::Directional {
                color, intensity, ..
            }
            | Self::Point {
                color, intensity, ..
            }
            | Self::Spot {
                color, intensity, ..
            } => (color, intensity),
        };
        color.tint(intensity)
    }

    /// The direction the light travels, for directional and spot lights.
    ///
    /// Point lights return `None` because they have no direction.
    #[must_use]
    pub fn direction(&self) -> Option<Vec3> {
        match *self {
            Self::Directional { direction, .. } => Some(direction.normalize_or_zero()),
            Self::Spot { direction, .. } => Some(direction.normalize_or_zero()),
            Self::Point { .. } => None,
        }
    }

    /// The light's position, for point and spot lights.
    #[must_use]
    pub fn position(&self) -> Option<Vec3> {
        match *self {
            Self::Point { position, .. } | Self::Spot { position, .. } => Some(position),
            Self::Directional { .. } => None,
        }
    }

    /// The effective range, or `f32::INFINITY` for a directional light.
    #[must_use]
    pub fn range(&self) -> f32 {
        match *self {
            Self::Directional { .. } => f32::INFINITY,
            Self::Point { range, .. } | Self::Spot { range, .. } => range,
        }
    }

    /// Whether the light can influence anything at the given bounds.
    ///
    /// The renderer uses this to skip lights that cannot affect a draw call.
    #[must_use]
    pub fn affects_bounds(&self, center: Vec3, radius: f32) -> bool {
        match *self {
            Self::Directional { .. } => true,
            Self::Point {
                position, range, ..
            }
            | Self::Spot {
                position, range, ..
            } => {
                let r = if range <= 0.0 { f32::INFINITY } else { range };
                position.distance(center) <= r + radius
            }
        }
    }

    /// The light's contribution to a surface point, ignoring shadowing.
    ///
    /// `normal` must be unit length. Returns a linear RGB multiplier.
    ///
    /// Both renderers share this so a scene lit by the rasterizer and a scene
    /// lit by the ray tracer agree on brightness; any divergence is then purely
    /// a shadowing/ambient difference, which is far easier to debug.
    #[must_use]
    pub fn radiance_at(&self, point: Vec3, normal: Vec3) -> [f32; 3] {
        let radiant = self.radiant_color();
        let base = [radiant.r, radiant.g, radiant.b];
        match *self {
            Self::Directional { direction, .. } => {
                let l = -direction.normalize_or_zero();
                let n_dot_l = normal.dot(l).max(0.0);
                [base[0] * n_dot_l, base[1] * n_dot_l, base[2] * n_dot_l]
            }
            Self::Point {
                position,
                range,
                falloff,
                ..
            } => {
                let delta = position - point;
                let distance = delta.length();
                if distance < 1e-5 {
                    return base;
                }
                let l = delta * (1.0 / distance);
                let n_dot_l = normal.dot(l).max(0.0);
                let atten = attenuation(falloff, distance, range);
                [
                    base[0] * n_dot_l * atten,
                    base[1] * n_dot_l * atten,
                    base[2] * n_dot_l * atten,
                ]
            }
            Self::Spot {
                position,
                direction,
                range,
                cos_inner,
                cos_outer,
                falloff,
                ..
            } => {
                let delta = position - point;
                let distance = delta.length();
                if distance < 1e-5 {
                    return base;
                }
                let l = delta * (1.0 / distance);
                let n_dot_l = normal.dot(l).max(0.0);
                if n_dot_l <= 0.0 {
                    return [0.0; 3];
                }
                // Cone factor: 1 inside the inner angle, 0 outside the outer, a
                // smooth ramp between.
                let cos_angle = (-l).dot(direction.normalize_or_zero());
                let angular = if cos_angle <= cos_outer {
                    0.0
                } else if cos_angle >= cos_inner {
                    1.0
                } else {
                    let t = (cos_angle - cos_outer) / (cos_inner - cos_outer);
                    t * t * (3.0 - 2.0 * t)
                };
                let atten = attenuation(falloff, distance, range) * angular;
                [
                    base[0] * n_dot_l * atten,
                    base[1] * n_dot_l * atten,
                    base[2] * n_dot_l * atten,
                ]
            }
        }
    }
}

/// Distance attenuation for a light.
#[must_use]
pub fn attenuation(falloff: Falloff, distance: f32, range: f32) -> f32 {
    match falloff {
        Falloff::None => 1.0,
        Falloff::InverseSquare => {
            // The +1 keeps the value at 1.0 at zero distance instead of
            // diverging; the range, when set, still cuts the light off so the
            // renderer can cull it.
            let base = 1.0 / (1.0 + distance * distance);
            if range > 0.0 && distance > range {
                0.0
            } else {
                base
            }
        }
        Falloff::SmoothRange => {
            if range <= 0.0 {
                1.0
            } else if distance >= range {
                0.0
            } else {
                let t = distance / range;
                let s = 1.0 - t * t;
                s * s
            }
        }
    }
}

/// The scene's ambient terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ambient {
    /// Uniform ambient colour, added everywhere.
    pub sky: Color,
    /// Ambient colour reflected from below (ground bounce).
    pub ground: Color,
    /// How strongly the surface normal blends `sky` and `ground`. Zero disables
    /// the hemisphere blend and uses `sky` alone.
    pub hemisphere: f32,
    /// Overall multiplier.
    pub intensity: f32,
}

impl Default for Ambient {
    fn default() -> Self {
        Self {
            // A cool sky / warm ground pair reads as daylight without any
            // actual lights, which keeps an unlit scene from being pitch black.
            sky: Color::rgb(0.35, 0.42, 0.55),
            ground: Color::rgb(0.20, 0.18, 0.15),
            hemisphere: 0.65,
            intensity: 1.0,
        }
    }
}

impl Ambient {
    /// A dark night ambient.
    #[must_use]
    pub fn night() -> Self {
        Self {
            sky: Color::rgb(0.06, 0.08, 0.16),
            ground: Color::rgb(0.03, 0.03, 0.05),
            hemisphere: 0.7,
            intensity: 1.0,
        }
    }

    /// A bright indoor ambient.
    #[must_use]
    pub fn interior() -> Self {
        Self {
            sky: Color::rgb(0.22, 0.20, 0.18),
            ground: Color::rgb(0.12, 0.10, 0.09),
            hemisphere: 0.3,
            intensity: 1.0,
        }
    }

    /// The ambient contribution for a surface normal.
    #[must_use]
    pub fn radiance_at(&self, normal: Vec3) -> [f32; 3] {
        // `up` is 1 for a normal pointing at the sky and 0 for one pointing at
        // the ground. The blend must start at `sky` and move towards `ground`.
        let up = (normal.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let t = (1.0 - up) * self.hemisphere.clamp(0.0, 1.0);
        let c = self.sky.lerp(self.ground, t).tint(self.intensity);
        [c.r, c.g, c.b]
    }

    /// The ambient contribution with no directional information (used for
    /// emissive-only passes and for the ray tracer's miss colour).
    #[must_use]
    pub fn flat(&self) -> [f32; 3] {
        let c = self.sky.lerp(self.ground, 0.5).tint(self.intensity);
        [c.r, c.g, c.b]
    }
}

/// Distance fog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// Linear colour the scene fades to.
    pub color: Color,
    /// Distance at which fog starts.
    pub start: f32,
    /// Distance at which fog is total.
    pub end: f32,
    /// Height above which fog thins out; `0` disables height fog.
    pub height_falloff: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            color: Color::rgb(0.45, 0.5, 0.62),
            start: 40.0,
            end: 120.0,
            height_falloff: 0.0,
        }
    }
}

impl Fog {
    /// The fog factor at a world position, `0` (clear) to `1` (fully fogged).
    #[must_use]
    pub fn factor_at(&self, position: Vec3, camera_position: Vec3) -> f32 {
        let distance = position.distance(camera_position);
        let mut f = if self.end <= self.start {
            if distance >= self.end { 1.0 } else { 0.0 }
        } else {
            ((distance - self.start) / (self.end - self.start)).clamp(0.0, 1.0)
        };
        if self.height_falloff > 0.0 {
            // Thin the fog out above the reference height.
            let h = (position.y * self.height_falloff).clamp(0.0, 1.0);
            f *= 1.0 - h;
        }
        f
    }

    /// Blends `rgb` towards the fog colour.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3], factor: f32) -> [f32; 3] {
        if factor <= 0.0 {
            return rgb;
        }
        let f = factor.clamp(0.0, 1.0);
        [
            rgb[0] + (self.color.r - rgb[0]) * f,
            rgb[1] + (self.color.g - rgb[1]) * f,
            rgb[2] + (self.color.b - rgb[2]) * f,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_points_downwards() {
        let sun = Light::sun();
        let d = sun.direction().unwrap();
        assert!(d.y < 0.0, "the sun must shine downward: {d:?}");
        assert!(sun.casts_shadow());
        assert_eq!(sun.range(), f32::INFINITY);
        assert!(sun.position().is_none());
    }

    #[test]
    fn directional_radiance_follows_the_normal() {
        let sun = Light::sun();
        let up = sun.radiance_at(Vec3::ZERO, Vec3::Y);
        let down = sun.radiance_at(Vec3::ZERO, Vec3::new(0.0, -1.0, 0.0));
        assert!(up[0] > 0.0 && up[1] > 0.0);
        assert_eq!(down, [0.0; 3], "a downward normal faces away from the sun");
    }

    #[test]
    fn smooth_range_falloff_reaches_zero() {
        assert_eq!(attenuation(Falloff::SmoothRange, 10.0, 10.0), 0.0);
        assert_eq!(attenuation(Falloff::SmoothRange, 20.0, 10.0), 0.0);
        assert!((attenuation(Falloff::SmoothRange, 0.0, 10.0) - 1.0).abs() < 1e-6);
        let mid = attenuation(Falloff::SmoothRange, 5.0, 10.0);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn smooth_range_is_monotonic() {
        let mut previous = 1.0;
        for i in 0..=100 {
            let a = attenuation(Falloff::SmoothRange, i as f32 * 0.1, 10.0);
            assert!(a <= previous + 1e-6, "must not increase with distance");
            previous = a;
        }
    }

    #[test]
    fn inverse_square_falloff() {
        assert!((attenuation(Falloff::InverseSquare, 0.0, 0.0) - 1.0).abs() < 1e-6);
        assert!((attenuation(Falloff::InverseSquare, 1.0, 0.0) - 0.5).abs() < 1e-6);
        // Range still cuts it off hard when set.
        assert_eq!(attenuation(Falloff::InverseSquare, 5.0, 4.0), 0.0);
    }

    #[test]
    fn point_light_falls_off_with_distance() {
        // The light sits above the origin; both samples face it.
        let l = Light::point(Vec3::new(0.0, 10.0, 0.0), Color::WHITE, 1.0, 30.0);
        let near = l.radiance_at(Vec3::new(0.0, 9.0, 0.0), Vec3::Y);
        let far = l.radiance_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
        assert!(near[0] > 0.0, "the near sample must be lit: {near:?}");
        assert!(near[0] > far[0], "{near:?} vs {far:?}");
        assert!(near[0] <= 1.0 + 1e-5);
    }

    #[test]
    fn point_light_behind_the_normal_contributes_nothing() {
        let l = Light::point(Vec3::new(0.0, 10.0, 0.0), Color::WHITE, 1.0, 0.0);
        assert_eq!(
            l.radiance_at(Vec3::ZERO, Vec3::new(0.0, -1.0, 0.0)),
            [0.0; 3]
        );
    }

    #[test]
    fn ambient_hemisphere_interpolates_smoothly() {
        let a = Ambient {
            sky: Color::rgb(0.0, 0.0, 1.0),
            ground: Color::rgb(1.0, 1.0, 0.0),
            hemisphere: 1.0,
            intensity: 1.0,
        };
        let up = a.radiance_at(Vec3::Y);
        let side = a.radiance_at(Vec3::X);
        let down = a.radiance_at(Vec3::new(0.0, -1.0, 0.0));
        assert!(
            (up[2] - 1.0).abs() < 1e-5 && up[0] < 1e-5,
            "up must be sky: {up:?}"
        );
        assert!(
            (down[0] - 1.0).abs() < 1e-5 && down[2] < 1e-5,
            "down must be ground: {down:?}"
        );
        assert!(
            side[0] > up[0] && side[0] < down[0],
            "side is halfway: {side:?}"
        );
    }

    #[test]
    fn spot_cone_shape() {
        let l = Light::torch(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Color::WHITE,
            1.0,
            20.0,
        );
        // Straight below the torch: inside the inner cone.
        let inside = l.radiance_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
        assert!(inside[0] > 0.0, "{inside:?}");
        // Far off to the side and well below: outside the outer cone.
        let outside = l.radiance_at(Vec3::new(12.0, 0.0, 0.0), Vec3::Y);
        assert_eq!(outside, [0.0; 3], "{outside:?}");
    }

    #[test]
    fn spot_cone_ramps_smoothly() {
        let l = Light::torch(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Color::WHITE,
            1.0,
            50.0,
        );
        let mut previous = 1.0;
        for i in 0..40 {
            let x = i as f32 * 0.25;
            let v = l.radiance_at(Vec3::new(x, 0.0, 0.0), Vec3::Y)[0];
            assert!(
                v <= previous + 1e-4,
                "cone must not get brighter outwards at x={x}"
            );
            previous = v;
        }
    }

    #[test]
    fn radiant_color_scales_with_intensity() {
        let l = Light::point(Vec3::ZERO, Color::rgb(0.5, 0.5, 0.5), 4.0, 10.0);
        let c = l.radiant_color();
        assert!((c.r - 2.0).abs() < 1e-5);
    }

    #[test]
    fn affects_bounds_culls_distant_lights() {
        let l = Light::point(Vec3::ZERO, Color::WHITE, 1.0, 10.0);
        assert!(l.affects_bounds(Vec3::new(5.0, 0.0, 0.0), 1.0));
        assert!(!l.affects_bounds(Vec3::new(100.0, 0.0, 0.0), 1.0));
        // A directional light affects everything.
        assert!(Light::sun().affects_bounds(Vec3::new(1e6, 0.0, 0.0), 0.1));
    }

    #[test]
    fn ambient_hemisphere_blend() {
        let a = Ambient::default();
        let up = a.radiance_at(Vec3::Y);
        let down = a.radiance_at(Vec3::new(0.0, -1.0, 0.0));
        assert!(up[2] > down[2], "sky is bluer than ground");
        assert!(down[0] >= 0.0);
    }

    #[test]
    fn ambient_intensity_scales() {
        let a = Ambient {
            intensity: 2.0,
            ..Ambient::default()
        };
        let base = Ambient::default().radiance_at(Vec3::Y);
        let scaled = a.radiance_at(Vec3::Y);
        assert!((scaled[0] - base[0] * 2.0).abs() < 1e-5);
    }

    #[test]
    fn night_is_darker_than_day() {
        assert!(Ambient::night().flat()[0] < Ambient::default().flat()[0]);
    }

    #[test]
    fn fog_factors() {
        let fog = Fog {
            color: Color::BLACK,
            start: 10.0,
            end: 20.0,
            height_falloff: 0.0,
        };
        let cam = Vec3::ZERO;
        assert_eq!(fog.factor_at(Vec3::new(5.0, 0.0, 0.0), cam), 0.0);
        assert!((fog.factor_at(Vec3::new(15.0, 0.0, 0.0), cam) - 0.5).abs() < 1e-5);
        assert_eq!(fog.factor_at(Vec3::new(100.0, 0.0, 0.0), cam), 1.0);
    }

    #[test]
    fn fog_apply_blends_towards_the_colour() {
        let fog = Fog {
            color: Color::rgb(1.0, 1.0, 1.0),
            start: 0.0,
            end: 1.0,
            height_falloff: 0.0,
        };
        assert_eq!(fog.apply([0.0, 0.0, 0.0], 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(fog.apply([0.0, 0.0, 0.0], 1.0), [1.0, 1.0, 1.0]);
        assert_eq!(fog.apply([0.0, 0.0, 0.0], 0.5), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn height_fog_thins_with_altitude() {
        let fog = Fog {
            color: Color::BLACK,
            start: 0.0,
            end: 10.0,
            height_falloff: 1.0,
        };
        let low = fog.factor_at(Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO);
        let high = fog.factor_at(Vec3::new(10.0, 30.0, 0.0), Vec3::ZERO);
        assert!(high < low);
    }
}
