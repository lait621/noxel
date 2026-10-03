//! World-level simulation settings.

use noxel_core::math::Vec3;

/// Default broadphase cell size in metres.
///
/// The spatial hash wants a cell near the typical body size; a top-down RPG's
/// crates, NPCs and wall segments are all between 0.5 m and 4 m.
pub const DEFAULT_CELL_SIZE: f32 = 4.0;

/// Tuning for the fixed-step simulation.
///
/// All distances are metres, all times seconds, matching
/// [`noxel_core::METER`].
#[derive(Clone, Copy, Debug)]
pub struct PhysicsConfig {
    /// Acceleration applied to dynamic bodies, in m/s².
    pub gravity: Vec3,
    /// Broadphase hash cell edge length in metres.
    pub broadphase_cell_size: f32,
    /// Gap below which two shapes are considered touching, in metres.
    ///
    /// Contact generation keeps a small positive margin so resting bodies do
    /// not flicker between touching and separated.
    pub contact_tolerance: f32,
    /// Sequential-impulse iterations per step.
    pub solver_iterations: u32,
    /// Baumgarte position-correction factor, usually `0.1..=0.4`.
    pub position_correction: f32,
    /// Largest `dt` the simulation will accept, in seconds.
    pub max_step_delta: f32,
    /// Linear speed below which a body may fall asleep, in m/s.
    pub sleep_linear_threshold: f32,
    /// Angular speed below which a body may fall asleep, in rad/s.
    pub sleep_angular_threshold: f32,
    /// Seconds below both thresholds before a body sleeps.
    pub sleep_time_required: f32,
    /// Hard cap on live bodies; [`crate::PhysicsWorld::insert`] refuses more.
    pub max_bodies: usize,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            // A top-down RPG is read at a glance and falls are short, so
            // gravity is deliberately stronger than Earth's: props settle
            // ~2x faster, which reads better at 60 Hz.
            gravity: Vec3::new(0.0, -9.81 * 2.0, 0.0),
            broadphase_cell_size: DEFAULT_CELL_SIZE,
            contact_tolerance: 0.005,
            solver_iterations: 4,
            position_correction: 0.2,
            max_step_delta: 0.05,
            sleep_linear_threshold: 0.05,
            sleep_angular_threshold: 0.05,
            sleep_time_required: 0.5,
            max_bodies: 4096,
        }
    }
}

impl PhysicsConfig {
    /// Returns a copy with every field clamped into a range the solver can
    /// trust: durations and sizes are positive and finite.
    #[must_use]
    pub(crate) fn sanitized(&self) -> Self {
        let mut c = *self;
        if !c.gravity.is_finite() {
            c.gravity = Vec3::ZERO;
        }
        c.broadphase_cell_size = clamp_range(c.broadphase_cell_size, 0.05, 1024.0, DEFAULT_CELL_SIZE);
        c.contact_tolerance = clamp_range(c.contact_tolerance, 0.0, 0.5, 0.005);
        c.solver_iterations = c.solver_iterations.clamp(1, 64);
        c.position_correction = clamp_range(c.position_correction, 0.0, 1.0, 0.2);
        c.max_step_delta = clamp_range(c.max_step_delta, 1e-4, 1.0, 0.05);
        c.sleep_linear_threshold = clamp_range(c.sleep_linear_threshold, 0.0, 1e3, 0.05);
        c.sleep_angular_threshold = clamp_range(c.sleep_angular_threshold, 0.0, 1e3, 0.05);
        c.sleep_time_required = clamp_range(c.sleep_time_required, 0.0, 1e3, 0.5);
        c.max_bodies = c.max_bodies.max(1);
        c
    }
}

fn clamp_range(v: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(min, max) } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = PhysicsConfig::default();
        assert!(c.gravity.y < 0.0);
        assert!(c.gravity.y <= -9.81, "gravity must at least match Earth");
        assert!(c.solver_iterations >= 1);
        assert!(c.position_correction > 0.0 && c.position_correction < 1.0);
        assert!(c.max_step_delta > 0.0);
        assert!(c.contact_tolerance > 0.0);
        assert!(c.sleep_time_required > 0.0);
        assert!(c.max_bodies > 0);
    }

    #[test]
    fn sanitize_rejects_nonsense() {
        let c = PhysicsConfig {
            gravity: Vec3::new(f32::NAN, 0.0, 0.0),
            broadphase_cell_size: -4.0,
            contact_tolerance: f32::INFINITY,
            solver_iterations: 0,
            position_correction: 5.0,
            max_step_delta: 0.0,
            sleep_linear_threshold: -1.0,
            sleep_angular_threshold: f32::NAN,
            sleep_time_required: -3.0,
            max_bodies: 0,
        }
        .sanitized();
        assert_eq!(c.gravity, Vec3::ZERO);
        assert!(c.broadphase_cell_size > 0.0);
        assert!(c.contact_tolerance >= 0.0);
        assert!(c.solver_iterations >= 1);
        assert!(c.position_correction <= 1.0);
        assert!(c.max_step_delta > 0.0);
        assert!(c.sleep_linear_threshold >= 0.0);
        assert!(c.sleep_angular_threshold >= 0.0);
        assert!(c.sleep_time_required >= 0.0);
        assert!(c.max_bodies >= 1);
    }
}
