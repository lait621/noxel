//! The kinematic character controller.
//!
//! `move_character` is the API a player feels every frame, so it is built from
//! the same shape casts the rest of the engine uses and is deliberately
//! conservative: it never tunnels, never leaves the ground while walking down a
//! slope, and never climbs anything steeper than [`MAX_SLOPE_ANGLE`].
//!
//! # The move, in order
//!
//! 1. **Up** — the upward part of the motion is swept first, so a jump cannot
//!    push the character through a ceiling.
//! 2. **Across** — the horizontal part is swept and slid along up to
//!    [`MAX_SLIDES`] surfaces. If a slide is blocked by something walkable-sized
//!    the controller retries the whole move from [`STEP_HEIGHT`] higher, then
//!    drops back down; the stepped result only wins when it makes strictly more
//!    progress.
//! 3. **Down** — the downward part plus a ground-snap probe, which keeps the
//!    character glued to a floor while walking over small bumps and off ledges.
//!
//! # Carrying
//!
//! A character that was standing on a body moving with a velocity inherits that
//! body's displacement for the step (`velocity * last_step_delta`), which is
//! what makes a kinematic platform carry the player.

use noxel_core::math::{Quat, Ray, Vec3, to_radians};

use crate::body::{BodyHandle, LAYER_ALL};
use crate::query::QueryFilter;
use crate::shape::ColliderShape;
use crate::world::PhysicsWorld;

/// Steepest surface a character can stand on, in radians (46°).
///
/// Below this the move slides *up* the surface; above it the surface acts as a
/// wall and the character slides along its horizontal edge only.
pub const MAX_SLOPE_ANGLE: f32 = to_radians(46.0);

/// How high a lip the character can walk up, in metres.
pub const STEP_HEIGHT: f32 = 0.4;

/// How far down the controller looks for ground when it was already grounded,
/// in metres. Keeps the character attached while walking down stairs.
pub const GROUND_SNAP_DISTANCE: f32 = 0.2;

/// Maximum number of surface slides in one horizontal move.
const MAX_SLIDES: u32 = 4;

/// A stepped move must beat the direct move by this much to be accepted.
const STEP_PROGRESS_EPSILON: f32 = 1e-3;

/// What [`PhysicsWorld::move_character`] did.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct CharacterMove {
    /// The translation actually applied, in metres.
    pub translation: Vec3,
    /// `translation / dt`, using the last step's `dt`; zero before the first
    /// step.
    pub velocity: Vec3,
    /// True when the character ended the move standing on walkable ground.
    pub grounded: bool,
    /// Normal of the surface being stood on (equal to `up` when airborne).
    pub ground_normal: Vec3,
    /// The body being stood on, when it is a body rather than the void.
    pub ground_body: Option<BodyHandle>,
    /// True when the upward part of the move hit something.
    pub hit_ceiling: bool,
    /// True when the horizontal part of the move was stopped by a wall or a
    /// too-steep slope.
    pub hit_wall: bool,
    /// Number of blocking contacts encountered.
    pub hit_count: u32,
}

/// Everything a sweep needs to know about the character being moved.
#[derive(Clone, Copy, Debug)]
struct Mover {
    shape: ColliderShape,
    rotation: Quat,
    up: Vec3,
    filter: QueryFilter,
}

/// Result of the horizontal sliding phase.
#[derive(Clone, Copy, Debug)]
struct SlideOutcome {
    position: Vec3,
    hits: u32,
    blocked: bool,
}

/// Result of a successful step-up attempt.
struct StepOutcome {
    outcome: SlideOutcome,
    ground: BodyHandle,
    normal: Vec3,
}

impl PhysicsWorld {
    /// Sweeps a character's collider through the world and moves the body.
    ///
    /// `desired_motion` is a displacement in metres (velocity × frame time),
    /// and `up` is the character's up axis (usually [`Vec3::Y`]). The body's own
    /// collider, its `mask` and the world geometry all participate; the body
    /// itself is always ignored.
    ///
    /// Intended for [`crate::BodyKind::Kinematic`] bodies: dynamic bodies are
    /// also moved, but the solver will then fight the controller for them.
    pub fn move_character(
        &mut self,
        handle: BodyHandle,
        desired_motion: Vec3,
        up: Vec3,
    ) -> CharacterMove {
        let Some(body) = self.body(handle) else {
            return CharacterMove::default();
        };
        let shape = body.shape;
        let rotation = body.rotation;
        let start = body.position;
        let filter = QueryFilter {
            mask: if body.mask == 0 { LAYER_ALL } else { body.mask },
            ignore: Some(handle),
            include_sensors: false,
            include_static: true,
        };

        let mover = Mover {
            shape,
            rotation,
            up: up.try_normalize().unwrap_or(Vec3::Y),
            filter,
        };
        let up = mover.up;
        let motion = if desired_motion.is_finite() {
            desired_motion
        } else {
            Vec3::ZERO
        };

        // Carry: a character standing on a moving platform inherits the
        // platform's displacement for the step.
        let ground_before = self.character_ground(handle);
        let carry = match ground_before {
            Some(ground) if motion.dot(up) <= 0.0 => self
                .body(ground)
                .map(|body| body.linear_velocity * self.last_step_delta())
                .filter(|delta| delta.is_finite() && *delta != Vec3::ZERO)
                .unwrap_or(Vec3::ZERO),
            _ => Vec3::ZERO,
        };
        let was_grounded = ground_before.is_some();
        let desired = motion + carry;

        let mut position = start;
        let mut hit_count = 0u32;
        let mut hit_ceiling = false;
        let mut hit_wall = false;
        let mut grounded = false;
        let mut ground_normal = up;
        let mut ground_body: Option<BodyHandle> = None;

        // --- 1. up ------------------------------------------------------
        let vertical = desired.dot(up);
        if vertical > 0.0 {
            match self.sweep_shape(&shape, position, rotation, up, vertical, &filter) {
                Some(hit) => {
                    hit_ceiling = true;
                    hit_count += 1;
                    position += up * hit.distance;
                }
                None => position += up * vertical,
            }
        }

        // --- 2. across --------------------------------------------------
        let horizontal = desired - up * vertical;
        if horizontal.length_squared() > 1e-12 {
            let direct = self.slide_move(&mover, position, horizontal);
            let mut chosen = direct;
            let mut stepped_ground = None;
            if direct.blocked && (was_grounded || vertical <= 0.0) {
                if let Some(step) = self.try_step_up(&mover, position, horizontal, &direct) {
                    chosen = step.outcome;
                    stepped_ground = Some((step.ground, step.normal));
                }
            }
            position = chosen.position;
            hit_count += chosen.hits;
            hit_wall |= chosen.blocked;
            if let Some((ground, normal)) = stepped_ground {
                // The step counts as ground only on a walkable surface; a
                // capsule balanced on the corner of a step is mid-climb, and
                // the downward phase below decides whether it is grounded.
                ground_normal = normal;
                if is_walkable(normal, up) {
                    grounded = true;
                    ground_body = Some(ground);
                }
            }
        }

        // --- 3. down ----------------------------------------------------
        let down = (-vertical).max(0.0);
        // A character that is not moving upwards probes for ground: that keeps
        // it glued to floors, stairs and slopes, and lets a freshly spawned
        // character report `grounded` without a separate settling frame. A
        // jumping character is never pulled back down.
        let snap = if vertical <= 0.0 {
            GROUND_SNAP_DISTANCE
        } else {
            0.0
        };
        let cast = down + snap;
        if cast > 0.0 {
            match self.sweep_shape(&shape, position, rotation, -up, cast, &filter) {
                Some(hit) => {
                    hit_count += 1;
                    position -= up * hit.distance;
                    ground_normal = hit.normal;
                    if is_walkable(hit.normal, up) {
                        grounded = true;
                        ground_body = Some(hit.body);
                    } else {
                        // A wall or a too-steep slope: the character stops
                        // against it rather than standing on it.
                        grounded = false;
                        ground_body = None;
                    }
                }
                None => {
                    if down > 0.0 {
                        position -= up * down;
                    }
                    grounded = false;
                    ground_body = None;
                }
            }
        }

        if !position.is_finite() {
            position = start;
        }
        if position != start {
            self.set_position(handle, position);
        }
        self.set_character_ground(handle, ground_body);
        let translation = position - start;
        let dt = self.last_step_delta();
        let velocity = if dt > 0.0 {
            translation / dt
        } else {
            Vec3::ZERO
        };
        CharacterMove {
            translation,
            velocity,
            grounded,
            ground_normal,
            ground_body,
            hit_ceiling,
            hit_wall,
            hit_count,
        }
    }

    /// True when a short downward probe from `position` finds walkable ground.
    ///
    /// This is the "is the character supported?" question asked with a ray
    /// instead of the collider, so a capsule balanced on the corner of a step
    /// still counts as supported while a ramp does not.
    fn probe_walkable_ground(&self, mover: &Mover, position: Vec3) -> bool {
        let Mover {
            shape, up, filter, ..
        } = *mover;
        let reach = shape.bounding_radius() + STEP_HEIGHT + 0.1;
        let ray = Ray {
            origin: position,
            dir: -up,
            max_t: reach,
        };
        match self.raycast(&ray, filter) {
            Some(hit) => is_walkable(hit.normal, up),
            None => false,
        }
    }

    /// Sweeps `motion` and slides along whatever it hits.
    fn slide_move(&self, mover: &Mover, start: Vec3, motion: Vec3) -> SlideOutcome {
        let Mover {
            shape,
            rotation,
            up,
            filter,
        } = *mover;
        let mut position = start;
        let mut remaining = motion;
        let mut hits = 0u32;
        let mut blocked = false;
        for _ in 0..MAX_SLIDES {
            let length = remaining.length();
            if length <= 1e-6 {
                break;
            }
            let dir = remaining / length;
            let Some(hit) = self.sweep_shape(&shape, position, rotation, dir, length, &filter)
            else {
                // Path is clear for the rest of the motion.
                position += remaining;
                break;
            };
            position += dir * hit.distance;
            hits += 1;
            let walkable = is_walkable(hit.normal, up);
            blocked |= !walkable;
            // A walkable surface slides along its true normal, which lifts the
            // character up a ramp. A too-steep surface only cancels the
            // horizontal component, so it cannot be climbed.
            let plane_normal = if walkable {
                hit.normal
            } else {
                let flat = hit.normal - up * hit.normal.dot(up);
                if flat.length_squared() > 1e-8 {
                    flat.normalize_or_zero()
                } else {
                    hit.normal
                }
            };
            remaining -= plane_normal * remaining.dot(plane_normal);
        }
        SlideOutcome {
            position,
            hits,
            blocked,
        }
    }

    /// Retries a blocked horizontal move from one step higher.
    fn try_step_up(
        &self,
        mover: &Mover,
        position: Vec3,
        motion: Vec3,
        direct: &SlideOutcome,
    ) -> Option<StepOutcome> {
        let Mover {
            shape,
            rotation,
            up,
            filter,
        } = *mover;
        // Head-room: refuse to step if the ceiling is closer than the step.
        if self
            .sweep_shape(&shape, position, rotation, up, STEP_HEIGHT, &filter)
            .is_some()
        {
            return None;
        }
        let raised = position + up * STEP_HEIGHT;
        let stepped = self.slide_move(mover, raised, motion);
        // Drop back down. If there is no ground within one step the raised move
        // was a ledge, not a step, so the direct move stands.
        let landed = self.sweep_shape(
            &shape,
            stepped.position,
            rotation,
            -up,
            STEP_HEIGHT,
            &filter,
        )?;
        let landing = stepped.position - up * landed.distance;
        // The drop usually lands on the step's *top face*, which is walkable.
        // When the frame's motion is small the capsule instead catches the
        // step's leading *edge*, whose contact normal is as steep as a ramp —
        // so also accept the step when a downward probe from the character's
        // centre finds walkable ground below it. A ramp fails that probe (the
        // ray hits the ramp itself), which is what keeps the slope limit
        // meaningful.
        if !is_walkable(landed.normal, up) && !self.probe_walkable_ground(mover, landing) {
            return None;
        }
        let dir = motion.normalize_or_zero();
        let direct_progress = (direct.position - position).dot(dir);
        let stepped_progress = (landing - position).dot(dir);
        if stepped_progress <= direct_progress + STEP_PROGRESS_EPSILON {
            return None;
        }
        Some(StepOutcome {
            outcome: SlideOutcome {
                position: landing,
                hits: stepped.hits + 1,
                blocked: stepped.blocked,
            },
            ground: landed.body,
            normal: landed.normal,
        })
    }
}

/// True when a surface with this normal can be stood on.
#[must_use]
pub(crate) fn is_walkable(normal: Vec3, up: Vec3) -> bool {
    normal.dot(up) >= MAX_SLOPE_ANGLE.cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::BodyDesc;
    use crate::config::PhysicsConfig;
    use noxel_core::math::Aabb;

    /// A capsule character roughly 1.6 m tall: radius 0.3, half height 0.5, so
    /// its feet are 0.8 below its centre.
    fn spawn_character(world: &mut PhysicsWorld, feet_y: f32) -> BodyHandle {
        world.insert(
            BodyDesc::kinematic(ColliderShape::Capsule {
                radius: 0.3,
                half_height: 0.5,
            })
            .at(Vec3::new(0.0, feet_y + 0.8, 0.0))
            .with_layer(crate::body::LAYER_PLAYER, LAYER_ALL),
        )
    }

    fn ground(world: &mut PhysicsWorld) {
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-50.0, -1.0, -50.0), Vec3::new(50.0, 0.0, 50.0)),
            0,
        );
    }

    #[test]
    fn walking_into_a_wall_slides_along_it() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        // A wall whose near face is at z = 1, i.e. inside the move's reach.
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, 0.0, 1.0), Vec3::new(10.0, 3.0, 2.0)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        // Settle on the floor first so the controller knows it is grounded.
        world.move_character(character, Vec3::ZERO, Vec3::Y);
        // Walk diagonally at the wall: +Z is into it, +X slides along it.
        let moved = world.move_character(character, Vec3::new(2.0, 0.0, 1.0), Vec3::Y);
        assert!(moved.hit_wall);
        assert!(
            moved.translation.x > 1.9,
            "slides along X: {}",
            moved.translation.x
        );
        let z = world.body(character).unwrap().position.z;
        let radius = 0.3;
        assert!(
            (z - (1.0 - radius)).abs() < 0.02,
            "stops one radius from the wall face, z = {z}"
        );
    }

    #[test]
    fn a_shallow_step_is_climbed() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        // A 0.25 m kerb in front of the character.
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, 0.0, 1.0), Vec3::new(10.0, 0.25, 10.0)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        world.move_character(character, Vec3::ZERO, Vec3::Y);
        let moved = world.move_character(character, Vec3::new(0.0, 0.0, 1.5), Vec3::Y);
        let position = world.body(character).unwrap().position;
        assert!(moved.grounded, "lands on the kerb");
        assert!(
            (position.y - (0.25 + 0.8)).abs() < 0.02,
            "feet on the 0.25 m kerb, centre y = {}",
            position.y
        );
        assert!(
            position.z > 1.0,
            "kept walking onto the kerb: z = {}",
            position.z
        );
    }

    #[test]
    fn a_step_above_the_limit_blocks() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        // 1.0 m tall: far above STEP_HEIGHT.
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, 0.0, 1.0), Vec3::new(10.0, 1.0, 10.0)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        world.move_character(character, Vec3::ZERO, Vec3::Y);
        let moved = world.move_character(character, Vec3::new(0.0, 0.0, 1.5), Vec3::Y);
        let position = world.body(character).unwrap().position;
        assert!(moved.hit_wall);
        assert!(position.y < 0.85, "did not climb: y = {}", position.y);
        assert!(
            (position.z - (1.0 - 0.3)).abs() < 0.02,
            "stopped at the face: z = {}",
            position.z
        );
    }

    #[test]
    fn falling_onto_ground_sets_grounded() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        let character = spawn_character(&mut world, 1.0);
        world.step(1.0 / 60.0);
        // Fall far enough to reach the floor in one move.
        let moved = world.move_character(character, Vec3::new(0.0, -2.0, 0.0), Vec3::Y);
        assert!(moved.grounded, "the sweep lands on the floor");
        assert!((moved.ground_normal - Vec3::Y).length() < 1e-4);
        assert!(moved.ground_body.is_some());
        let y = world.body(character).unwrap().position.y;
        assert!((y - 0.8).abs() < 0.02, "feet on the floor: centre y = {y}");
        assert!(!moved.hit_ceiling && !moved.hit_wall);
    }

    #[test]
    fn ground_snap_keeps_the_character_attached_across_a_step_down() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        // An upper platform for z < 0 and a lower one 0.15 m down for z >= 0.
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 0.0)),
            0,
        );
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, -1.0, 0.0), Vec3::new(10.0, -0.15, 10.0)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        let settled = world.move_character(character, Vec3::ZERO, Vec3::Y);
        assert!(settled.grounded);
        // Walk forward over the lip with no downward motion of our own.
        let moved = world.move_character(character, Vec3::new(0.0, 0.0, 1.0), Vec3::Y);
        assert!(moved.grounded, "the snap finds the lower platform");
        assert!(
            (moved.translation.y + 0.15).abs() < 0.02,
            "{:?}",
            moved.translation
        );
        assert!(
            (moved.translation.z - 1.0).abs() < 1e-3,
            "{:?}",
            moved.translation
        );
    }

    #[test]
    fn no_tunnelling_at_twenty_metres_per_second() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        // A thin wall (0.1 m) 4 m ahead.
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, 0.0, 4.0), Vec3::new(10.0, 3.0, 4.1)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        world.move_character(character, Vec3::ZERO, Vec3::Y);
        // 20 m/s for a 1/60 s frame is 0.333 m; run four frames of a *2 m*
        // move to prove the cast is continuous rather than positional.
        for _ in 0..4 {
            world.move_character(character, Vec3::new(0.0, 0.0, 2.0), Vec3::Y);
        }
        let z = world.body(character).unwrap().position.z;
        assert!(z < 4.0, "stopped in front of the wall, z = {z}");
        assert!(
            (z - (4.0 - 0.3)).abs() < 0.02,
            "rests against the wall, z = {z}"
        );
    }

    #[test]
    fn ceiling_is_reported() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        world.insert_static_aabb(
            Aabb::new(Vec3::new(-5.0, 2.2, -5.0), Vec3::new(5.0, 3.0, 5.0)),
            0,
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        let moved = world.move_character(character, Vec3::new(0.0, 1.5, 0.0), Vec3::Y);
        assert!(moved.hit_ceiling);
        let y = world.body(character).unwrap().position.y;
        assert!(
            y <= 2.2 - 0.8 + 0.01,
            "head stops under the ceiling: y = {y}"
        );
    }

    #[test]
    fn steep_slopes_are_not_climbed() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        // A 60-degree ramp: rotate a long slab about X.
        let ramp = world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::new(5.0, 0.25, 5.0),
            })
            .at(Vec3::new(0.0, 1.0, 3.0))
            .with_rotation(noxel_core::math::Quat::from_rotation_x(to_radians(-60.0))),
        );
        assert!(world.contains(ramp));
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        world.move_character(character, Vec3::ZERO, Vec3::Y);
        world.move_character(character, Vec3::new(0.0, 0.0, 1.0), Vec3::Y);
        let y = world.body(character).unwrap().position.y;
        assert!(y < 1.0, "did not walk far up a 60 degree slope: y = {y}");
    }

    #[test]
    fn character_ignores_its_own_body() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let character = spawn_character(&mut world, 0.0);
        // No floor at all: the character must be able to move freely.
        let moved = world.move_character(character, Vec3::new(1.0, 0.0, 0.0), Vec3::Y);
        assert!(
            (moved.translation.x - 1.0).abs() < 1e-4,
            "{:?}",
            moved.translation
        );
        assert!(!moved.grounded);
    }

    #[test]
    fn sensor_bodies_do_not_block() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        ground(&mut world);
        world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(0.5),
            })
            .at(Vec3::new(0.0, 0.5, 1.0))
            .as_sensor(),
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        let moved = world.move_character(character, Vec3::new(0.0, 0.0, 1.0), Vec3::Y);
        assert!(moved.translation.z > 0.5, "walked through the trigger");
    }

    #[test]
    fn kinematic_platform_carries_the_character() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let platform = world.insert(
            BodyDesc::kinematic(ColliderShape::Box {
                half_extents: Vec3::new(4.0, 0.5, 4.0),
            })
            .at(Vec3::new(0.0, -0.5, 0.0))
            .with_velocity(Vec3::new(2.0, 0.0, 0.0)),
        );
        let character = spawn_character(&mut world, 0.0);
        world.step(1.0 / 60.0);
        let settled = world.move_character(character, Vec3::ZERO, Vec3::Y);
        assert!(settled.grounded);
        assert_eq!(settled.ground_body, Some(platform));

        world.step(1.0 / 60.0);
        let moved = world.move_character(character, Vec3::ZERO, Vec3::Y);
        // The platform advanced 2 m/s * 1/60 s = 0.0333 m; the character
        // inherits exactly that.
        assert!(
            (moved.translation.x - 2.0 / 60.0).abs() < 1e-3,
            "{:?}",
            moved.translation
        );
        let dx =
            world.body(character).unwrap().position.x - world.body(platform).unwrap().position.x;
        assert!(
            dx.abs() < 2.0 / 60.0 + 1e-3,
            "stays on the platform, offset {dx}"
        );
    }

    #[test]
    fn invalid_handle_returns_a_default_move() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let moved = world.move_character(BodyHandle::INVALID, Vec3::X, Vec3::Y);
        assert_eq!(moved, CharacterMove::default());
    }

    #[test]
    fn walkable_predicate_matches_the_slope_limit() {
        let flat = Vec3::Y;
        assert!(is_walkable(flat, Vec3::Y));
        let at_limit = Vec3::new(MAX_SLOPE_ANGLE.sin(), MAX_SLOPE_ANGLE.cos(), 0.0);
        assert!(is_walkable(at_limit, Vec3::Y));
        let beyond = Vec3::new(to_radians(50.0).sin(), to_radians(50.0).cos(), 0.0);
        assert!(!is_walkable(beyond, Vec3::Y));
    }
}
