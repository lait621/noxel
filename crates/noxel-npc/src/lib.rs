//! # Noxel NPC
//!
//! A crowd simulator for a town full of people: spawn and despawn around a
//! moving focus, level-of-detail tiers, A\* and flow-field pathfinding, steering,
//! and a daily schedule per agent kind. **Zero `unsafe`, zero third-party
//! dependencies**, and deterministic: the same seed produces the same town.
//!
//! ## The requirement this crate exists to satisfy
//!
//! *"Support large numbers of simultaneous NPCs."* Concretely: **1000+ resident
//! agents inside a frame budget of about four milliseconds**. Four decisions get
//! the crowd there:
//!
//! | Decision | Why it matters |
//! |---|---|
//! | Crowd tiers ([`TierConfig`]) | Only the 25 m bubble is physics-backed and steered every step; the rest of the town advances on a coarse clock |
//! | Flow fields ([`FlowField`]) | Two hundred villagers walking to the same stall cost *one* search, not two hundred |
//! | A per-step path budget | A\* is capped at [`path::MAX_PATHS_PER_STEP`] searches and [`path::MAX_NODES_PER_STEP`] nodes per frame, so a repath storm cannot blow the frame |
//! | Reused scratch | The neighbour hash, the search frontier and the path cache are allocated once and reused |
//!
//! [`NpcStats::us_per_agent`] is the number to watch: at a 4 ms budget, every
//! agent beyond `4000 / us_per_agent` is late. The crate's integration tests
//! measure it for 1000 agents and assert a ceiling.
//!
//! ## The frame
//!
//! ```text
//! maintain_population   spawn/despawn so the target population is resident
//! assign tiers          distance from the crowd centre decides the tier
//! rebuild neighbours    one spatial hash for the whole crowd
//! per agent             decide (if its clock says so), steer, move
//! resolve paths         budgeted A* for whatever asked for a route
//! sync bodies           tier-0 agents own a physics capsule
//! ```
//!
//! ## Example
//!
//! ```no_run
//! use noxel_core::math::Vec3;
//! use noxel_npc::{NpcConfig, NpcContext, NpcSystem};
//! # use noxel_physics::{PhysicsConfig, PhysicsWorld};
//! # use noxel_world::{WorldConfig, WorldGenerator, WorldStreamer};
//! # use std::sync::Arc;
//! # let generator = WorldGenerator::new(WorldConfig::new(7), Arc::new(Default::default()), Vec::new());
//! # let mut streamer = WorldStreamer::new(generator);
//! # let mut physics = PhysicsWorld::new(PhysicsConfig::default());
//! # let camera = Vec3::ZERO;
//! # streamer.update(camera);
//! let mut system = NpcSystem::new(NpcConfig::default());
//! let mut ctx = NpcContext {
//!     streamer: &streamer,
//!     physics: &mut physics,
//!     center: camera,
//!     world_time: 8.0 * 3600.0,
//!     dt: 1.0 / 60.0,
//! };
//! system.maintain_population(&mut ctx);
//! let stats = system.update(&mut ctx);
//! assert!(stats.active > 0);
//! ```
//!
//! ## Determinism
//!
//! Spawn points, kinds, schedules, destinations and path choices are all pure
//! functions of `(seed, index)`: every draw comes from an addressable
//! [`RngStream`](noxel_core::rng::RngStream), the crowd is stored in slot order
//! and never iterated as a hash map, and the A\* frontier breaks ties by node
//! coordinate. Two runs of the same seed produce bit-identical positions, which
//! the integration tests assert over 600 steps.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod agent;
pub mod crowd;
pub mod flow;
pub mod path;
pub mod schedule;
pub mod spawn;
pub mod steering;

use std::time::Instant;

use noxel_core::math::{Aabb, Vec3};
use noxel_core::pool::Handle;
use noxel_core::rng::RngStream;
use noxel_core::spatial::SpatialHash;
use noxel_core::spatial::hash::HashEntry;
use noxel_physics::{BodyDesc, BodyHandle, ColliderShape, LAYER_NPC, LAYER_WORLD, PhysicsWorld};
use noxel_world::road;
use noxel_world::stream::WorldStreamer;

pub use crate::agent::{NpcAgent, NpcId, NpcKind, NpcState, NpcStats};
pub use crate::crowd::{CrowdManager, CrowdTier, TierConfig};
pub use crate::flow::{FlowField, FlowFieldCache};
pub use crate::path::{Path, PathConfig, PathRequest, Pathfinder};
pub use crate::schedule::{Activity, ActivityPlace, DailySchedule, ScheduleConfig, ScheduleEntry};
pub use crate::spawn::{NpcSpawner, SpawnKind};
pub use crate::steering::{Steering, SteeringWeights};

/// The cell size of the crowd's flow fields, in metres.
///
/// Quantising a destination to this grid is what lets nearby agents share one
/// field; it is also the resolution at which a field's directions change.
pub const FLOW_CELL: f32 = 2.0;

/// The edge length of the square region a flow field covers, in metres.
pub const FLOW_EXTENT: f32 = 60.0;

/// How many flow fields the crowd keeps cached.
pub const FLOW_CACHE_CAPACITY: usize = 24;

/// How many agents [`NpcSystem::maintain_population`] may add in one call.
///
/// Spawning validates a world position, so a thousand agents cost a hundred
/// thousand samples; doing that in a single frame is a visible hitch. The
/// budget spreads a fresh town over a few dozen frames, which is invisible, and
/// costs nothing once the town is full.
pub const MAX_SPAWNS_PER_STEP: usize = 64;

/// How many flow fields the crowd may *build* in one step.
///
/// A field costs one search over its region, which is cheap once and expensive
/// a thousand times over: without this budget the first frame of a town builds
/// a field for every distinct destination the crowd picks. Agents whose goal
/// has no field yet fall back to a normal path request, and the town's points
/// of interest acquire their fields over the first second or so.
pub const MAX_FLOW_BUILDS_PER_STEP: usize = 2;

/// Seconds without progress before an agent is considered stuck.
pub const STUCK_TIME: f32 = 3.0;

/// How many times a stuck agent repaths before it gives up and wanders.
pub const MAX_REPAIRS: u8 = 2;

/// How far the crowd centre may move before the local points of interest are
/// recomputed, in metres.
pub const POI_REFRESH_DISTANCE: f32 = 10.0;

/// The largest timestep the crowd will simulate, in seconds.
///
/// A frame hitch must not teleport the town across the map: the crowd clamps
/// `dt` and reports the step it actually took.
pub const MAX_DT: f32 = 0.1;

/// Health at or below which an agent flees instead of walking.
pub const FLEE_HEALTH: f32 = 30.0;

/// How wide the jitter around a destination may be, in metres.
const DESTINATION_JITTER: f32 = 6.0;

/// Configuration for a whole crowd.
///
/// [`NpcConfig::default`] is a town: four hundred residents within seventy
/// metres of the camera, four thousand as a hard ceiling, and the tier and
/// steering defaults the rest of the engine is tuned against.
#[derive(Clone, Debug)]
pub struct NpcConfig {
    /// The world seed. Every spawn, schedule and destination descends from it.
    pub seed: u64,
    /// Hard ceiling on resident agents.
    pub max_agents: usize,
    /// Agents are spawned within this radius of the crowd centre, in metres.
    pub spawn_radius: f32,
    /// Agents further than this from the centre are despawned, in metres.
    pub despawn_radius: f32,
    /// How many agents [`NpcSystem::maintain_population`] keeps resident.
    pub target_population: usize,
    /// The crowd tiers.
    pub tier: TierConfig,
    /// The steering weights.
    pub steering: SteeringWeights,
    /// The pathfinder's tuning.
    pub path: PathConfig,
    /// How the daily schedule is applied.
    pub schedule: ScheduleConfig,
    /// Agent height in metres, used for the physics capsule.
    pub agent_height: f32,
    /// Agent radius in metres, used for the physics capsule and separation.
    pub agent_radius: f32,
    /// Walking speed in metres per second.
    pub walk_speed: f32,
    /// Running speed in metres per second, used when fleeing.
    pub run_speed: f32,
    /// How close counts as arrived, in metres.
    pub arrive_distance: f32,
    /// Seconds between path refreshes for an agent that is still travelling.
    pub repath_interval: f32,
}

impl Default for NpcConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            max_agents: 4096,
            spawn_radius: 70.0,
            despawn_radius: 90.0,
            target_population: 400,
            tier: TierConfig::default(),
            steering: SteeringWeights::default(),
            path: PathConfig::default(),
            schedule: ScheduleConfig::default(),
            agent_height: 1.7,
            agent_radius: 0.35,
            walk_speed: 1.6,
            run_speed: 4.2,
            arrive_distance: 0.6,
            repath_interval: 1.5,
        }
    }
}

impl NpcConfig {
    /// The same configuration with every value inside a usable range.
    ///
    /// A configuration that arrives from a file is not trusted: negative radii,
    /// non-finite speeds and a `max_agents` of zero all have defined, harmless
    /// behaviour after this call.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.max_agents = self.max_agents.min(1 << 20);
        self.spawn_radius = sanitise_radius(self.spawn_radius, 70.0);
        self.despawn_radius = sanitise_radius(self.despawn_radius, 90.0).max(self.spawn_radius);
        self.target_population = self.target_population.min(self.max_agents);
        self.agent_height = sanitise_range(self.agent_height, 1.7, 0.2, 8.0);
        self.agent_radius =
            sanitise_range(self.agent_radius, 0.35, 0.05, 4.0).min(self.agent_height * 0.5);
        self.walk_speed = sanitise_range(self.walk_speed, 1.6, 0.05, 40.0);
        self.run_speed = sanitise_range(self.run_speed, 4.2, self.walk_speed, 60.0);
        self.arrive_distance = sanitise_range(self.arrive_distance, 0.6, 0.05, 10.0);
        self.repath_interval = sanitise_range(self.repath_interval, 1.5, 0.0, 600.0);
        self.tier = self.tier.sanitised();
        self.steering = self.steering.sanitised();
        self.path = self.path.sanitised();
        self.schedule = self.schedule.sanitised();
        self
    }
}

/// A finite radius, falling back to `fallback`.
fn sanitise_radius(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 1.0 {
        value.min(1.0e6)
    } else {
        fallback
    }
}

/// A finite value inside `[min, max]`, falling back to `fallback`.
fn sanitise_range(value: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

/// Everything an update needs.
///
/// The context is built fresh each frame — it borrows the streamer and the
/// physics world — so it is a plain struct with public fields rather than a
/// long-lived resource.
pub struct NpcContext<'a> {
    /// The chunk streamer: where the world is, and what is resident.
    pub streamer: &'a WorldStreamer,
    /// The physics world. Tier-0 agents own a capsule in it.
    pub physics: &'a mut PhysicsWorld,
    /// Where the crowd is centred (usually the camera focus).
    pub center: Vec3,
    /// Seconds since the world started, for the daily schedule.
    pub world_time: f64,
    /// The timestep to simulate, in seconds.
    pub dt: f32,
}

impl<'a> NpcContext<'a> {
    /// A context for one step.
    pub fn new(
        streamer: &'a WorldStreamer,
        physics: &'a mut PhysicsWorld,
        center: Vec3,
        world_time: f64,
        dt: f32,
    ) -> Self {
        Self {
            streamer,
            physics,
            center,
            world_time,
            dt,
        }
    }
}

/// The `Copy` fields of one agent, snapshotted before a step decodes them.
///
/// Holding a reference to an agent while also mutating the system is not
/// possible in safe Rust, so the per-step pass copies the handful of scalars it
/// needs and works from those.
#[derive(Clone, Copy, Debug)]
struct AgentView {
    id: NpcId,
    kind: NpcKind,
    state: NpcState,
    position: Vec3,
    destination: Option<Vec3>,
    tier: CrowdTier,
    health: f32,
    cursor: usize,
    blocked_time: f32,
    decision_timer: f32,
}

/// What one agent decided to do this step, before it is applied.
#[derive(Clone, Copy, Debug)]
struct Decision {
    /// The direction the agent wants to travel, after obstacle avoidance.
    desired: Vec3,
    /// The destination after any retargeting.
    destination: Option<Vec3>,
    /// Whether a path was asked for.
    request: bool,
    /// Whether the current path must be dropped (a new destination, or arrival).
    clear_path: bool,
    /// Whether the agent is standing at its destination.
    arrived: bool,
    /// Whether the agent has just given up on this destination.
    stuck: bool,
    /// Whether the agent's decision clock fired this step.
    due: bool,
    /// The decision timer to store back.
    timer: f32,
    /// Seconds until the next path request.
    repath: f32,
    /// Failed repaths since the last progress.
    repairs: u8,
}

/// The local points of interest the schedule sends agents to.
#[derive(Clone, Copy, Debug)]
struct LocalPois {
    /// Open ground in the middle of the nearest town.
    plaza: Vec3,
    /// Where the market stalls are.
    market: Vec3,
    /// The tavern.
    tavern: Vec3,
    /// A workplace.
    work: Vec3,
    /// The nearest point on a road.
    road: Vec3,
}

/// The whole NPC system: agents, pathfinding, steering and spawn/despawn.
///
/// One system owns one crowd. It is deliberately not generic over the world:
/// the streamer is passed in with every context, which is what lets an
/// application stream a different part of the map without rebuilding the crowd.
pub struct NpcSystem {
    config: NpcConfig,
    crowd: CrowdManager,
    pathfinder: Pathfinder,
    flow: FlowFieldCache,
    spawner: NpcSpawner,
    schedules: [DailySchedule; NpcKind::ALL.len()],
    /// Current path per agent, indexed by [`NpcId`].
    paths: Vec<Path>,
    /// Home anchor per agent: where it sleeps, and where it returns to.
    homes: Vec<Vec3>,
    /// Seconds until the agent may ask for a path again.
    repath: Vec<f32>,
    /// Failed repaths since the agent last made progress.
    repairs: Vec<u8>,
    /// Physics capsule per agent, where the tier calls for one.
    bodies: Vec<Option<BodyHandle>>,
    /// Path requests raised this step, in agent order.
    pending: Vec<(NpcId, Vec3)>,
    /// Requests raised by [`NpcSystem::send_to`], served before the rest.
    urgent: Vec<(NpcId, Vec3)>,
    /// Agents retired this step, so the crowd is not mutated mid-walk.
    doomed: Vec<NpcId>,
    neighbours: SpatialHash<u32>,
    handles: Vec<Handle<HashEntry<u32>>>,
    scratch: Vec<Handle<HashEntry<u32>>>,
    pois: Option<LocalPois>,
    poi_center: Vec3,
    spawn_counter: u64,
    step: u64,
    /// How many slots have per-agent state. Agents spawned through
    /// [`NpcSystem::crowd_mut`] are picked up by the next update.
    initialised: usize,
    /// Flow fields built so far this step, against [`MAX_FLOW_BUILDS_PER_STEP`].
    flow_builds_this_step: usize,
    stats: NpcStats,
}

impl NpcSystem {
    /// A system with no agents, ready to be populated.
    #[must_use]
    pub fn new(config: NpcConfig) -> Self {
        let config = config.sanitised();
        let schedules = build_schedules(&config);
        let cell = (config.steering.neighbour_radius * 2.0).max(1.0);
        Self {
            crowd: CrowdManager::new(config.clone()),
            pathfinder: Pathfinder::new(config.path),
            flow: FlowFieldCache::new(FLOW_CACHE_CAPACITY),
            spawner: NpcSpawner::new(config.clone()),
            schedules,
            paths: Vec::new(),
            homes: Vec::new(),
            repath: Vec::new(),
            repairs: Vec::new(),
            bodies: Vec::new(),
            pending: Vec::new(),
            urgent: Vec::new(),
            doomed: Vec::new(),
            neighbours: SpatialHash::new(cell),
            handles: Vec::new(),
            scratch: Vec::new(),
            pois: None,
            poi_center: Vec3::new(f32::MAX, 0.0, 0.0),
            spawn_counter: 0,
            step: 0,
            initialised: 0,
            flow_builds_this_step: 0,
            stats: NpcStats::default(),
            config,
        }
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> &NpcConfig {
        &self.config
    }

    /// Replaces the configuration, keeping the resident agents.
    ///
    /// Changing the seed re-derives every daily schedule and drops the caches,
    /// because both were functions of the old seed.
    pub fn set_config(&mut self, config: NpcConfig) {
        let config = config.sanitised();
        let seed_changed = config.seed != self.config.seed;
        self.pathfinder.set_config(config.path);
        self.spawner.set_config(config.clone());
        self.crowd.set_config(config.clone());
        if seed_changed {
            self.schedules = build_schedules(&config);
            self.flow.clear();
            self.pathfinder.clear_cache();
            self.pois = None;
            self.poi_center = Vec3::new(f32::MAX, 0.0, 0.0);
        }
        self.config = config;
    }

    /// The crowd.
    #[inline]
    #[must_use]
    pub fn crowd(&self) -> &CrowdManager {
        &self.crowd
    }

    /// The crowd, mutably.
    ///
    /// Spawning or despawning through this handle is supported: the next
    /// [`NpcSystem::update`] reconciles the physics capsules and the per-agent
    /// arrays.
    #[inline]
    #[must_use]
    pub fn crowd_mut(&mut self) -> &mut CrowdManager {
        &mut self.crowd
    }

    /// The pathfinder, for tuning and for its statistics.
    #[inline]
    #[must_use]
    pub fn pathfinder(&self) -> &Pathfinder {
        &self.pathfinder
    }

    /// The pathfinder, mutably.
    #[inline]
    #[must_use]
    pub fn pathfinder_mut(&mut self) -> &mut Pathfinder {
        &mut self.pathfinder
    }

    /// The flow-field cache, for tuning and for its statistics.
    #[inline]
    #[must_use]
    pub fn flow_cache(&self) -> &FlowFieldCache {
        &self.flow
    }

    /// The schedule for one kind of agent.
    #[must_use]
    pub fn schedule(&self, kind: NpcKind) -> &DailySchedule {
        &self.schedules[kind.index()]
    }

    /// Spawns and despawns so that `target_population` agents are resident
    /// within `spawn_radius` of the centre.
    ///
    /// Agents further than `despawn_radius` from the centre — and any whose
    /// position has gone non-finite — are retired first; new agents are then
    /// drawn from the spawner's deterministic stream until the population
    /// reaches the target. Physics capsules are inserted for the tiers that
    /// collide and removed with their agents, so the body count returns to its
    /// starting value after a crowd has come and gone.
    pub fn maintain_population(&mut self, ctx: &mut NpcContext<'_>) {
        let center = sanitise_vec(ctx.center);
        let despawn = self
            .config
            .despawn_radius
            .max(self.config.spawn_radius)
            .max(1.0);

        self.doomed.clear();
        for (index, slot) in self.crowd.slots().iter().enumerate() {
            let Some(agent) = slot else { continue };
            if !agent.position.is_finite() || ground_distance(agent.position, center) > despawn {
                self.doomed.push(NpcId(index as u32));
            }
        }
        let doomed = core::mem::take(&mut self.doomed);
        for id in &doomed {
            self.retire(*id, ctx);
        }
        self.doomed = doomed;
        self.doomed.clear();

        let target = self.config.target_population.min(self.config.max_agents);
        let mut guard = 0usize;
        while self.crowd.len() < target && guard < MAX_SPAWNS_PER_STEP {
            guard += 1;
            let index = self.spawn_counter;
            let Some((kind, position)) = self.spawner.choose_spawn(ctx.streamer, center, index)
            else {
                break;
            };
            self.spawn_counter = self.spawn_counter.saturating_add(1);
            if !self.spawn_agent(kind, position, center, ctx) {
                break;
            }
        }
    }

    /// Spawns one agent and everything that hangs off it. Returns false when the
    /// crowd is full.
    fn spawn_agent(
        &mut self,
        kind: NpcKind,
        position: Vec3,
        center: Vec3,
        ctx: &mut NpcContext<'_>,
    ) -> bool {
        let id = self.crowd.spawn(kind, position);
        if !id.is_valid() {
            return false;
        }
        let index = id.index();
        self.ensure_agent_arrays(index + 1);
        self.paths[index] = Path::empty();
        self.homes[index] = position;
        self.repath[index] = 0.0;
        self.repairs[index] = 0;
        let tier = self.config.tier.tier_for(ground_distance(position, center));
        if let Some(agent) = self.crowd.agent_mut(id) {
            agent.tier = tier;
        }
        if self.config.tier.collides_in(tier) {
            self.insert_body(id, ctx);
        }
        true
    }

    /// Retires one agent and its physics capsule.
    fn retire(&mut self, id: NpcId, ctx: &mut NpcContext<'_>) {
        if let Some(handle) = self.bodies.get(id.index()).copied().flatten() {
            ctx.physics.remove(handle);
            if let Some(slot) = self.bodies.get_mut(id.index()) {
                *slot = None;
            }
        }
        self.crowd.despawn(id);
    }

    /// Inserts the physics capsule for an agent that should own one.
    fn insert_body(&mut self, id: NpcId, ctx: &mut NpcContext<'_>) {
        let Some(agent) = self.crowd.agent(id) else {
            return;
        };
        let position = agent.position;
        let index = id.index();
        if index >= self.bodies.len() {
            return;
        }
        let radius = self.config.agent_radius;
        let height = self.config.agent_height.max(radius * 2.0);
        let half_height = (height * 0.5 - radius).max(0.01);
        let handle = ctx.physics.insert(
            BodyDesc::kinematic(ColliderShape::Capsule {
                radius,
                half_height,
            })
            .at(position + Vec3::Y * (height * 0.5))
            // The capsule blocks the world for the agent, and is visible to
            // anything that masks `LAYER_NPC` (the player, a projectile).
            // NPCs do not sweep against each other: separation is the
            // crowd's job, and a thousand mutually colliding capsules would
            // cost more than it looks.
            .with_layer(LAYER_NPC, LAYER_WORLD)
            .with_user_data(u64::from(id.0)),
        );
        self.bodies[index] = Some(handle);
    }

    /// One simulation step.
    ///
    /// This is the whole frame: population, tiers, decisions, steering, motion,
    /// the budgeted path searches and the physics capsules. The physics world is
    /// *not* stepped here — an application that owns a fixed-step loop calls
    /// [`PhysicsWorld::step`] itself, and the crowd's character moves are
    /// applied on top of whatever the solver did.
    pub fn update(&mut self, ctx: &mut NpcContext<'_>) -> NpcStats {
        let started = Instant::now();
        let dt = sanitise_dt(ctx.dt);
        let center = sanitise_vec(ctx.center);
        let hour = hour_of(ctx.world_time);

        self.crowd.begin_step();
        self.maintain_population(ctx);
        self.sync_external_spawns();
        self.step = self.step.wrapping_add(1);
        self.flow_builds_this_step = 0;
        self.refresh_pois(ctx.streamer, center);
        self.assign_tiers(center);
        self.rebuild_neighbours();
        self.step_agents(ctx, dt, hour, center);
        self.resolve_paths(ctx);
        self.sync_bodies(ctx);
        self.finish(started)
    }

    /// The statistics of the most recent step, merged with the crowd's current
    /// state and the pathfinder's cumulative counters.
    #[must_use]
    pub fn stats(&self) -> NpcStats {
        let mut stats = self.crowd.stats();
        let paths = self.pathfinder.stats();
        stats.paths_computed = paths.paths_computed;
        stats.path_nodes_expanded = paths.path_nodes_expanded;
        stats.flow_fields_cached = self.flow.len();
        stats.last_step_ms = self.stats.last_step_ms;
        stats.mean_step_ms = self.stats.mean_step_ms;
        stats.us_per_agent = if stats.active == 0 {
            0.0
        } else {
            stats.last_step_ms * 1000.0 / stats.active as f32
        };
        stats
    }

    /// Approximate heap footprint of the whole system, in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let paths: usize = self.paths.iter().map(Path::memory_bytes).sum();
        self.crowd.memory_bytes()
            + paths
            + self.homes.capacity() * core::mem::size_of::<Vec3>()
            + self.bodies.capacity() * core::mem::size_of::<Option<BodyHandle>>()
            + self.neighbours.memory_bytes()
            + self.handles.capacity() * core::mem::size_of::<Handle<HashEntry<u32>>>()
            + self.pathfinder.memory_bytes()
            + self.flow.memory_bytes()
            + self
                .schedules
                .iter()
                .map(DailySchedule::memory_bytes)
                .sum::<usize>()
            + core::mem::size_of::<Self>()
    }

    /// Assigns a destination and requests a path, returning `false` when the
    /// request could not be made.
    ///
    /// The path itself is computed on the next [`NpcSystem::update`], because
    /// only an update holds the streamer: a request that names a resident agent
    /// and a finite destination is accepted here and served within the step's
    /// path budget, ahead of the crowd's own requests. `false` means the id is
    /// not resident, or the destination is not finite.
    pub fn send_to(&mut self, id: NpcId, destination: Vec3) -> bool {
        if !destination.is_finite() || self.crowd.agent(id).is_none() {
            return false;
        }
        if let Some(agent) = self.crowd.agent_mut(id) {
            agent.destination = Some(destination);
            agent.path_cursor = 0;
        }
        let index = id.index();
        if index < self.paths.len() {
            self.paths[index] = Path::empty();
        }
        if index < self.repath.len() {
            self.repath[index] = 0.0;
        }
        if index < self.repairs.len() {
            self.repairs[index] = 0;
        }
        self.urgent.push((id, destination));
        true
    }

    /// Gives per-agent state to anything that appeared through
    /// [`NpcSystem::crowd_mut`] rather than through the spawner.
    ///
    /// A game is allowed to spawn into the crowd directly — a summon, a loaded
    /// save, a scripted arrival — and the system's parallel arrays must catch up
    /// before the next step reads them.
    fn sync_external_spawns(&mut self) {
        let slots = self.crowd.slot_count();
        if slots > self.initialised {
            self.ensure_agent_arrays(slots);
            for index in self.initialised..slots {
                let Some(agent) = self.crowd.slots().get(index).and_then(Option::as_ref) else {
                    continue;
                };
                let position = agent.position;
                self.paths[index] = Path::empty();
                self.homes[index] = position;
                self.repath[index] = 0.0;
                self.repairs[index] = 0;
            }
            self.initialised = slots;
        }
    }

    /// Grows every per-agent array to at least `len`.
    fn ensure_agent_arrays(&mut self, len: usize) {
        if self.paths.len() < len {
            self.paths.resize_with(len, Path::empty);
            self.homes.resize(len, Vec3::ZERO);
            self.repath.resize(len, 0.0);
            self.repairs.resize(len, 0);
            self.bodies.resize(len, None);
        }
    }

    /// Writes each agent's tier from its distance to the crowd centre.
    fn assign_tiers(&mut self, center: Vec3) {
        let tier_config = self.config.tier;
        for slot in self.crowd.slots_mut() {
            let Some(agent) = slot else { continue };
            if !agent.position.is_finite() {
                continue;
            }
            agent.tier = tier_config.tier_for(ground_distance(agent.position, center));
        }
    }

    /// Rebuilds the crowd's neighbour hash, reusing the previous allocation.
    fn rebuild_neighbours(&mut self) {
        let cell = (self.config.steering.neighbour_radius * 2.0).max(1.0);
        steering::fill_neighbours(&mut self.neighbours, &self.crowd, cell);
    }

    /// Recomputes the local points of interest when the centre has moved far
    /// enough to make them stale.
    fn refresh_pois(&mut self, streamer: &WorldStreamer, center: Vec3) {
        let stale =
            self.pois.is_none() || ground_distance(self.poi_center, center) > POI_REFRESH_DISTANCE;
        if !stale {
            return;
        }
        let Some(plaza) = nearest_walkable(streamer, center, 48.0) else {
            return;
        };
        let market =
            nearest_walkable(streamer, plaza + Vec3::new(12.0, 0.0, 0.0), 24.0).unwrap_or(plaza);
        let tavern =
            nearest_walkable(streamer, plaza + Vec3::new(-12.0, 0.0, 0.0), 24.0).unwrap_or(plaza);
        let work =
            nearest_walkable(streamer, plaza + Vec3::new(0.0, 0.0, 14.0), 24.0).unwrap_or(plaza);
        let road = nearest_road(streamer, center).unwrap_or(plaza);
        self.pois = Some(LocalPois {
            plaza,
            market,
            tavern,
            work,
            road,
        });
        self.poi_center = center;
    }

    /// One pass over the crowd: decide, steer and move.
    ///
    /// Each agent is handled in three stages — read, decide, apply — because the
    /// steering force needs to read the whole crowd while the agent being moved
    /// is borrowed from it, and Rust will not allow both at once. Snapshotting
    /// the agent's `Copy` fields first and applying the frame's numbers last is
    /// the safe way to write it, and it costs one small struct per agent.
    fn step_agents(&mut self, ctx: &mut NpcContext<'_>, dt: f32, hour: f32, center: Vec3) {
        let count = self.crowd.slot_count();
        for index in 0..count {
            let Some(view) = self.agent_view(index) else {
                continue;
            };
            let decision = self.decide(index, view, dt, hour, center, ctx.streamer);
            let force = self.compute_force(index, decision.desired, ctx.streamer);
            self.apply_step(index, view, decision, force, dt, hour, ctx);
        }
    }

    /// Copies the per-step inputs of one agent out of the crowd.
    fn agent_view(&self, index: usize) -> Option<AgentView> {
        self.crowd
            .slots()
            .get(index)
            .and_then(Option::as_ref)
            .map(|agent| AgentView {
                id: agent.id,
                kind: agent.kind,
                state: agent.state,
                position: agent.position,
                destination: agent.destination,
                tier: agent.tier,
                health: agent.health,
                cursor: agent.path_cursor,
                blocked_time: agent.blocked_time,
                decision_timer: agent.decision_timer,
            })
    }

    /// Decides what one agent wants to do this step, without touching the crowd.
    fn decide(
        &mut self,
        index: usize,
        view: AgentView,
        dt: f32,
        hour: f32,
        center: Vec3,
        streamer: &WorldStreamer,
    ) -> Decision {
        let tier_config = self.config.tier;
        let arrive_distance = self.config.arrive_distance;
        let interval = tier_config.interval_of(view.tier);
        let mut timer = view.decision_timer - dt;
        let due = interval <= 0.0 || timer <= 0.0;
        if due {
            timer = if interval > 0.0 { interval } else { 0.0 };
        }
        let mut decision = Decision {
            desired: Vec3::ZERO,
            destination: view.destination,
            request: false,
            clear_path: false,
            arrived: false,
            stuck: false,
            due,
            timer,
            repath: (self.repath[index] - dt).max(0.0),
            repairs: self.repairs[index],
        };
        let activity = if self.config.schedule.enabled {
            self.schedules[view.kind.index()]
                .activity_at(hour + self.config.schedule.retarget_margin)
        } else {
            Activity::Wander
        };

        if due && view.position.is_finite() {
            let arrived = decision
                .destination
                .is_some_and(|d| ground_distance(view.position, d) <= arrive_distance);
            if decision.destination.is_none() || arrived {
                decision.destination =
                    self.choose_destination(index, activity, view.position, center, streamer);
                decision.clear_path = true;
                decision.repairs = 0;
                // An agent whose goal already has a flow field around it needs
                // no route of its own: the field answers "which way" every
                // step, which is the whole point of sharing one search.
                decision.request = decision.destination.is_some_and(|goal| {
                    !self
                        .flow
                        .find(goal, FLOW_CELL)
                        .is_some_and(|field| field.contains(view.position))
                });
            } else if view.blocked_time > STUCK_TIME {
                if decision.repairs < MAX_REPAIRS {
                    // First and second failure: repath to the same place.
                    decision.repairs = decision.repairs.saturating_add(1);
                    decision.request = true;
                } else {
                    // Third failure: give up on this destination and pick a
                    // nearby one the agent can actually reach.
                    decision.repairs = 0;
                    decision.stuck = true;
                    decision.destination = self
                        .nearby_destination(index, view.position, streamer)
                        .or(decision.destination);
                    decision.clear_path = true;
                    decision.request = decision.destination.is_some();
                }
            } else if decision.repath <= 0.0
                && self.paths[index].next_waypoint(view.cursor).is_none()
            {
                decision.request = decision.destination.is_some();
            }
        }

        // Where the agent wants to go this step.
        if let Some(waypoint) = self.paths[index].next_waypoint(view.cursor) {
            decision.desired = flatten(waypoint - view.position).normalize_or_zero();
        } else if let Some(goal) = decision.destination {
            if ground_distance(view.position, goal) <= arrive_distance {
                decision.arrived = true;
                decision.clear_path = true;
            } else {
                decision.desired = match self.flow.find(goal, FLOW_CELL) {
                    Some(field) => field
                        .direction_at(view.position)
                        .unwrap_or_else(|| flatten(goal - view.position).normalize_or_zero()),
                    None => flatten(goal - view.position).normalize_or_zero(),
                };
            }
        }

        // Obstacle avoidance, for the tiers that steer.
        if tier_config.uses_full_steering(view.tier) && decision.desired.length_squared() > 0.0 {
            let speed = self.speed_of(view);
            if let Some(agent) = self.crowd.slots().get(index).and_then(Option::as_ref) {
                let probe = 1.2 + speed * 0.35;
                decision.desired = Steering::avoid(agent, decision.desired, streamer, probe);
            }
        }
        decision
    }

    /// The steering force for one agent, in metres per second squared.
    fn compute_force(&mut self, index: usize, desired: Vec3, streamer: &WorldStreamer) -> Vec3 {
        let weights = self.config.steering;
        let tier_config = self.config.tier;
        let Some(agent) = self.crowd.slots().get(index).and_then(Option::as_ref) else {
            return Vec3::ZERO;
        };
        if !tier_config.uses_full_steering(agent.tier) {
            return desired * weights.seek;
        }
        let position = agent.position;
        let radius = weights.neighbour_radius.max(0.01);
        let query = Aabb::new(
            position - Vec3::new(radius, 1.0, radius),
            position + Vec3::new(radius, 1.0, radius),
        );
        self.neighbours
            .query_aabb_into(query, &mut self.handles, &mut self.scratch);
        let mut force = steering::force_from_handles(
            agent,
            desired,
            &self.handles,
            &self.neighbours,
            &self.crowd,
            &weights,
        );
        if desired.length_squared() > 0.0 {
            force += desired * weights.obstacle;
            force += road_bias(streamer, position) * weights.road_bias;
        }
        let force = steering::clamp_length(force, weights.max_force);
        if force.is_finite() { force } else { Vec3::ZERO }
    }

    #[allow(clippy::too_many_arguments)]
    /// Writes one agent's step back into the crowd and moves it.
    fn apply_step(
        &mut self,
        index: usize,
        view: AgentView,
        decision: Decision,
        force: Vec3,
        dt: f32,
        hour: f32,
        ctx: &mut NpcContext<'_>,
    ) {
        let tier_config = self.config.tier;
        let interval = tier_config.interval_of(view.tier);
        let speed = self.speed_of(view);
        // Coarse tiers move in one jump per decision, which keeps their average
        // speed right while touching them a quarter as often. A zero-length
        // step moves nobody, whatever their tier: `dt = 0` is a legal frame
        // (a paused game, a netcode catch-up) and must be a no-op.
        let move_dt = if dt <= 0.0 {
            0.0
        } else if interval <= 0.0 {
            dt
        } else if decision.due {
            interval
        } else {
            0.0
        };

        if decision.clear_path
            && let Some(agent) = self
                .crowd
                .slots_mut()
                .get_mut(index)
                .and_then(Option::as_mut)
            && self.paths[index].next_waypoint(agent.path_cursor).is_some()
        {
            self.paths[index] = Path::empty();
            agent.path_cursor = 0;
        }
        if move_dt > 0.0 && view.position.is_finite() {
            if tier_config.collides_in(view.tier) {
                let height = self.config.agent_height;
                self.move_with_physics(index, ctx, force, speed, move_dt, height);
            } else if let Some(agent) = self
                .crowd
                .slots_mut()
                .get_mut(index)
                .and_then(Option::as_mut)
            {
                let max_slope = self.pathfinder.config().max_slope;
                move_direct(
                    agent,
                    decision.desired,
                    speed,
                    move_dt,
                    ctx.streamer,
                    max_slope,
                );
            }
        }

        let Some(agent) = self
            .crowd
            .slots_mut()
            .get_mut(index)
            .and_then(Option::as_mut)
        else {
            return;
        };
        agent.decision_timer = decision.timer;
        if decision.due {
            agent.destination = decision.destination;
        }
        self.repairs[index] = decision.repairs;
        if decision.request
            && let Some(goal) = decision.destination
        {
            self.repath[index] = self.config.repath_interval;
            self.pending.push((view.id, goal));
        } else {
            self.repath[index] = decision.repath;
        }

        let moved = if agent.position.is_finite() {
            ground_distance(view.position, agent.position)
        } else {
            agent.position = view.position;
            0.0
        };
        let wanted = decision.desired.length_squared() > 0.0;
        let progress = speed * move_dt * 0.25 + 1e-4;
        if move_dt > 0.0 && wanted {
            if moved < progress {
                agent.blocked_time += move_dt;
                agent.blocked = true;
            } else {
                agent.blocked_time = 0.0;
                agent.blocked = false;
            }
        } else if move_dt > 0.0 {
            agent.blocked = false;
            agent.blocked_time = 0.0;
        }
        if moved > progress {
            agent.face(agent.velocity);
        }
        let activity = if self.config.schedule.enabled {
            self.schedules[view.kind.index()]
                .activity_at(hour + self.config.schedule.retarget_margin)
        } else {
            Activity::Wander
        };
        agent.state = if decision.stuck {
            NpcState::Stuck
        } else {
            next_state(
                view.state,
                view.health,
                moved > progress,
                decision.arrived,
                activity,
            )
        };
    }

    /// The agent's speed for this step, in metres per second.
    fn speed_of(&self, view: AgentView) -> f32 {
        let base = if view.health <= FLEE_HEALTH {
            self.config.run_speed
        } else {
            self.config.walk_speed
        };
        base * view.kind.speed_scale()
    }

    /// Moves a tier-0 agent with the character controller.
    fn move_with_physics(
        &mut self,
        index: usize,
        ctx: &mut NpcContext<'_>,
        force: Vec3,
        speed: f32,
        dt: f32,
        height: f32,
    ) {
        let Some(handle) = self.bodies.get(index).copied().flatten() else {
            return;
        };
        let Some(agent) = self
            .crowd
            .slots_mut()
            .get_mut(index)
            .and_then(Option::as_mut)
        else {
            return;
        };
        agent.velocity += force * dt;
        let current = agent.velocity.length_xz();
        if current > speed && current > 0.0 {
            agent.velocity *= speed / current;
        }
        if force.length_squared() <= 0.0 {
            let damping = (1.0 - 6.0 * dt).max(0.0);
            agent.velocity *= damping;
        }
        let motion = Vec3::new(agent.velocity.x * dt, 0.0, agent.velocity.z * dt);
        let moved = ctx.physics.move_character(handle, motion, Vec3::Y);
        if let Some(body) = ctx.physics.body(handle) {
            agent.position = body.position - Vec3::Y * (height * 0.5);
        }
        agent.velocity = if dt > 0.0 {
            moved.translation / dt
        } else {
            Vec3::ZERO
        };
        if !agent.velocity.is_finite() {
            agent.velocity = Vec3::ZERO;
        }
    }

    /// Serves the queued path requests, inside the step's budget.
    fn resolve_paths(&mut self, ctx: &mut NpcContext<'_>) {
        let urgent = core::mem::take(&mut self.urgent);
        let pending = core::mem::take(&mut self.pending);
        if !urgent.is_empty() || !pending.is_empty() {
            let before = self.pathfinder.stats().path_nodes_expanded;
            let mut searches = 0usize;
            let mut used = 0u64;
            for (id, destination) in urgent.iter().chain(pending.iter()) {
                if searches >= path::MAX_PATHS_PER_STEP || used >= path::MAX_NODES_PER_STEP {
                    break;
                }
                let Some(agent) = self.crowd.agent(*id) else {
                    continue;
                };
                let from = agent.position;
                if !from.is_finite() {
                    continue;
                }
                let found = self.pathfinder.find_path(ctx.streamer, from, *destination);
                searches += 1;
                used = self
                    .pathfinder
                    .stats()
                    .path_nodes_expanded
                    .saturating_sub(before);
                let index = id.index();
                if index < self.paths.len() {
                    self.paths[index] = found.unwrap_or_else(Path::empty);
                }
                if let Some(agent) = self.crowd.agent_mut(*id) {
                    agent.path_cursor = 0;
                }
                if index < self.repath.len() {
                    self.repath[index] = self.config.repath_interval;
                }
            }
        }
        self.urgent = urgent;
        self.urgent.clear();
        self.pending = pending;
        self.pending.clear();
    }

    /// Inserts and removes physics capsules so that exactly the colliding tiers
    /// own one.
    fn sync_bodies(&mut self, ctx: &mut NpcContext<'_>) {
        let tier_config = self.config.tier;
        for index in 0..self.crowd.slot_count() {
            let wanted = self
                .crowd
                .slots()
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|agent| {
                    tier_config.collides_in(agent.tier) && agent.position.is_finite()
                });
            let present = self.bodies.get(index).copied().flatten();
            match (wanted, present) {
                (true, None) => self.insert_body(NpcId(index as u32), ctx),
                (false, Some(handle)) => {
                    ctx.physics.remove(handle);
                    if let Some(slot) = self.bodies.get_mut(index) {
                        *slot = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Picks the next destination for an agent, from its schedule.
    ///
    /// Shared places (work, market, plaza, tavern, road) are quantised to the
    /// flow-field grid so that a group heading the same way shares one field;
    /// home and "anywhere" get a wider jitter so the crowd does not stack up on
    /// a single point.
    fn choose_destination(
        &mut self,
        index: usize,
        activity: Activity,
        position: Vec3,
        center: Vec3,
        streamer: &WorldStreamer,
    ) -> Option<Vec3> {
        let some_pois = self.pois;
        let place = activity.place();
        let shared = matches!(
            place,
            ActivityPlace::Work
                | ActivityPlace::Plaza
                | ActivityPlace::Market
                | ActivityPlace::Tavern
                | ActivityPlace::Road
        );
        let base = match (place, some_pois) {
            (ActivityPlace::Home, _) => self.homes.get(index).copied().unwrap_or(position),
            (_, Some(pois)) => match place {
                ActivityPlace::Work => pois.work,
                ActivityPlace::Plaza => pois.plaza,
                ActivityPlace::Market => pois.market,
                ActivityPlace::Tavern => pois.tavern,
                ActivityPlace::Road => pois.road,
                ActivityPlace::Home | ActivityPlace::Anywhere => pois.plaza,
            },
            (_, None) => position,
        };
        let seed = streamer.generator().config().seed;
        let salt = place_salt(place);
        let step_salt = (self.step % 1_000_000) as i32;
        let mut rng = RngStream::for_grid3(seed, "npc/dest", index as i32, step_salt, salt).rng();
        // Shared goals are quantised to *two* flow cells: every agent in the
        // same two-cell bucket shares one field, and the crowd does not need a
        // field per centimetre of jitter.
        let spread = if shared { FLOW_CELL * 2.0 } else { 1.0 };
        let x = (rng.range_f32(-DESTINATION_JITTER, DESTINATION_JITTER) / spread).round() * spread;
        let z = (rng.range_f32(-DESTINATION_JITTER, DESTINATION_JITTER) / spread).round() * spread;
        let limit = self.config.spawn_radius * 0.85;
        let mut candidate = clamp_to_center(base + Vec3::new(x, 0.0, z), center, limit);
        // Three tries at a standable point, then take what we have: the
        // pathfinder will clamp or refuse, and the stuck detector cleans up.
        for attempt in 0..3 {
            if let Some(point) = walkable_ground(streamer, candidate) {
                candidate = point;
                if path::is_traversable(streamer, self.pathfinder.config(), candidate) {
                    break;
                }
            }
            let mut retry = RngStream::for_grid3(
                seed,
                "npc/dest/retry",
                index as i32,
                step_salt,
                salt + attempt,
            )
            .rng();
            candidate = clamp_to_center(
                base + Vec3::new(
                    retry.range_f32(-DESTINATION_JITTER * 2.0, DESTINATION_JITTER * 2.0),
                    0.0,
                    retry.range_f32(-DESTINATION_JITTER * 2.0, DESTINATION_JITTER * 2.0),
                ),
                center,
                limit,
            );
        }
        // A shared destination is worth a flow field: one search then serves
        // every agent heading there, and the per-step lookup is a hash hit. The
        // per-step budget keeps the first frame of a town from building one
        // field per agent.
        if shared
            && self.flow.find(candidate, FLOW_CELL).is_none()
            && self.flow_builds_this_step < MAX_FLOW_BUILDS_PER_STEP
        {
            let before = self.flow.stats().1;
            let _ = self
                .flow
                .get_or_build(streamer, candidate, FLOW_EXTENT, FLOW_CELL);
            if self.flow.stats().1 > before {
                self.flow_builds_this_step += 1;
            }
        }
        Some(candidate)
    }

    /// A nearby, reachable destination for an agent that cannot get where it
    /// was going.
    fn nearby_destination(
        &mut self,
        index: usize,
        position: Vec3,
        streamer: &WorldStreamer,
    ) -> Option<Vec3> {
        let seed = streamer.generator().config().seed;
        let mut rng = RngStream::for_grid3(
            seed,
            "npc/stuck",
            index as i32,
            (self.step % 1_000_000) as i32,
            0,
        )
        .rng();
        for _ in 0..8 {
            let angle = rng.range_f32(0.0, core::f32::consts::TAU);
            let distance = rng.range_f32(2.0, 12.0);
            let candidate =
                position + Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance);
            if let Some(point) = walkable_ground(streamer, candidate) {
                return Some(point);
            }
        }
        None
    }

    /// Records the step's statistics.
    fn finish(&mut self, started: Instant) -> NpcStats {
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        let mut stats = self.crowd.stats();
        let paths = self.pathfinder.stats();
        stats.paths_computed = paths.paths_computed;
        stats.path_nodes_expanded = paths.path_nodes_expanded;
        stats.flow_fields_cached = self.flow.len();
        stats.record_step(ms, stats.active);
        self.stats = stats;
        stats
    }
}

impl core::fmt::Debug for NpcSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NpcSystem")
            .field("config", &self.config)
            .field("crowd", &self.crowd)
            .field("step", &self.step)
            .finish()
    }
}

/// Builds one schedule per agent kind from the configuration's seed.
fn build_schedules(config: &NpcConfig) -> [DailySchedule; NpcKind::ALL.len()] {
    core::array::from_fn(|i| DailySchedule::for_kind(NpcKind::ALL[i], config.seed))
}

/// A stable salt per activity place, for destination jitter.
fn place_salt(place: ActivityPlace) -> i32 {
    match place {
        ActivityPlace::Home => 1,
        ActivityPlace::Work => 2,
        ActivityPlace::Plaza => 3,
        ActivityPlace::Market => 4,
        ActivityPlace::Tavern => 5,
        ActivityPlace::Road => 6,
        ActivityPlace::Anywhere => 7,
    }
}

/// The next behaviour state, from what actually happened this step.
fn next_state(
    previous: NpcState,
    health: f32,
    moving: bool,
    arrived: bool,
    activity: Activity,
) -> NpcState {
    if previous == NpcState::Stuck {
        // A stuck agent stays stuck until it makes progress again.
        return if moving {
            NpcState::Walking
        } else {
            NpcState::Stuck
        };
    }
    if moving {
        return if health <= FLEE_HEALTH {
            NpcState::Fleeing
        } else {
            NpcState::Walking
        };
    }
    if arrived {
        return arrived_state(activity);
    }
    NpcState::Idle
}

/// The state an agent shows while it is standing at its destination.
fn arrived_state(activity: Activity) -> NpcState {
    match activity {
        Activity::Sleep => NpcState::Sleeping,
        Activity::Work | Activity::Shop | Activity::Patrol => NpcState::Working,
        Activity::Eat | Activity::Socialise => NpcState::Talking,
        Activity::Commute | Activity::Wander | Activity::Idle | Activity::GoHome => NpcState::Idle,
    }
}

/// Moves a non-colliding agent along `desired`, sliding along blocked axes.
///
/// A tier-1+ agent has no capsule, so it integrates its own position. The move
/// is validated against the streamer first — an agent may not walk into water,
/// a wall or a chunk that is not resident — and a blocked move slides along X
/// or Z before giving up, which is what stops the mid and far rings from
/// sticking on corners the near ring would have walked around.
fn move_direct(
    agent: &mut NpcAgent,
    desired: Vec3,
    speed: f32,
    dt: f32,
    streamer: &WorldStreamer,
    max_slope: f32,
) {
    let step = flatten(desired).normalize_or_zero() * (speed * dt);
    if step.length_squared() <= 0.0 || dt <= 0.0 {
        agent.velocity *= (1.0 - 6.0 * dt).max(0.0);
        return;
    }
    let from = agent.position;
    let target = from + step;
    if step_ok(streamer, from, target, max_slope) {
        agent.position = target;
        agent.velocity = step / dt;
        return;
    }
    let along_x = Vec3::new(step.x, 0.0, 0.0);
    if along_x.length_squared() > 1e-8 && step_ok(streamer, from, from + along_x, max_slope) {
        agent.position = from + along_x;
        agent.velocity = along_x / dt;
        return;
    }
    let along_z = Vec3::new(0.0, 0.0, step.z);
    if along_z.length_squared() > 1e-8 && step_ok(streamer, from, from + along_z, max_slope) {
        agent.position = from + along_z;
        agent.velocity = along_z / dt;
        return;
    }
    agent.velocity = Vec3::ZERO;
}

/// True when an agent may walk from `from` to `to`.
fn step_ok(streamer: &WorldStreamer, from: Vec3, to: Vec3, max_slope: f32) -> bool {
    if !to.is_finite() || streamer.chunk_at(to).is_none() {
        return false;
    }
    if !streamer.is_walkable(to) {
        return false;
    }
    let distance = ground_distance(from, to);
    if distance <= 1e-4 {
        return true;
    }
    let rise = (streamer.height_at(to) - streamer.height_at(from)).abs();
    rise / distance <= max_slope
}

/// The standable ground position at `p`, if the streamer holds one.
fn walkable_ground(streamer: &WorldStreamer, p: Vec3) -> Option<Vec3> {
    if !p.is_finite() || streamer.chunk_at(p).is_none() {
        return None;
    }
    let ground = Vec3::new(p.x, streamer.height_at(p), p.z);
    if ground.is_finite() && streamer.is_walkable(ground) {
        Some(ground)
    } else {
        None
    }
}

/// The nearest standable point to `center` within `radius`, on a golden-angle
/// spiral so the answer is stable across runs.
fn nearest_walkable(streamer: &WorldStreamer, center: Vec3, radius: f32) -> Option<Vec3> {
    if let Some(point) = walkable_ground(streamer, center) {
        return Some(point);
    }
    let steps = 48;
    for step in 0..steps {
        let t = step as f32 / steps as f32;
        let distance = radius * (0.1 + 0.9 * t);
        let angle = step as f32 * 2.399_963;
        let candidate = Vec3::new(
            center.x + angle.cos() * distance,
            0.0,
            center.z + angle.sin() * distance,
        );
        if let Some(point) = walkable_ground(streamer, candidate) {
            return Some(point);
        }
    }
    None
}

/// The nearest standable point on a road, if there is one nearby.
fn nearest_road(streamer: &WorldStreamer, center: Vec3) -> Option<Vec3> {
    let config = streamer.generator().config();
    if config.road_grid_chunks <= 0 {
        return None;
    }
    let x_index = road::nearest_line_index(config, center.x);
    let z_index = road::nearest_line_index(config, center.z);
    let line_x = road::macro_line_x(config, x_index);
    let line_z = road::macro_line_z(config, z_index);
    let vertical = Vec3::new(line_x, 0.0, center.z);
    let horizontal = Vec3::new(center.x, 0.0, line_z);
    let first = if (line_x - center.x).abs() <= (line_z - center.z).abs() {
        vertical
    } else {
        horizontal
    };
    nearest_walkable(streamer, first, 24.0)
        .or_else(|| nearest_walkable(streamer, horizontal, 24.0))
        .or_else(|| nearest_walkable(streamer, vertical, 24.0))
}

/// A unit pull towards the nearest macro road, or zero when already on one.
fn road_bias(streamer: &WorldStreamer, position: Vec3) -> Vec3 {
    let config = streamer.generator().config();
    if config.road_grid_chunks <= 0 || !position.is_finite() {
        return Vec3::ZERO;
    }
    let line_x = road::macro_line_x(config, road::nearest_line_index(config, position.x));
    let line_z = road::macro_line_z(config, road::nearest_line_index(config, position.z));
    let dx = line_x - position.x;
    let dz = line_z - position.z;
    let (offset, direction) = if dx.abs() <= dz.abs() {
        (dx.abs(), Vec3::new(dx, 0.0, 0.0))
    } else {
        (dz.abs(), Vec3::new(0.0, 0.0, dz))
    };
    if offset <= 3.0 {
        // Close enough: the path already prefers the pavement, and a bias here
        // would only make the crowd hug the kerb.
        return Vec3::ZERO;
    }
    direction.normalize_or_zero()
}

/// Pulls `p` inside `radius` of `center` on the ground plane.
fn clamp_to_center(p: Vec3, center: Vec3, radius: f32) -> Vec3 {
    if !p.is_finite() || !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return p;
    }
    let delta = Vec3::new(p.x - center.x, 0.0, p.z - center.z);
    let distance = delta.length();
    if distance <= radius || distance <= 1e-4 {
        return p;
    }
    center + delta * (radius / distance)
}

/// Flattens a vector onto the ground plane.
#[inline]
fn flatten(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Ground-plane distance between two positions.
#[inline]
fn ground_distance(a: Vec3, b: Vec3) -> f32 {
    if !a.is_finite() || !b.is_finite() {
        return f32::MAX;
    }
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    (dx * dx + dz * dz).sqrt()
}

/// Clamps a timestep into `[0, MAX_DT]`.
fn sanitise_dt(dt: f32) -> f32 {
    if dt.is_finite() {
        dt.clamp(0.0, MAX_DT)
    } else {
        0.0
    }
}

/// Replaces a non-finite position with the origin.
fn sanitise_vec(v: Vec3) -> Vec3 {
    if v.is_finite() { v } else { Vec3::ZERO }
}

/// The hour of the day for a world time in seconds.
#[must_use]
pub fn hour_of(world_time: f64) -> f32 {
    if !world_time.is_finite() {
        return 0.0;
    }
    let hours = (world_time / 3600.0) % 24.0;
    if hours < 0.0 {
        (hours + 24.0) as f32
    } else {
        hours as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_town_sized() {
        let config = NpcConfig::default();
        assert_eq!(config.seed, 0);
        assert_eq!(config.max_agents, 4096);
        assert_eq!(config.spawn_radius, 70.0);
        assert_eq!(config.despawn_radius, 90.0);
        assert_eq!(config.target_population, 400);
        assert_eq!(config.agent_height, 1.7);
        assert_eq!(config.agent_radius, 0.35);
        assert_eq!(config.walk_speed, 1.6);
        assert_eq!(config.run_speed, 4.2);
        assert_eq!(config.arrive_distance, 0.6);
        assert_eq!(config.repath_interval, 1.5);
        assert_eq!(config.tier.near, 25.0);
        assert_eq!(config.tier.mid, 60.0);
        assert_eq!(config.tier.far, 90.0);
        assert_eq!(config.path.node_size, 2.0);
        assert_eq!(config.steering.max_force, 12.0);
    }

    #[test]
    fn config_sanitiser_repairs_nonsense() {
        let broken = NpcConfig {
            max_agents: 0,
            spawn_radius: f32::NAN,
            despawn_radius: -4.0,
            target_population: 10_000,
            agent_height: 0.0,
            agent_radius: 99.0,
            walk_speed: f32::INFINITY,
            run_speed: 0.0,
            arrive_distance: -1.0,
            repath_interval: f32::NAN,
            ..NpcConfig::default()
        }
        .sanitised();
        assert_eq!(broken.max_agents, 0);
        assert_eq!(broken.target_population, 0);
        assert!(broken.spawn_radius.is_finite() && broken.spawn_radius > 0.0);
        assert!(broken.despawn_radius >= broken.spawn_radius);
        assert!(broken.agent_height > 0.0);
        assert!(broken.agent_radius <= broken.agent_height * 0.5);
        assert!(broken.walk_speed.is_finite() && broken.walk_speed > 0.0);
        assert!(broken.run_speed >= broken.walk_speed);
        assert!(broken.arrive_distance > 0.0);
        assert!(broken.repath_interval.is_finite());
    }

    #[test]
    fn hours_wrap_and_survive_junk() {
        assert_eq!(hour_of(0.0), 0.0);
        assert!((hour_of(3600.0 * 8.5) - 8.5).abs() < 1e-4);
        assert!((hour_of(3600.0 * 25.0) - 1.0).abs() < 1e-4);
        assert!(hour_of(-1.0) > 23.9, "a second before midnight");
        assert_eq!(hour_of(f64::NAN), 0.0);
        assert_eq!(hour_of(f64::INFINITY), 0.0);
        assert!((hour_of(-3600.0) - 23.0).abs() < 1e-4);
    }

    #[test]
    fn sanitise_helpers_are_total() {
        assert_eq!(sanitise_dt(f32::NAN), 0.0);
        assert_eq!(sanitise_dt(-1.0), 0.0);
        assert_eq!(sanitise_dt(10.0), MAX_DT);
        assert_eq!(sanitise_vec(Vec3::new(f32::NAN, 1.0, 0.0)), Vec3::ZERO);
        assert_eq!(flatten(Vec3::new(1.0, 9.0, 2.0)), Vec3::new(1.0, 0.0, 2.0));
        assert_eq!(ground_distance(Vec3::ZERO, Vec3::ZERO), 0.0);
        assert_eq!(
            ground_distance(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO),
            f32::MAX
        );
        let clamped = clamp_to_center(Vec3::new(100.0, 0.0, 0.0), Vec3::ZERO, 10.0);
        assert!((clamped.x - 10.0).abs() < 1e-4);
        assert!(
            clamp_to_center(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO, 10.0)
                .x
                .is_nan()
        );
        assert_eq!(clamp_to_center(Vec3::X, Vec3::ZERO, 0.0), Vec3::X);
    }

    #[test]
    fn state_machine_covers_every_branch() {
        assert_eq!(
            next_state(NpcState::Idle, 100.0, true, false, Activity::Work),
            NpcState::Walking
        );
        assert_eq!(
            next_state(NpcState::Idle, 10.0, true, false, Activity::Work),
            NpcState::Fleeing
        );
        assert_eq!(
            next_state(NpcState::Walking, 100.0, false, true, Activity::Sleep),
            NpcState::Sleeping
        );
        assert_eq!(
            next_state(NpcState::Walking, 100.0, false, false, Activity::Work),
            NpcState::Idle
        );
        assert_eq!(
            next_state(NpcState::Stuck, 100.0, false, true, Activity::Work),
            NpcState::Stuck
        );
        assert_eq!(
            next_state(NpcState::Stuck, 100.0, true, false, Activity::Work),
            NpcState::Walking
        );
        assert_eq!(arrived_state(Activity::Shop), NpcState::Working);
        assert_eq!(arrived_state(Activity::Patrol), NpcState::Working);
        assert_eq!(arrived_state(Activity::Eat), NpcState::Talking);
        assert_eq!(arrived_state(Activity::Socialise), NpcState::Talking);
        assert_eq!(arrived_state(Activity::Idle), NpcState::Idle);
    }

    #[test]
    fn schedule_config_sanitises() {
        let config = ScheduleConfig {
            enabled: false,
            retarget_margin: f32::NAN,
        }
        .sanitised();
        assert!(!config.enabled);
        assert!(config.retarget_margin.is_finite());
        assert!(ScheduleConfig::default().enabled);
    }

    #[test]
    fn place_salts_are_unique() {
        let mut seen = Vec::new();
        for place in ActivityPlace::ALL {
            let salt = place_salt(place);
            assert!(!seen.contains(&salt), "duplicate salt for {}", place.name());
            seen.push(salt);
        }
    }

    #[test]
    fn system_starts_empty_and_survives_bad_ids() {
        let mut system = NpcSystem::new(NpcConfig::default());
        assert_eq!(system.crowd().len(), 0);
        assert!(system.crowd().is_empty());
        assert!(!system.send_to(NpcId(7), Vec3::ZERO));
        assert!(!system.send_to(NpcId::INVALID, Vec3::ZERO));
        assert_eq!(system.stats().paths_computed, 0);
        assert!(system.memory_bytes() > 0);
        assert!(format!("{system:?}").contains("NpcSystem"));
        assert_eq!(system.pathfinder().cache_len(), 0);
        let schedule = system.schedule(NpcKind::Guard);
        assert!(schedule.validate().is_ok());
    }

    #[test]
    fn set_config_rebuilds_seed_dependent_state() {
        let mut system = NpcSystem::new(NpcConfig::default());
        let before = system.schedule(NpcKind::Villager).to_text();
        let config = NpcConfig {
            seed: 99,
            ..NpcConfig::default()
        };
        system.set_config(config);
        assert_eq!(system.config().seed, 99);
        assert_ne!(system.schedule(NpcKind::Villager).to_text(), before);
        let same = NpcConfig {
            seed: 99,
            max_agents: 0,
            ..NpcConfig::default()
        };
        system.set_config(same);
        assert_eq!(system.config().max_agents, 0);
    }
}
