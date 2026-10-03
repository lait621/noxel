//! Per-step simulation events.

use noxel_core::math::Vec3;

use crate::body::BodyHandle;

/// Something the simulation observed during a step.
///
/// Events accumulate in [`crate::PhysicsWorld::events`] until
/// [`crate::PhysicsWorld::clear_events`] is called, which gameplay code should
/// do at the end of the frame that consumed them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PhysicsEvent {
    /// Two solid bodies started touching.
    CollisionEnter {
        /// The body with the lower handle; `normal` points from `a` to `b`.
        a: BodyHandle,
        /// The body with the higher handle.
        b: BodyHandle,
        /// World-space contact point.
        point: Vec3,
        /// Unit contact normal, pointing from `a` towards `b`.
        normal: Vec3,
        /// Accumulated normal impulse of the step that produced the contact,
        /// in kg·m/s. Useful as an impact-strength signal for audio and damage.
        impulse: f32,
    },
    /// Two solid bodies stopped touching.
    CollisionExit {
        /// The body with the lower handle.
        a: BodyHandle,
        /// The body with the higher handle.
        b: BodyHandle,
    },
    /// A sensor began overlapping another body.
    TriggerEnter {
        /// The sensor body.
        trigger: BodyHandle,
        /// The body inside the sensor.
        other: BodyHandle,
    },
    /// A sensor stopped overlapping another body.
    TriggerExit {
        /// The sensor body.
        trigger: BodyHandle,
        /// The body that left the sensor.
        other: BodyHandle,
    },
    /// A dynamic body dropped below the sleep thresholds long enough to sleep.
    BodySlept {
        /// The body that slept.
        body: BodyHandle,
    },
    /// The world woke a sleeping body: a neighbour reached it, or gameplay
    /// code moved it through a world setter. (`Body::apply_impulse` wakes the
    /// body directly, without a world to report through.)
    BodyWoke {
        /// The body that woke.
        body: BodyHandle,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::pool::Handle;

    #[test]
    fn events_compare_by_value() {
        let a: BodyHandle = Handle::from_bits(1);
        let b: BodyHandle = Handle::from_bits(2);
        let e = PhysicsEvent::CollisionEnter {
            a,
            b,
            point: Vec3::ZERO,
            normal: Vec3::Y,
            impulse: 1.0,
        };
        let f = e;
        assert_eq!(e, f);
        assert_ne!(e, PhysicsEvent::CollisionExit { a, b });
        assert_ne!(e, PhysicsEvent::TriggerEnter { trigger: a, other: b });
    }
}
