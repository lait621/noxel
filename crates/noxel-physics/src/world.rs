//! The physics world: broadphase, integration, sleeping, events and stats.
//!
//! # Step order
//!
//! 1. recover any non-finite state left by gameplay code,
//! 2. integrate velocities (gravity + damping),
//! 3. sync the spatial hash and collect overlapping pairs,
//! 4. narrowphase: rebuild the contact list,
//! 5. solve velocities (sequential impulses),
//! 6. integrate positions (semi-implicit Euler, the solver's velocities),
//! 7. correct positions (Baumgarte projection),
//! 8. wake bodies touched by moving neighbours, then update sleep timers,
//! 9. emit collision/trigger/sleep events,
//! 10. recover again, so nothing non-finite survives the step.
//!
//! Everything iterates the body slot map in slot order (or a `BTreeSet` of
//! handles), so two runs of the same scenario produce bit-identical results.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use noxel_core::math::{Aabb, Quat, Vec3};
use noxel_core::pool::SlotMap;
use noxel_core::spatial::SpatialHash;
use noxel_core::spatial::hash::HashEntry;

use crate::body::{Body, BodyDesc, BodyHandle};
use crate::config::PhysicsConfig;
use crate::events::PhysicsEvent;
use crate::narrow::{Contact, contact_between};
use crate::query::QueryFilter;
use crate::shape::ColliderShape;
use crate::solver::SolverScratch;

/// A snapshot of the world taken once per step.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhysicsStats {
    /// Number of live bodies.
    pub body_count: usize,
    /// Dynamic or kinematic bodies that are awake.
    pub active_bodies: usize,
    /// Bodies currently asleep.
    pub sleeping_bodies: usize,
    /// Body pairs the broadphase reported this step.
    pub broadphase_pairs: usize,
    /// Contacts the narrowphase produced this step.
    pub contact_count: usize,
    /// Iterations the solver ran this step.
    pub solver_iterations: u32,
}

/// The last known-good state of a body, used by the non-finite guard.
#[derive(Clone, Copy, Debug)]
struct BodyState {
    position: Vec3,
    rotation: Quat,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
}

impl BodyState {
    fn of(body: &Body) -> Self {
        Self {
            position: body.position,
            rotation: body.rotation,
            linear_velocity: body.linear_velocity,
            angular_velocity: body.angular_velocity,
        }
    }
}

impl Default for BodyState {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
        }
    }
}

/// The simulation: bodies, broadphase, contacts, events and queries.
///
/// ```
/// use noxel_core::math::Vec3;
/// use noxel_physics::{BodyDesc, ColliderShape, PhysicsConfig, PhysicsWorld};
///
/// let mut world = PhysicsWorld::new(PhysicsConfig::default());
/// let floor = world.insert_static_aabb(
///     noxel_core::math::Aabb::new(Vec3::new(-5.0, -1.0, -5.0), Vec3::new(5.0, 0.0, 5.0)),
///     1,
/// );
/// let ball = world.insert(
///     BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 }).at(Vec3::new(0.0, 2.0, 0.0)),
/// );
/// for _ in 0..120 {
///     world.step(1.0 / 60.0);
/// }
/// let y = world.body(ball).unwrap().position.y;
/// assert!((y - 0.5).abs() < 0.05, "the ball rests on the floor at y = 0.5, got {y}");
/// assert!(world.contains(floor));
/// ```
pub struct PhysicsWorld {
    config: PhysicsConfig,
    bodies: SlotMap<Body>,
    hash: SpatialHash<BodyHandle>,
    entries: HashMap<BodyHandle, noxel_core::pool::Handle<HashEntry<BodyHandle>>>,
    contacts: Vec<Contact>,
    solver: SolverScratch,
    events: Vec<PhysicsEvent>,
    solid_pairs: BTreeSet<(BodyHandle, BodyHandle)>,
    trigger_pairs: BTreeSet<(BodyHandle, BodyHandle)>,
    last_finite: HashMap<BodyHandle, BodyState>,
    character_ground: HashMap<BodyHandle, Option<BodyHandle>>,
    last_dt: f32,
    stats: PhysicsStats,
}

impl PhysicsWorld {
    /// Creates an empty world.
    #[must_use]
    pub fn new(config: PhysicsConfig) -> Self {
        let config = config.sanitized();
        Self {
            hash: SpatialHash::new(config.broadphase_cell_size),
            config,
            bodies: SlotMap::new(),
            entries: HashMap::new(),
            contacts: Vec::new(),
            solver: SolverScratch::default(),
            events: Vec::new(),
            solid_pairs: BTreeSet::new(),
            trigger_pairs: BTreeSet::new(),
            last_finite: HashMap::new(),
            character_ground: HashMap::new(),
            last_dt: 1.0 / 60.0,
            stats: PhysicsStats::default(),
        }
    }

    /// The active configuration.
    #[inline]
    #[must_use]
    pub fn config(&self) -> &PhysicsConfig {
        &self.config
    }

    /// Replaces the configuration and rebuilds the broadphase when the cell
    /// size changed.
    pub fn set_config(&mut self, config: PhysicsConfig) {
        let config = config.sanitized();
        let resize = (config.broadphase_cell_size - self.config.broadphase_cell_size).abs() > 1e-6;
        self.config = config;
        if resize {
            self.rebuild_broadphase();
        }
    }

    /// Sets world gravity.
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.config.gravity = if gravity.is_finite() { gravity } else { Vec3::ZERO };
    }

    /// World gravity.
    #[inline]
    #[must_use]
    pub fn gravity(&self) -> Vec3 {
        self.config.gravity
    }

    /// Inserts a body built from `desc`.
    ///
    /// Returns [`BodyHandle::INVALID`] when the world already holds
    /// `config.max_bodies` bodies; check with [`PhysicsWorld::contains`].
    pub fn insert(&mut self, desc: BodyDesc) -> BodyHandle {
        if self.bodies.len() >= self.config.max_bodies {
            return BodyHandle::INVALID;
        }
        let body = desc.build();
        let bounds = body.aabb();
        let handle = self.bodies.insert(body);
        let entry = self.hash.insert(bounds, handle);
        self.entries.insert(handle, entry);
        if let Some(body) = self.bodies.get(handle) {
            self.last_finite.insert(handle, BodyState::of(body));
        }
        handle
    }

    /// Inserts an immovable box body covering `bounds`.
    ///
    /// The usual way to turn a level's collision mesh into physics geometry.
    pub fn insert_static_aabb(&mut self, bounds: Aabb, user_data: u64) -> BodyHandle {
        let half = bounds.half_extents().max(Vec3::splat(1e-4));
        let center = bounds.center();
        let desc = BodyDesc::static_body(ColliderShape::Box { half_extents: half })
            .at(center)
            .with_user_data(user_data);
        self.insert(desc)
    }

    /// Removes a body and every record the world keeps about it.
    ///
    /// Returns `false` when the handle is stale.
    pub fn remove(&mut self, handle: BodyHandle) -> bool {
        if self.bodies.remove(handle).is_none() {
            return false;
        }
        if let Some(entry) = self.entries.remove(&handle) {
            self.hash.remove(entry);
        }
        self.last_finite.remove(&handle);
        self.character_ground.remove(&handle);
        self.solid_pairs.retain(|&(a, b)| a != handle && b != handle);
        self.trigger_pairs.retain(|&(a, b)| a != handle && b != handle);
        self.contacts.retain(|c| c.a != handle && c.b != handle);
        true
    }

    /// True when the handle refers to a live body.
    #[inline]
    #[must_use]
    pub fn contains(&self, handle: BodyHandle) -> bool {
        self.bodies.contains(handle)
    }

    /// Shared access to a body.
    #[inline]
    #[must_use]
    pub fn body(&self, handle: BodyHandle) -> Option<&Body> {
        self.bodies.get(handle)
    }

    /// Mutable access to a body.
    ///
    /// Position changes made through this method are picked up by the next
    /// [`PhysicsWorld::step`]; prefer [`PhysicsWorld::set_position`] when the
    /// change must be visible to queries immediately.
    #[inline]
    #[must_use]
    pub fn body_mut(&mut self, handle: BodyHandle) -> Option<&mut Body> {
        self.bodies.get_mut(handle)
    }

    /// Every live body, in slot order (deterministic).
    pub fn bodies(&self) -> impl Iterator<Item = (BodyHandle, &Body)> + '_ {
        self.bodies.iter()
    }

    /// Number of live bodies.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// True when the world holds no bodies.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Removes every body and clears all contact and event history.
    pub fn clear(&mut self) {
        self.bodies.clear();
        self.hash.reset();
        self.entries.clear();
        self.contacts.clear();
        self.events.clear();
        self.solid_pairs.clear();
        self.trigger_pairs.clear();
        self.last_finite.clear();
        self.character_ground.clear();
        self.stats = PhysicsStats::default();
    }

    /// Teleports a body, refreshes the broadphase and wakes it.
    ///
    /// Returns `false` for a stale handle or a non-finite position.
    pub fn set_position(&mut self, handle: BodyHandle, position: Vec3) -> bool {
        if !position.is_finite() {
            return false;
        }
        let Some(body) = self.bodies.get(handle) else { return false };
        if body.position == position {
            self.wake_body(handle);
            return true;
        }
        if let Some(body) = self.bodies.get_mut(handle) {
            body.position = position;
        }
        self.wake_body(handle);
        self.sync_entry(handle);
        true
    }

    /// Replaces a body's orientation, refreshes the broadphase and wakes it.
    ///
    /// Returns `false` for a stale handle or a non-finite rotation.
    pub fn set_rotation(&mut self, handle: BodyHandle, rotation: Quat) -> bool {
        if !rotation.is_finite() {
            return false;
        }
        let Some(body) = self.bodies.get(handle) else { return false };
        let rotation = rotation.normalize();
        if body.rotation == rotation {
            self.wake_body(handle);
            return true;
        }
        if let Some(body) = self.bodies.get_mut(handle) {
            body.rotation = rotation;
        }
        self.wake_body(handle);
        self.sync_entry(handle);
        true
    }

    /// Replaces a body's linear velocity and wakes it.
    ///
    /// Returns `false` for a stale handle or a non-finite velocity.
    pub fn set_velocity(&mut self, handle: BodyHandle, velocity: Vec3) -> bool {
        if !velocity.is_finite() {
            return false;
        }
        if self.bodies.get(handle).is_none() {
            return false;
        }
        if let Some(body) = self.bodies.get_mut(handle) {
            body.linear_velocity = velocity;
        }
        self.wake_body(handle);
        true
    }

    /// Moves a body by `delta`, refreshing the broadphase, and wakes it.
    ///
    /// Intended for kinematic platforms and doors. Static bodies are refused
    /// (level geometry is built once), and non-finite deltas are ignored.
    pub fn translate_kinematic(&mut self, handle: BodyHandle, delta: Vec3) -> bool {
        if !delta.is_finite() {
            return false;
        }
        let Some(body) = self.bodies.get(handle) else { return false };
        if body.is_static() {
            return false;
        }
        let target = body.position + delta;
        self.set_position(handle, target)
    }

    /// Advances the simulation by one fixed step.
    ///
    /// `dt` is clamped to `config.max_step_delta`; a non-finite or
    /// non-positive `dt` is ignored entirely.
    pub fn step(&mut self, dt: f32) {
        let dt = self.clamp_dt(dt);
        self.last_dt = if dt > 0.0 { dt } else { self.last_dt };
        self.stats.solver_iterations = 0;
        if dt <= 0.0 {
            return;
        }
        self.recover_non_finite();
        self.integrate_velocities(dt);
        self.sync_broadphase();
        self.build_contacts();
        self.solve_velocities();
        self.integrate_positions(dt);
        self.solver.correct_positions(&mut self.bodies, &self.contacts, self.config.position_correction);
        self.wake_on_contact();
        self.update_sleeping(dt);
        self.emit_events();
        self.recover_non_finite();
        self.update_stats();
    }

    /// The contacts produced by the most recent step.
    #[inline]
    #[must_use]
    pub fn contacts(&self) -> &[Contact] {
        &self.contacts
    }

    /// Events accumulated since the last [`PhysicsWorld::clear_events`].
    #[inline]
    #[must_use]
    pub fn events(&self) -> &[PhysicsEvent] {
        &self.events
    }

    /// Empties the event list. Call once per frame, after consuming the events.
    #[inline]
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Counters describing the most recent step.
    #[inline]
    #[must_use]
    pub fn stats(&self) -> PhysicsStats {
        self.stats
    }

    /// The `dt` of the most recent step, in seconds.
    #[inline]
    #[must_use]
    pub fn last_step_delta(&self) -> f32 {
        self.last_dt
    }

    /// The direction and length the ground of a character last rested on.
    #[inline]
    pub(crate) fn set_character_ground(&mut self, handle: BodyHandle, ground: Option<BodyHandle>) {
        self.character_ground.insert(handle, ground);
    }

    /// The body a character last stood on, if any.
    #[inline]
    #[must_use]
    pub(crate) fn character_ground(&self, handle: BodyHandle) -> Option<BodyHandle> {
        self.character_ground.get(&handle).copied().flatten()
    }

    // -- internals ---------------------------------------------------------

    fn clamp_dt(&self, dt: f32) -> f32 {
        if !dt.is_finite() || dt <= 0.0 { 0.0 } else { dt.min(self.config.max_step_delta) }
    }

    /// Clears the sleep flag and records a `BodyWoke` event when it was set.
    fn wake_body(&mut self, handle: BodyHandle) {
        let mut woke = false;
        if let Some(body) = self.bodies.get_mut(handle) {
            if body.sleeping {
                woke = true;
            }
            body.wake();
        }
        if woke {
            self.events.push(PhysicsEvent::BodyWoke { body: handle });
        }
    }

    /// Rebuilds the spatial hash from scratch.
    fn rebuild_broadphase(&mut self) {
        self.hash = SpatialHash::new(self.config.broadphase_cell_size);
        self.entries.clear();
        let handles: Vec<BodyHandle> = self.bodies.keys().collect();
        for handle in handles {
            if let Some(body) = self.bodies.get(handle) {
                let entry = self.hash.insert(body.aabb(), handle);
                self.entries.insert(handle, entry);
            }
        }
    }

    /// Re-registers one body's bounds in the hash.
    pub(crate) fn sync_entry(&mut self, handle: BodyHandle) {
        let Some(body) = self.bodies.get(handle) else { return };
        let bounds = finite_bounds(body.aabb());
        match self.entries.get(&handle).copied() {
            Some(entry) => {
                self.hash.update(entry, bounds);
            }
            None => {
                let entry = self.hash.insert(bounds, handle);
                self.entries.insert(handle, entry);
            }
        }
    }

    /// Every body whose broadphase bounds overlap `bounds` and that passes
    /// `filter`, sorted by handle.
    ///
    /// `out` is cleared first. This is the entry point every query uses before
    /// its analytic test.
    pub(crate) fn gather_candidates(
        &self,
        bounds: Aabb,
        filter: &QueryFilter,
        out: &mut Vec<BodyHandle>,
    ) {
        // The spatial hash hands back its own entry handles; map them to the
        // bodies and drop whatever the filter rejects.
        let mut entries: Vec<noxel_core::pool::Handle<HashEntry<BodyHandle>>> = Vec::new();
        self.hash.query_aabb(bounds, &mut entries);
        out.clear();
        for entry in entries {
            let Some(&handle) = self.hash.get(entry) else { continue };
            if filter.ignore == Some(handle) {
                continue;
            }
            if self.bodies.get(handle).is_some_and(|body| filter.accepts(body)) {
                out.push(handle);
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Re-registers every body's bounds (once per step).
    fn sync_broadphase(&mut self) {
        let handles: Vec<BodyHandle> = self.bodies.keys().collect();
        for handle in handles {
            self.sync_entry(handle);
        }
    }

    fn integrate_velocities(&mut self, dt: f32) {
        let gravity = self.config.gravity;
        for (_, body) in self.bodies.iter_mut() {
            if body.sleeping || !body.is_dynamic() {
                continue;
            }
            body.linear_velocity += gravity * (body.gravity_scale * dt);
            // Exponential decay is the frame-rate independent form: two half
            // steps leave exactly the same residue as one whole step.
            body.linear_velocity *= (-body.linear_damping * dt).exp();
            body.angular_velocity *= (-body.angular_damping * dt).exp();
        }
    }

    fn integrate_positions(&mut self, dt: f32) {
        for (_, body) in self.bodies.iter_mut() {
            if body.sleeping || body.is_static() {
                continue;
            }
            body.position += body.linear_velocity * dt;
            let spin = body.angular_velocity;
            let speed = spin.length();
            if speed > 1e-6 {
                let delta = Quat::from_axis_angle(spin / speed, speed * dt);
                body.rotation = (delta * body.rotation).normalize();
            }
        }
    }

    fn build_contacts(&mut self) {
        self.contacts.clear();
        self.stats.broadphase_pairs = 0;
        let mut pairs: Vec<(BodyHandle, BodyHandle)> = Vec::new();
        {
            let bodies = &self.bodies;
            let hash = &self.hash;
            hash.for_each_overlapping_pair(|entry_a, entry_b| {
                let (Some(&raw_a), Some(&raw_b)) = (hash.get(entry_a), hash.get(entry_b)) else {
                    return;
                };
                if raw_a == raw_b {
                    return;
                }
                // Canonical order: the lower handle is always `a`, so both the
                // pair set and the contact list are stable.
                let (a, b) = if raw_a < raw_b { (raw_a, raw_b) } else { (raw_b, raw_a) };
                let (Some(body_a), Some(body_b)) = (bodies.get(a), bodies.get(b)) else {
                    return;
                };
                if !body_a.can_collide_with(body_b) {
                    return;
                }
                if body_a.is_static() && body_b.is_static() {
                    return;
                }
                let sensor = body_a.is_sensor || body_b.is_sensor;
                if !sensor && !body_a.is_dynamic() && !body_b.is_dynamic() {
                    // Nothing here can move or respond.
                    return;
                }
                pairs.push((a, b));
            });
        }
        pairs.sort_unstable();
        pairs.dedup();
        self.stats.broadphase_pairs = pairs.len();

        let tolerance = self.config.contact_tolerance;
        for &(a, b) in &pairs {
            let (Some(body_a), Some(body_b)) = (self.bodies.get(a), self.bodies.get(b)) else {
                continue;
            };
            if let Some(contact) = contact_between(a, b, body_a, body_b, tolerance) {
                self.contacts.push(contact);
            }
        }
        self.solver.prepare(&self.bodies, &self.contacts);
    }

    fn solve_velocities(&mut self) {
        let iterations = self.config.solver_iterations;
        self.stats.solver_iterations = iterations;
        self.solver.solve_velocities(&mut self.bodies, &self.contacts, iterations);
    }

    /// Wakes sleeping bodies that an awake neighbour reached or pushed.
    ///
    /// Two triggers, both requiring the other body to be awake and not static:
    ///
    /// * the contact is **new** this step — something arrived, so the sleeper
    ///   must respond. This catches a projectile whose impact the solver has
    ///   already absorbed, which would otherwise look motionless by now;
    /// * the neighbour is still **moving** after the solve, which catches a
    ///   kinematic platform that was already touching the sleeper and started
    ///   moving.
    ///
    /// A resting stack triggers neither: its contacts persist across steps and
    /// its post-solve velocities are zero, so its members do not wake each
    /// other in a loop.
    fn wake_on_contact(&mut self) {
        let linear = self.config.sleep_linear_threshold;
        let angular = self.config.sleep_angular_threshold;
        let mut to_wake: BTreeSet<BodyHandle> = BTreeSet::new();
        for contact in &self.contacts {
            if contact.is_sensor {
                continue;
            }
            let is_new = !self.solid_pairs.contains(&(contact.a, contact.b));
            let (Some(body_a), Some(body_b)) =
                (self.bodies.get(contact.a), self.bodies.get(contact.b))
            else {
                continue;
            };
            if body_a.sleeping
                && !body_b.sleeping
                && !body_b.is_static()
                && (is_new || body_b.is_moving_faster_than(linear, angular))
            {
                to_wake.insert(contact.a);
            }
            if body_b.sleeping
                && !body_a.sleeping
                && !body_a.is_static()
                && (is_new || body_a.is_moving_faster_than(linear, angular))
            {
                to_wake.insert(contact.b);
            }
        }
        for handle in to_wake {
            if self.bodies.get(handle).is_some_and(|b| b.sleeping) {
                self.wake_body(handle);
            }
        }
    }

    fn update_sleeping(&mut self, dt: f32) {
        let linear = self.config.sleep_linear_threshold;
        let angular = self.config.sleep_angular_threshold;
        let required = self.config.sleep_time_required;
        let mut slept: Vec<BodyHandle> = Vec::new();
        let mut woke: Vec<BodyHandle> = Vec::new();
        for (handle, body) in self.bodies.iter_mut() {
            if !body.is_dynamic() {
                continue;
            }
            if body.is_moving_faster_than(linear, angular) {
                body.sleep_timer = 0.0;
                if body.sleeping {
                    body.sleeping = false;
                    woke.push(handle);
                }
            } else {
                body.sleep_timer += dt;
                if !body.sleeping && body.sleep_timer >= required {
                    body.sleeping = true;
                    body.linear_velocity = Vec3::ZERO;
                    body.angular_velocity = Vec3::ZERO;
                    slept.push(handle);
                }
            }
        }
        for handle in slept {
            self.events.push(PhysicsEvent::BodySlept { body: handle });
        }
        for handle in woke {
            self.events.push(PhysicsEvent::BodyWoke { body: handle });
        }
    }

    fn emit_events(&mut self) {
        let mut solid: BTreeMap<(BodyHandle, BodyHandle), usize> = BTreeMap::new();
        let mut triggers: BTreeMap<(BodyHandle, BodyHandle), usize> = BTreeMap::new();
        for (index, contact) in self.contacts.iter().enumerate() {
            if contact.is_sensor {
                let a_is_sensor = self.bodies.get(contact.a).is_some_and(|b| b.is_sensor);
                let pair = if a_is_sensor {
                    (contact.a, contact.b)
                } else {
                    (contact.b, contact.a)
                };
                triggers.insert(pair, index);
            } else {
                solid.insert((contact.a, contact.b), index);
            }
        }

        for (&pair, &index) in &solid {
            if !self.solid_pairs.contains(&pair) {
                let contact = self.contacts[index];
                let impulse = self.solver.normal_impulse(index);
                self.events.push(PhysicsEvent::CollisionEnter {
                    a: pair.0,
                    b: pair.1,
                    point: contact.point,
                    normal: contact.normal,
                    impulse,
                });
            }
        }
        for &pair in &self.solid_pairs {
            if !solid.contains_key(&pair) {
                self.events.push(PhysicsEvent::CollisionExit { a: pair.0, b: pair.1 });
            }
        }
        self.solid_pairs = solid.into_keys().collect();

        for (&pair, &index) in &triggers {
            if !self.trigger_pairs.contains(&pair) {
                // Reuse the contact point as the trigger's observation point.
                let _ = index;
                self.events
                    .push(PhysicsEvent::TriggerEnter { trigger: pair.0, other: pair.1 });
            }
        }
        for &pair in &self.trigger_pairs {
            if !triggers.contains_key(&pair) {
                self.events.push(PhysicsEvent::TriggerExit { trigger: pair.0, other: pair.1 });
            }
        }
        self.trigger_pairs = triggers.into_keys().collect();
    }

    /// Restores the last finite state of any body that has gone non-finite.
    ///
    /// The recovery is deliberately quiet: the body is put to sleep at its last
    /// good position with zero velocity, which is the mildest outcome the rest
    /// of the engine can observe.
    fn recover_non_finite(&mut self) {
        for (handle, body) in self.bodies.iter_mut() {
            if body.is_finite() {
                self.last_finite.insert(handle, BodyState::of(body));
                continue;
            }
            let previous = self.last_finite.get(&handle).copied().unwrap_or_default();
            body.position = previous.position;
            body.rotation = previous.rotation;
            body.linear_velocity = previous.linear_velocity;
            body.angular_velocity = previous.angular_velocity;
            let was_awake = !body.sleeping;
            body.sleeping = true;
            body.sleep_timer = 0.0;
            if was_awake {
                self.events.push(PhysicsEvent::BodySlept { body: handle });
            }
        }
    }

    fn update_stats(&mut self) {
        let mut active = 0usize;
        let mut sleeping = 0usize;
        for (_, body) in self.bodies.iter() {
            if body.sleeping {
                sleeping += 1;
            } else if !body.is_static() {
                active += 1;
            }
        }
        self.stats = PhysicsStats {
            body_count: self.bodies.len(),
            active_bodies: active,
            sleeping_bodies: sleeping,
            broadphase_pairs: self.stats.broadphase_pairs,
            contact_count: self.contacts.len(),
            solver_iterations: self.stats.solver_iterations,
        };
    }
}

/// Bounds guaranteed to be finite so a poisoned body cannot wreck the hash.
#[inline]
fn finite_bounds(bounds: Aabb) -> Aabb {
    if bounds.is_finite() {
        bounds
    } else {
        Aabb::from_center_half_extents(Vec3::ZERO, Vec3::splat(1e-3))
    }
}
