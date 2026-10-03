//! Steering: the forces that turn "where the agent wants to go" into motion.
//!
//! Each frame, an agent has a *desired direction* — from its path, or from a
//! flow field. [`Steering::force`] turns that into an acceleration by summing
//! the classic Reynolds behaviours:
//!
//! | Term | What it does | Default weight |
//! |---|---|---|
//! | seek | accelerates towards the desired direction | 1.0 |
//! | separation | pushes away from neighbours that are too close | 1.6 |
//! | alignment | matches the average heading of the neighbours | 0.15 |
//! | cohesion | drifts towards the centre of the neighbours | 0.1 |
//!
//! The sum is truncated to [`SteeringWeights::max_force`], which is what keeps a
//! crush of twenty agents in a doorway from launching anybody: separation grows
//! with the crowd, the clamp does not.
//!
//! Obstacle avoidance and the road bias are separate because they need the
//! world: [`Steering::avoid`] probes left, centre and right with the streamer's
//! walkability, and the crowd system adds the two extra terms before clamping
//! the total a second time.
//!
//! ## Units
//!
//! Forces are accelerations in metres per second squared. With the default
//! weights the maximum is 12 m/s², roughly a person's sprint start; typical
//! values are 1–3 m/s² walking, so an agent reaches its 1.6 m/s cruising speed
//! in about a second.

use noxel_core::math::{Aabb, Quat, Vec3};
use noxel_core::pool::Handle;
use noxel_core::spatial::SpatialHash;
use noxel_core::spatial::hash::HashEntry;
use noxel_world::WorldStreamer;

use crate::agent::NpcAgent;
use crate::crowd::CrowdManager;

/// A lateral obstacle probe is taken at this angle from the desired heading,
/// in radians (40°).
pub const AVOID_ANGLE: f32 = 0.698;

/// How far ahead a probe looks when the caller does not say, in metres.
pub const DEFAULT_PROBE: f32 = 2.0;

/// The steepest rise a probe may cross, as rise over run.
pub const AVOID_MAX_RISE: f32 = 0.8;

/// The weights of the steering behaviours.
///
/// The defaults are tuned for a town: enough separation that a queue outside a
/// bakery stays a queue rather than a pile, and enough seek that an agent that
/// has been pushed off its path walks back onto it.
#[derive(Clone, Copy, Debug)]
pub struct SteeringWeights {
    /// Weight of the pull towards the desired direction.
    pub seek: f32,
    /// Weight of the push away from crowded neighbours.
    pub separation: f32,
    /// Weight of the pull towards the neighbours' average heading.
    pub alignment: f32,
    /// Weight of the pull towards the neighbours' centre.
    pub cohesion: f32,
    /// Weight of the obstacle-avoidance direction.
    pub obstacle: f32,
    /// Weight of the pull towards the nearest road.
    pub road_bias: f32,
    /// Hard cap on the total steering acceleration, in m/s².
    pub max_force: f32,
    /// Radius in which neighbours are considered for alignment and cohesion.
    pub neighbour_radius: f32,
    /// Radius inside which neighbours push each other apart.
    pub separation_radius: f32,
}

impl Default for SteeringWeights {
    fn default() -> Self {
        Self {
            seek: 1.0,
            separation: 1.6,
            alignment: 0.15,
            cohesion: 0.1,
            obstacle: 2.0,
            road_bias: 0.25,
            max_force: 12.0,
            neighbour_radius: 2.0,
            separation_radius: 0.9,
        }
    }
}

impl SteeringWeights {
    /// The same weights with every value finite and inside a usable range.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.seek = finite_weight(self.seek, 1.0);
        self.separation = finite_weight(self.separation, 1.6);
        self.alignment = finite_weight(self.alignment, 0.15);
        self.cohesion = finite_weight(self.cohesion, 0.1);
        self.obstacle = finite_weight(self.obstacle, 2.0);
        self.road_bias = finite_weight(self.road_bias, 0.25);
        self.max_force = finite_weight(self.max_force, 12.0).max(0.1);
        self.neighbour_radius = finite_radius(self.neighbour_radius, 2.0);
        self.separation_radius = finite_radius(self.separation_radius, 0.9);
        self
    }
}

/// A finite, non-negative weight.
fn finite_weight(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

/// A finite radius of at least a centimetre.
fn finite_radius(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.01 {
        value
    } else {
        fallback
    }
}

/// The steering behaviours, as free functions over the crowd.
///
/// The type is deliberately a namespace rather than a struct with state: a
/// force depends only on the agent, its neighbours and the weights, which is
/// what makes steering deterministic and trivially parallelisable later.
pub struct Steering;

impl Steering {
    /// Builds a spatial hash of agent positions for neighbour queries.
    ///
    /// `cell` is the hash's cell size. Agents are inserted as boxes of half a
    /// cell, so pass at least twice the steering neighbourhood radius or a
    /// query can miss a neighbour that is standing just across a cell border.
    ///
    /// Queries are conservative: [`SpatialHash`] returns every agent whose box
    /// meets the query box, which can include candidates up to a cell beyond
    /// the radius. [`Steering::force`] filters them by true distance, so the
    /// extra candidates cost a little time and change nothing else.
    #[must_use]
    pub fn build_neighbours(agents: &CrowdManager, cell: f32) -> SpatialHash<u32> {
        let cell = sanitise_cell(cell);
        let mut hash = SpatialHash::new(cell);
        fill_neighbours(&mut hash, agents, cell);
        hash
    }

    /// The steering force for one agent, in metres per second squared.
    ///
    /// The result contains seek, separation, alignment and cohesion, truncated
    /// to [`SteeringWeights::max_force`]. Obstacle avoidance and the road bias
    /// are **not** included, because both need the world rather than the crowd;
    /// the caller adds `Steering::avoid(..) * weights.obstacle` and its road
    /// term, then clamps the sum again.
    #[must_use]
    pub fn force(
        agent: &NpcAgent,
        desired_direction: Vec3,
        neighbours: &SpatialHash<u32>,
        crowd: &CrowdManager,
        weights: &SteeringWeights,
    ) -> Vec3 {
        let radius = weights.neighbour_radius.max(0.01);
        let query = Aabb::new(
            agent.position - Vec3::new(radius, 1.0, radius),
            agent.position + Vec3::new(radius, 1.0, radius),
        );
        let mut handles: Vec<Handle<HashEntry<u32>>> = Vec::new();
        let mut scratch: Vec<Handle<HashEntry<u32>>> = Vec::new();
        neighbours.query_aabb_into(query, &mut handles, &mut scratch);
        force_from_handles(
            agent,
            desired_direction,
            &handles,
            neighbours,
            crowd,
            weights,
        )
    }

    /// Cheap obstacle avoidance: probe left, centre and right and pick the
    /// clearest.
    ///
    /// Returns the direction to walk instead of `desired` — a unit vector, or
    /// [`Vec3::ZERO`] when every probe is blocked, which the caller should read
    /// as "no safe direction this step". The probe checks both the walkability
    /// of the ground at half and full reach and the rise along it, so an agent
    /// walks around a house rather than into it while still being willing to
    /// climb a kerb.
    #[must_use]
    pub fn avoid(agent: &NpcAgent, desired: Vec3, streamer: &WorldStreamer, probe: f32) -> Vec3 {
        let probe = if probe.is_finite() && probe > 0.05 {
            probe.min(20.0)
        } else {
            DEFAULT_PROBE
        };
        let base = Vec3::new(desired.x, 0.0, desired.z).normalize_or_zero();
        if base.length_squared() <= 0.0 || !agent.position.is_finite() {
            return Vec3::ZERO;
        }
        // Centre first: when the way ahead is as clear as either side, the
        // agent should not weave.
        let mut best = Vec3::ZERO;
        let mut best_score = 0.0;
        for angle in [0.0, AVOID_ANGLE, -AVOID_ANGLE] {
            let dir = if angle == 0.0 {
                base
            } else {
                Quat::from_rotation_y(angle) * base
            };
            let score = clearance(agent.position, dir, streamer, probe);
            if score > best_score {
                best_score = score;
                best = dir;
            }
        }
        if best_score <= 0.0 { Vec3::ZERO } else { best }
    }
}

/// How far along `dir` the agent can walk before something blocks it.
///
/// Returns `probe` for a fully clear probe, `probe * 0.5` when only the near
/// sample is clear, and `0.0` when even that is blocked.
fn clearance(position: Vec3, dir: Vec3, streamer: &WorldStreamer, probe: f32) -> f32 {
    let near = position + dir * (probe * 0.5);
    let far = position + dir * probe;
    if !walkable_step(streamer, position, far, probe) {
        return if walkable_step(streamer, position, near, probe * 0.5) {
            probe * 0.5
        } else {
            0.0
        };
    }
    probe
}

/// True when an agent can take one step from `from` to `to`.
fn walkable_step(streamer: &WorldStreamer, from: Vec3, to: Vec3, distance: f32) -> bool {
    if !to.is_finite() || !streamer.is_walkable(to) {
        return false;
    }
    if distance <= 1e-3 {
        return true;
    }
    let rise = (streamer.height_at(to) - streamer.height_at(from)).abs() / distance;
    rise <= AVOID_MAX_RISE
}

/// The steering force over a pre-gathered neighbour set.
///
/// [`Steering::force`] wraps this; the crowd system calls it directly so a
/// frame's thousand steering queries reuse one scratch allocation instead of
/// allocating per agent.
#[must_use]
pub(crate) fn force_from_handles(
    agent: &NpcAgent,
    desired_direction: Vec3,
    handles: &[Handle<HashEntry<u32>>],
    neighbours: &SpatialHash<u32>,
    crowd: &CrowdManager,
    weights: &SteeringWeights,
) -> Vec3 {
    let desired = Vec3::new(desired_direction.x, 0.0, desired_direction.z).normalize_or_zero();
    let mut force = desired * weights.seek;
    if !agent.position.is_finite() {
        return clamp_length(force, weights.max_force);
    }
    let neighbour_radius = weights.neighbour_radius.max(0.01);
    let separation_radius = weights.separation_radius.max(0.01);
    let mut separation = Vec3::ZERO;
    let mut align_sum = Vec3::ZERO;
    let mut cohesion_sum = Vec3::ZERO;
    let mut counted = 0.0f32;
    for handle in handles {
        let Some(&index) = neighbours.get(*handle) else {
            continue;
        };
        if index as usize == agent.id.index() {
            continue;
        }
        let Some(other) = crowd.agent_at(index as usize) else {
            continue;
        };
        if !other.position.is_finite() {
            continue;
        }
        let dx = agent.position.x - other.position.x;
        let dz = agent.position.z - other.position.z;
        let distance_sq = dx * dx + dz * dz;
        if distance_sq >= neighbour_radius * neighbour_radius {
            continue;
        }
        counted += 1.0;
        align_sum += other.velocity;
        cohesion_sum += other.position;
        if distance_sq <= separation_radius * separation_radius {
            if distance_sq > 1e-8 {
                let distance = distance_sq.sqrt();
                let strength = 1.0 - distance / separation_radius;
                separation += Vec3::new(dx / distance, 0.0, dz / distance) * strength;
            } else {
                // Exactly coincident: no direction is defined, so derive a
                // stable one from the two ids rather than dividing by zero.
                separation += fallback_separation(agent.id.0, other.id.0);
            }
        }
    }
    let mut total = force + separation * weights.separation;
    if counted > 0.0 {
        let average_velocity = align_sum / counted;
        let average_position = cohesion_sum / counted;
        let alignment = Vec3::new(
            average_velocity.x - agent.velocity.x,
            0.0,
            average_velocity.z - agent.velocity.z,
        )
        .normalize_or_zero();
        let cohesion = Vec3::new(
            average_position.x - agent.position.x,
            0.0,
            average_position.z - agent.position.z,
        )
        .normalize_or_zero();
        total += alignment * weights.alignment + cohesion * weights.cohesion;
    }
    force = total;
    clamp_length(force, weights.max_force)
}

/// Clamps a neighbour-hash cell size into a usable range.
fn sanitise_cell(cell: f32) -> f32 {
    if cell.is_finite() && cell >= 0.1 {
        cell.min(64.0)
    } else {
        4.0
    }
}

/// Refills a neighbour hash with the crowd's current positions.
///
/// The hash is cleared first but keeps its allocations, which is what lets the
/// crowd rebuild it every step for a thousand agents without touching the
/// allocator more than once.
pub(crate) fn fill_neighbours(hash: &mut SpatialHash<u32>, agents: &CrowdManager, cell: f32) {
    let cell = sanitise_cell(cell);
    hash.clear();
    let half = Vec3::new(cell * 0.5, 1.0, cell * 0.5);
    for (index, slot) in agents.slots().iter().enumerate() {
        let Some(agent) = slot else { continue };
        if !agent.position.is_finite() {
            continue;
        }
        hash.insert(
            Aabb::new(agent.position - half, agent.position + half),
            index as u32,
        );
    }
}

/// A deterministic push direction for two agents standing on the same spot.
fn fallback_separation(a: u32, b: u32) -> Vec3 {
    let mix = a.wrapping_mul(2_654_435_761) ^ b.wrapping_mul(40_503);
    let angle = (mix % 3600) as f32 * (core::f32::consts::TAU / 3600.0);
    Vec3::new(angle.cos(), 0.0, angle.sin())
}

/// Truncates a vector to `max` metres per second squared.
#[must_use]
pub fn clamp_length(v: Vec3, max: f32) -> Vec3 {
    if !v.is_finite() {
        return Vec3::ZERO;
    }
    let max = if max.is_finite() { max.max(0.0) } else { 0.0 };
    let length_sq = v.length_squared();
    if length_sq > max * max && length_sq > 0.0 {
        v * (max / length_sq.sqrt())
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights() -> SteeringWeights {
        SteeringWeights::default()
    }

    #[test]
    fn default_weights_match_the_contract() {
        let w = weights();
        assert_eq!(w.seek, 1.0);
        assert_eq!(w.separation, 1.6);
        assert_eq!(w.alignment, 0.15);
        assert_eq!(w.cohesion, 0.1);
        assert_eq!(w.obstacle, 2.0);
        assert_eq!(w.road_bias, 0.25);
        assert_eq!(w.max_force, 12.0);
        assert_eq!(w.neighbour_radius, 2.0);
        assert_eq!(w.separation_radius, 0.9);
    }

    #[test]
    fn sanitising_replaces_broken_weights() {
        let broken = SteeringWeights {
            seek: f32::NAN,
            separation: -1.0,
            alignment: f32::INFINITY,
            cohesion: 0.0,
            obstacle: f32::NAN,
            road_bias: 2.0,
            max_force: 0.0,
            neighbour_radius: -5.0,
            separation_radius: f32::NAN,
        }
        .sanitised();
        assert_eq!(broken.seek, 1.0);
        assert_eq!(broken.separation, 1.6);
        assert_eq!(broken.alignment, 0.15);
        assert_eq!(broken.cohesion, 0.0);
        assert_eq!(broken.obstacle, 2.0);
        assert_eq!(broken.road_bias, 2.0);
        assert_eq!(broken.max_force, 0.1);
        assert_eq!(broken.neighbour_radius, 2.0);
        assert_eq!(broken.separation_radius, 0.9);
    }

    #[test]
    fn clamp_length_truncates_and_survives_nan() {
        let v = clamp_length(Vec3::new(3.0, 4.0, 0.0), 1.0);
        assert!((v.length() - 1.0).abs() < 1e-5);
        assert_eq!(clamp_length(Vec3::new(f32::NAN, 0.0, 0.0), 1.0), Vec3::ZERO);
        assert_eq!(clamp_length(Vec3::X, f32::NAN), Vec3::ZERO);
        assert_eq!(clamp_length(Vec3::new(0.1, 0.0, 0.0), 1.0).x, 0.1);
    }

    #[test]
    fn fallback_separation_is_deterministic_and_unit() {
        let a = fallback_separation(1, 2);
        let b = fallback_separation(1, 2);
        assert_eq!(a, b);
        assert!((a.length() - 1.0).abs() < 1e-4);
        assert!(a.y == 0.0);
        assert_ne!(a, fallback_separation(2, 1));
    }
}
