//! Trauma-based camera shake.
//!
//! Shake is driven by a single scalar "trauma" in `[0, 1]` that decays linearly;
//! the actual offset is `trauma²` scaled by a per-axis smooth noise. Squaring is
//! what makes a shake *start* sharply and *end* gently, which is how explosions
//! and impacts read; a linear falloff feels like the camera is being pushed.
//!
//! The noise is hash-based and indexed by a step counter rather than by wall
//! time, so a shake is **deterministic**: recording the trauma events is enough
//! to replay the exact same camera motion, which is what makes a golden-image
//! test of a scripted explosion possible.

use noxel_core::math::{Vec3, lerp};

use crate::shake_noise;

/// Decaying camera shake.
#[derive(Clone, Debug)]
pub struct CameraShake {
    trauma: f32,
    seed: u64,
    /// Continuous time in seconds, advanced by [`CameraShake::update`].
    time: f32,
    /// How fast the noise is sampled, in steps per second.
    frequency: f32,
    /// World-space offset at trauma 1.
    max_offset: f32,
    /// Camera roll at trauma 1, radians.
    max_roll: f32,
    /// Trauma lost per second.
    decay: f32,
    /// The offset from the most recent [`CameraShake::offset`] call.
    last_offset: Vec3,
}

impl CameraShake {
    /// Creates an inactive shake with the default feel.
    #[must_use]
    pub fn new() -> Self {
        Self {
            trauma: 0.0,
            seed: 0x5EED_5EED,
            time: 0.0,
            // 18 Hz: fast enough to read as impact, slow enough not to alias.
            frequency: 18.0,
            max_offset: 0.55,
            max_roll: 0.035,
            // A full-strength shake is over in about 0.8 s.
            decay: 1.25,
            last_offset: Vec3::ZERO,
        }
    }

    /// A strong, short shake for an explosion.
    #[must_use]
    pub fn explosive() -> Self {
        Self {
            max_offset: 1.1,
            max_roll: 0.09,
            frequency: 24.0,
            decay: 1.6,
            ..Self::new()
        }
    }

    /// A subtle shake for footsteps and light impacts.
    #[must_use]
    pub fn rumble() -> Self {
        Self {
            max_offset: 0.18,
            max_roll: 0.008,
            frequency: 9.0,
            decay: 0.9,
            ..Self::new()
        }
    }

    /// Adds trauma, saturating at 1.
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount.max(0.0)).min(1.0);
    }

    /// The current trauma.
    #[inline]
    #[must_use]
    pub fn trauma(&self) -> f32 {
        self.trauma
    }

    /// True when the shake is still moving the camera.
    #[inline]
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.trauma > 1e-4
    }

    /// The shake's intensity, `trauma²`.
    ///
    /// This is the value the offset is scaled by; it is exposed so a game can
    /// drive a matching sound or a screen-edge vignette.
    #[inline]
    #[must_use]
    pub fn intensity(&self) -> f32 {
        self.trauma * self.trauma
    }

    /// Stops the shake immediately.
    pub fn stop(&mut self) {
        self.trauma = 0.0;
        self.last_offset = Vec3::ZERO;
    }

    /// Sets the deterministic seed, so a replay reproduces the same motion.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// Sets the noise frequency in steps per second.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.frequency = frequency.clamp(0.5, 240.0);
    }

    /// Sets the maximum offset and roll.
    pub fn set_magnitude(&mut self, max_offset: f32, max_roll: f32) {
        self.max_offset = max_offset.max(0.0);
        self.max_roll = max_roll.max(0.0);
    }

    /// Sets how much trauma is lost per second.
    pub fn set_decay(&mut self, decay: f32) {
        self.decay = decay.max(0.0);
    }

    /// Advances the decay and the noise clock.
    pub fn update(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.time += dt;
        self.trauma = (self.trauma - self.decay * dt).max(0.0);
        if self.trauma <= 1e-4 {
            self.trauma = 0.0;
            self.last_offset = Vec3::ZERO;
        }
    }

    /// The eye offset for this instant, in world units.
    ///
    /// `focus` is unused today but is part of the signature so a future version
    /// can scale the shake with distance without changing every call site.
    pub fn offset(&mut self, _focus: Vec3) -> Vec3 {
        if !self.is_active() {
            self.last_offset = Vec3::ZERO;
            return Vec3::ZERO;
        }
        let intensity = self.intensity();
        let step = (self.time * self.frequency).max(0.0);
        self.last_offset = Vec3::new(
            noise1d(step, 0, self.seed),
            noise1d(step, 1, self.seed),
            noise1d(step, 2, self.seed),
        ) * (self.max_offset * intensity);
        self.last_offset
    }

    /// The camera roll for this instant, in radians.
    ///
    /// A little roll is what separates a convincing shake from a translation
    /// that just looks like the camera is unsteady.
    #[must_use]
    pub fn roll(&self) -> f32 {
        if !self.is_active() {
            return 0.0;
        }
        let step = (self.time * self.frequency).max(0.0);
        noise1d(step, 3, self.seed) * self.max_roll * self.intensity()
    }

    /// The offset from the most recent [`CameraShake::offset`] call, without
    /// advancing anything.
    #[inline]
    #[must_use]
    pub fn last_offset(&self) -> Vec3 {
        self.last_offset
    }
}

impl Default for CameraShake {
    fn default() -> Self {
        Self::new()
    }
}

/// Smooth 1D noise: a lerp between two adjacent hash values.
///
/// Interpolating is what makes the shake read as a rumble rather than as
/// per-frame static.
fn noise1d(step: f32, axis: u32, seed: u64) -> f32 {
    let base = step.floor();
    let t = step - base;
    let a = shake_noise(base as u32, axis, seed);
    let b = shake_noise(base as u32 + 1, axis, seed);
    // Smoothstep, so the derivative is continuous and the motion has no corners.
    let s = t * t * (3.0 - 2.0 * t);
    lerp(a, b, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_shake_is_inactive() {
        let s = CameraShake::new();
        assert!(!s.is_active());
        assert_eq!(s.trauma(), 0.0);
        assert_eq!(s.intensity(), 0.0);
    }

    #[test]
    fn trauma_saturates_at_one() {
        let mut s = CameraShake::new();
        s.add_trauma(0.6);
        s.add_trauma(0.6);
        assert_eq!(s.trauma(), 1.0);
    }

    #[test]
    fn negative_trauma_is_ignored() {
        let mut s = CameraShake::new();
        s.add_trauma(-5.0);
        assert_eq!(s.trauma(), 0.0);
    }

    #[test]
    fn intensity_is_squared() {
        let mut s = CameraShake::new();
        s.add_trauma(0.5);
        assert!((s.intensity() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn trauma_decays_to_zero() {
        let mut s = CameraShake::new();
        s.add_trauma(1.0);
        for _ in 0..200 {
            s.update(1.0 / 60.0);
        }
        assert_eq!(s.trauma(), 0.0);
        assert!(!s.is_active());
    }

    #[test]
    fn decay_is_frame_rate_independent() {
        let mut a = CameraShake::new();
        let mut b = CameraShake::new();
        a.add_trauma(1.0);
        b.add_trauma(1.0);
        for _ in 0..30 {
            a.update(1.0 / 30.0);
        }
        for _ in 0..60 {
            b.update(1.0 / 60.0);
        }
        assert!(
            (a.trauma() - b.trauma()).abs() < 1e-5,
            "{} vs {}",
            a.trauma(),
            b.trauma()
        );
    }

    #[test]
    fn offset_is_zero_when_inactive() {
        let mut s = CameraShake::new();
        assert_eq!(s.offset(Vec3::ZERO), Vec3::ZERO);
        assert_eq!(s.roll(), 0.0);
    }

    #[test]
    fn offset_is_bounded_by_the_magnitude() {
        let mut s = CameraShake::new();
        s.set_magnitude(0.5, 0.05);
        s.add_trauma(1.0);
        for i in 0..500 {
            s.update(1.0 / 60.0);
            let o = s.offset(Vec3::ZERO);
            assert!(o.length() <= 0.5 * 3.0f32.sqrt() + 1e-4, "step {i}: {o:?}");
            assert!(s.roll().abs() <= 0.05 + 1e-6);
        }
    }

    #[test]
    fn offset_moves_over_time() {
        let mut s = CameraShake::new();
        s.add_trauma(1.0);
        let a = s.offset(Vec3::ZERO);
        for _ in 0..10 {
            s.update(1.0 / 60.0);
        }
        let b = s.offset(Vec3::ZERO);
        assert!(
            (a - b).length() > 1e-4,
            "the shake must actually move: {a:?} {b:?}"
        );
    }

    #[test]
    fn shake_is_deterministic_for_a_seed() {
        let mut a = CameraShake::new();
        let mut b = CameraShake::new();
        a.set_seed(99);
        b.set_seed(99);
        a.add_trauma(1.0);
        b.add_trauma(1.0);
        for _ in 0..50 {
            a.update(1.0 / 60.0);
            b.update(1.0 / 60.0);
            assert_eq!(a.offset(Vec3::ZERO), b.offset(Vec3::ZERO));
        }
    }

    #[test]
    fn different_seeds_give_different_motion() {
        let mut a = CameraShake::new();
        let mut b = CameraShake::new();
        a.set_seed(1);
        b.set_seed(2);
        a.add_trauma(1.0);
        b.add_trauma(1.0);
        let mut differs = false;
        for _ in 0..50 {
            a.update(1.0 / 60.0);
            b.update(1.0 / 60.0);
            if (a.offset(Vec3::ZERO) - b.offset(Vec3::ZERO)).length() > 1e-4 {
                differs = true;
            }
        }
        assert!(differs);
    }

    #[test]
    fn stop_clears_everything() {
        let mut s = CameraShake::new();
        s.add_trauma(1.0);
        s.update(1.0 / 60.0);
        s.offset(Vec3::ZERO);
        s.stop();
        assert_eq!(s.trauma(), 0.0);
        assert_eq!(s.last_offset(), Vec3::ZERO);
    }

    #[test]
    fn presets_differ_in_magnitude_and_speed() {
        let e = CameraShake::explosive();
        let r = CameraShake::rumble();
        assert!(e.max_offset > r.max_offset);
        assert!(e.frequency > r.frequency);
        assert!(r.decay < e.decay);
    }

    #[test]
    fn invalid_dt_is_ignored() {
        let mut s = CameraShake::new();
        s.add_trauma(0.5);
        s.update(f32::NAN);
        s.update(-1.0);
        s.update(0.0);
        assert_eq!(s.trauma(), 0.5);
    }

    #[test]
    fn frequency_and_decay_are_clamped() {
        let mut s = CameraShake::new();
        s.set_frequency(0.0);
        s.set_decay(-1.0);
        s.update(1.0); // must not panic or produce NaN
        assert!(s.trauma().is_finite());
    }

    #[test]
    fn magnitude_is_clamped_to_non_negative() {
        let mut s = CameraShake::new();
        s.set_magnitude(-1.0, -1.0);
        s.add_trauma(1.0);
        s.update(0.1);
        assert_eq!(s.offset(Vec3::ZERO), Vec3::ZERO);
    }

    #[test]
    fn stronger_shake_moves_further() {
        let mut weak = CameraShake::new();
        let mut strong = CameraShake::new();
        weak.add_trauma(0.3);
        strong.add_trauma(1.0);
        let mut weak_max = 0.0f32;
        let mut strong_max = 0.0f32;
        for _ in 0..60 {
            weak.update(1.0 / 60.0);
            strong.update(1.0 / 60.0);
            weak_max = weak_max.max(weak.offset(Vec3::ZERO).length());
            strong_max = strong_max.max(strong.offset(Vec3::ZERO).length());
        }
        assert!(strong_max > weak_max, "{strong_max} vs {weak_max}");
    }
}
