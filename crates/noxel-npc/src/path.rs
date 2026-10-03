//! Grid pathfinding: A\* over the streamer's walkable tiles.
//!
//! # The grid
//!
//! The world is a continuous height field, but an NPC only ever needs to know
//! *which way* to walk, so paths are searched on a coarse lattice of
//! [`PathConfig::node_size`] metres (two metres by default: one node per two
//! tiles). Nodes are eight-connected; a diagonal step is only allowed when both
//! of its orthogonal neighbours are traversable, which is what stops agents
//! from cutting the corner of a house.
//!
//! # What a node costs
//!
//! A node is traversable when the tile under it is walkable, or when it is
//! water shallower than [`SHALLOW_WATER_DEPTH`] — an agent wades a stream but
//! will not swim a lake. Deep water, cliffs and building walls are impassable.
//! Paved tiles cost [`PathConfig::road_cost`], water costs
//! [`PathConfig::water_cost`], everything else costs `1.0`, plus a slope
//! penalty, so a path prefers the road and crosses the ford rather than the
//! river.
//!
//! # Determinism
//!
//! The open set is a binary heap ordered by `total_cmp` on `f` and then by node
//! coordinates, ties are broken by construction order rather than by hash
//! order, and the record map is only ever read by key. The same request against
//! the same world therefore returns a bit-identical path on every run — a
//! property [`Pathfinder`]'s tests assert directly.
//!
//! # Never reading an unloaded chunk
//!
//! Every node the search touches goes through [`WorldStreamer::chunk_at`]; a
//! node in a chunk that is not resident is treated as impassable. A goal
//! outside the streamed region is clamped towards the agent (up to
//! [`MAX_CLAMP_STEPS`] nodes) or refused, and an unreachable goal returns
//! `None` rather than a path into the void.

use std::collections::{BTreeMap, BinaryHeap, HashMap};

use noxel_core::math::Vec3;
use noxel_world::WorldStreamer;

use crate::agent::NpcStats;

/// Water deeper than this (metres below sea level) is impassable; shallower
/// water is waded at [`PathConfig::water_cost`].
pub const SHALLOW_WATER_DEPTH: f32 = 1.0;

/// How many nodes a goal outside the streamed region may be pulled inwards
/// before the request is refused. At the default two-metre node this is 256 m.
pub const MAX_CLAMP_STEPS: u32 = 128;

/// How many rings of neighbours are searched for a usable node when the goal
/// itself lands on a wall or a deep river.
pub const GOAL_SNAP_RINGS: i32 = 4;

/// Most A\* searches the crowd system starts in one step.
pub const MAX_PATHS_PER_STEP: usize = 24;

/// Most grid nodes the crowd system expands in one step, across all searches.
pub const MAX_NODES_PER_STEP: u64 = 16_384;

/// Path cache capacity, in entries.
pub const PATH_CACHE_CAPACITY: usize = 256;

/// A walkable route, as a polyline of ground positions.
#[derive(Clone, Debug)]
pub struct Path {
    /// Ground positions to walk through, in order. The first is the first
    /// *turn*, not the agent's current position.
    pub waypoints: Vec<Vec3>,
    /// Accumulated traversal cost, in metres at cost 1.0 per metre.
    pub cost: f32,
    /// True when the last waypoint is the requested destination; false when the
    /// search stopped short (a partial path to the closest reachable node).
    pub complete: bool,
}

impl Path {
    /// An empty, finished path.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            waypoints: Vec::new(),
            cost: 0.0,
            complete: true,
        }
    }

    /// A path that walks straight from `from` to `to`.
    ///
    /// Used for the last few metres of a journey, where running A\* again would
    /// cost more than walking.
    #[must_use]
    pub fn direct(from: Vec3, to: Vec3) -> Self {
        let mut waypoints = Vec::with_capacity(1);
        if to.is_finite() {
            waypoints.push(to);
        }
        Self {
            waypoints,
            cost: ground_distance(from, to),
            complete: true,
        }
    }

    /// Number of waypoints left in the whole path.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.waypoints.len()
    }

    /// True when the path has no waypoints at all.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }

    /// The waypoint at `cursor`, or `None` once the path is finished.
    #[inline]
    #[must_use]
    pub fn next_waypoint(&self, cursor: usize) -> Option<Vec3> {
        self.waypoints.get(cursor).copied()
    }

    /// The cursor advanced past every waypoint within `radius` of `position`.
    ///
    /// Distances are measured on the ground plane, because that is the plane
    /// the agent walks in: standing on a bridge above the waypoint still counts
    /// as having reached it.
    #[must_use]
    pub fn advance(&self, cursor: usize, position: Vec3, radius: f32) -> usize {
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        let mut c = cursor;
        while c < self.waypoints.len() {
            let wp = self.waypoints[c];
            if !position.is_finite() || ground_distance(position, wp) <= radius {
                // A non-finite position advances: the caller's recovery code
                // will reset the agent, and being stuck on one waypoint is
                // worse than skipping it.
                c += 1;
            } else {
                break;
            }
        }
        c
    }

    /// Total polyline length, in metres.
    #[must_use]
    pub fn length(&self) -> f32 {
        let mut total = 0.0;
        for pair in self.waypoints.windows(2) {
            total += ground_distance(pair[0], pair[1]);
        }
        total
    }

    /// A copy of this path with redundant waypoints removed.
    ///
    /// Douglas–Peucker: a waypoint is dropped when it lies within `tolerance`
    /// metres of the line between the waypoints that survive around it. A
    /// staircase of grid steps collapses to a straight walk while no point of
    /// the original route moves further than `tolerance`, which is what keeps
    /// the simplified route on the walkable ground the original followed.
    #[must_use]
    pub fn simplify(&self, tolerance: f32) -> Path {
        let tolerance = if tolerance.is_finite() {
            tolerance.max(0.0)
        } else {
            0.0
        };
        let n = self.waypoints.len();
        if n <= 2 || tolerance <= 0.0 {
            return self.clone();
        }
        let mut keep = vec![false; n];
        keep[0] = true;
        keep[n - 1] = true;
        // Iterative Douglas–Peucker: an explicit stack keeps a pathological
        // path from overflowing the real one.
        let mut stack: Vec<(usize, usize)> = vec![(0, n - 1)];
        while let Some((first, last)) = stack.pop() {
            if last <= first + 1 {
                continue;
            }
            let mut worst = 0.0f32;
            let mut worst_index = first;
            for (offset, candidate) in self.waypoints[first + 1..last].iter().enumerate() {
                let d = segment_distance(self.waypoints[first], self.waypoints[last], *candidate);
                if d > worst {
                    worst = d;
                    worst_index = first + 1 + offset;
                }
            }
            if worst > tolerance {
                keep[worst_index] = true;
                stack.push((first, worst_index));
                stack.push((worst_index, last));
            }
        }
        let waypoints: Vec<Vec3> = self
            .waypoints
            .iter()
            .zip(keep.iter())
            .filter_map(|(p, k)| if *k { Some(*p) } else { None })
            .collect();
        Path {
            waypoints,
            cost: self.cost,
            complete: self.complete,
        }
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.waypoints.capacity() * core::mem::size_of::<Vec3>() + core::mem::size_of::<Self>()
    }
}

/// Tuning for the grid search.
#[derive(Clone, Copy, Debug)]
pub struct PathConfig {
    /// Hard cap on distinct nodes a single search may touch.
    pub max_nodes: usize,
    /// Hard cap on node expansions (heap pops) per search.
    pub max_iterations: u32,
    /// Edge length of one grid node, in metres.
    pub node_size: f32,
    /// Steepest ground an agent may cross, as rise over run.
    pub max_slope: f32,
    /// Cost multiplier for wading shallow water.
    pub water_cost: f32,
    /// Cost multiplier for paved tiles.
    pub road_cost: f32,
    /// Multiplier on the heuristic; slightly above `1.0` trades a little
    /// optimality for a lot of speed.
    pub heuristic_weight: f32,
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            max_nodes: 4096,
            max_iterations: 8192,
            node_size: 2.0,
            max_slope: 0.8,
            water_cost: 8.0,
            road_cost: 0.5,
            heuristic_weight: 1.1,
        }
    }
}

impl PathConfig {
    /// The same configuration with every field in a usable range.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.max_nodes = self.max_nodes.clamp(2, 1 << 22);
        self.max_iterations = self.max_iterations.clamp(2, 1 << 24);
        self.node_size = if self.node_size.is_finite() && self.node_size >= 0.25 {
            self.node_size.min(64.0)
        } else {
            2.0
        };
        self.max_slope = if self.max_slope.is_finite() && self.max_slope > 0.0 {
            self.max_slope.min(10.0)
        } else {
            0.8
        };
        self.water_cost = finite_cost(self.water_cost, 8.0);
        self.road_cost = finite_cost(self.road_cost, 0.5);
        self.heuristic_weight = if self.heuristic_weight.is_finite() && self.heuristic_weight > 0.0
        {
            self.heuristic_weight.clamp(0.1, 4.0)
        } else {
            1.1
        };
        self
    }
}

/// A finite, non-negative cost, falling back to `fallback`.
fn finite_cost(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

/// One queued path computation.
#[derive(Clone, Copy, Debug)]
pub struct PathRequest {
    /// Where the agent is.
    pub from: Vec3,
    /// Where the agent wants to be.
    pub to: Vec3,
    /// Higher runs first; the crowd system uses tier and distance.
    pub priority: u8,
}

impl PathRequest {
    /// A request with the given endpoints and priority.
    #[must_use]
    pub fn new(from: Vec3, to: Vec3, priority: u8) -> Self {
        Self { from, to, priority }
    }
}

/// True when an agent may stand at `p`: walkable ground, or water shallow
/// enough to wade.
///
/// This is the predicate the search uses; it reports `false` for a position in
/// a chunk that is not resident, so a caller can never accidentally plan
/// through unloaded world.
#[must_use]
pub fn is_traversable(streamer: &WorldStreamer, config: &PathConfig, p: Vec3) -> bool {
    node_cost(streamer, config, p).is_some()
}

/// The cost of standing on `p`, or `None` when it is impassable or unloaded.
fn node_cost(streamer: &WorldStreamer, config: &PathConfig, p: Vec3) -> Option<f32> {
    terrain_cost(streamer, config.road_cost, config.water_cost, p)
}

/// The terrain cost of a position, given the two preference multipliers.
///
/// This is the single place the engine decides what "walkable" means for NPCs,
/// shared by the A\* grid and the flow fields so the two can never disagree:
///
/// * a walkable tile costs `road_cost` when it is paved and `1.0` otherwise,
///   scaled by the tile's slope;
/// * water shallower than [`SHALLOW_WATER_DEPTH`] costs `water_cost`;
/// * deep water, cliffs, building walls and anything in a chunk that is not
///   resident are impassable.
pub(crate) fn terrain_cost(
    streamer: &WorldStreamer,
    road_cost: f32,
    water_cost: f32,
    p: Vec3,
) -> Option<f32> {
    if !p.is_finite() {
        return None;
    }
    let chunk = streamer.chunk_at(p)?;
    let (x, y) = chunk.world_to_tile(p)?;
    let set = streamer.generator().tile_set();
    let def = chunk.tile_def(x, y, set)?;
    let height = chunk.height(x, y);
    let sea_level = streamer.generator().config().sea_level;
    if def.flags.walkable {
        let slope = chunk.slope(x, y);
        let penalty = if slope.is_finite() { 1.0 + slope } else { 1.0 };
        let base = if def.flags.road { road_cost } else { 1.0 };
        Some((base * penalty).max(0.0))
    } else if def.flags.water {
        let depth = sea_level - height;
        if depth.is_finite() && depth <= SHALLOW_WATER_DEPTH {
            Some(water_cost)
        } else {
            None
        }
    } else {
        None
    }
}

/// `(x, z)` grid coordinates of a position.
#[inline]
fn node_of(p: Vec3, node_size: f32) -> (i32, i32) {
    let inv = 1.0 / node_size;
    (
        (p.x * inv).floor().clamp(-1.0e7, 1.0e7) as i32,
        (p.z * inv).floor().clamp(-1.0e7, 1.0e7) as i32,
    )
}

/// The ground position at the centre of a node.
#[inline]
fn node_centre(node: (i32, i32), node_size: f32) -> Vec3 {
    Vec3::new(
        (node.0 as f32 + 0.5) * node_size,
        0.0,
        (node.1 as f32 + 0.5) * node_size,
    )
}

/// Ground-plane distance between two positions.
#[inline]
fn ground_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    (dx * dx + dz * dz).sqrt()
}

/// Distance from `p` to the segment `a`–`b`, on the ground plane.
fn segment_distance(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let abx = b.x - a.x;
    let abz = b.z - a.z;
    let len_sq = abx * abx + abz * abz;
    if len_sq <= 1e-9 {
        return ground_distance(a, p);
    }
    let t = (((p.x - a.x) * abx + (p.z - a.z) * abz) / len_sq).clamp(0.0, 1.0);
    let cx = a.x + abx * t;
    let cz = a.z + abz * t;
    let dx = p.x - cx;
    let dz = p.z - cz;
    (dx * dx + dz * dz).sqrt()
}

/// One entry in the A\* open set.
///
/// `f` is stored as raw bits: every cost is finite and non-negative, so the bit
/// pattern is monotonic in the value and the node can derive `Ord`, which keeps
/// the heap exactly reproducible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenNode {
    f_bits: u32,
    x: i32,
    y: i32,
}

impl OpenNode {
    fn new(f: f32, node: (i32, i32)) -> Self {
        let f = if f.is_finite() { f.max(0.0) } else { f32::MAX };
        Self {
            f_bits: f.to_bits(),
            x: node.0,
            y: node.1,
        }
    }
}

impl Ord for OpenNode {
    /// A **min**-heap ordering: the smallest `f` is the greatest node.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other
            .f_bits
            .cmp(&self.f_bits)
            .then_with(|| other.x.cmp(&self.x))
            .then_with(|| other.y.cmp(&self.y))
    }
}

impl PartialOrd for OpenNode {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// One visited node.
#[derive(Clone, Copy, Debug)]
struct Record {
    g: f32,
    parent: (i32, i32),
    closed: bool,
}

/// Search state that outlives a single request, so a frame's twenty-four
/// searches allocate once rather than twenty-four times.
#[derive(Debug, Default)]
struct Search {
    open: BinaryHeap<OpenNode>,
    records: HashMap<(i32, i32), Record>,
}

/// The eight grid neighbours, in a fixed order so ties resolve identically on
/// every run.
const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

/// A grid A\* pathfinder with a small cache of recent routes.
///
/// One pathfinder serves a whole crowd: the search state and the cache are
/// reused between requests, and the statistics it reports are cumulative, which
/// is what makes "did sharing a flow field save work?" a measurable question.
#[derive(Debug)]
pub struct Pathfinder {
    config: PathConfig,
    search: Search,
    cache: HashMap<(i32, i32, i32, i32), Path>,
    stamps: HashMap<(i32, i32, i32, i32), u64>,
    order: BTreeMap<u64, (i32, i32, i32, i32)>,
    clock: u64,
    stats: NpcStats,
    cached_seed: Option<u64>,
}

impl Pathfinder {
    /// A pathfinder with the given tuning.
    #[must_use]
    pub fn new(config: PathConfig) -> Self {
        Self {
            config: config.sanitised(),
            search: Search::default(),
            cache: HashMap::new(),
            stamps: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            stats: NpcStats::default(),
            cached_seed: None,
        }
    }

    /// The tuning in force.
    #[must_use]
    pub fn config(&self) -> &PathConfig {
        &self.config
    }

    /// Replaces the tuning and drops the cache, which may no longer be valid.
    pub fn set_config(&mut self, config: PathConfig) {
        self.config = config.sanitised();
        self.clear_cache();
    }

    /// Finds a route from `from` to `to`.
    ///
    /// Returns `None` when the start is not on resident, traversable ground,
    /// when the goal cannot be brought inside the streamed region, or when the
    /// goal is traversable but walled off from the start by deep water, cliffs
    /// or buildings. A goal that is *itself* blocked (a destination inside a
    /// wall) yields a partial path to the closest reachable node, marked with
    /// [`Path::complete`] `false`.
    pub fn find_path(&mut self, streamer: &WorldStreamer, from: Vec3, to: Vec3) -> Option<Path> {
        let config = self.config;
        if !from.is_finite() || !to.is_finite() {
            return None;
        }
        // The start must be somewhere an agent can stand and that the streamer
        // actually holds.
        node_cost(streamer, &config, from)?;

        // A different world invalidates every cached route.
        let seed = streamer.generator().config().seed;
        if self.cached_seed != Some(seed) {
            self.clear_cache();
            self.cached_seed = Some(seed);
        }

        let start_node = node_of(from, config.node_size);
        let goal = self.clamp_goal(streamer, &config, from, to)?;
        let goal_node = node_of(goal, config.node_size);
        let key = (start_node.0, start_node.1, goal_node.0, goal_node.1);
        if let Some(path) = self.cache.get(&key) {
            let path = path.clone();
            self.touch(key);
            return Some(path);
        }

        // A blocked goal is snapped to the closest usable node and the result
        // is flagged incomplete.
        let (target, complete) =
            match node_cost(streamer, &config, node_centre(goal_node, config.node_size)) {
                Some(_) => (goal_node, true),
                None => (
                    self.nearest_traversable(streamer, &config, goal_node)?,
                    false,
                ),
            };

        let path = self.search(streamer, &config, from, start_node, target, goal, complete);
        if let Some(path) = &path {
            self.store(key, path.clone());
        }
        path
    }

    /// Runs the A\* search itself.
    #[allow(clippy::too_many_arguments)]
    fn search(
        &mut self,
        streamer: &WorldStreamer,
        config: &PathConfig,
        from: Vec3,
        start_node: (i32, i32),
        target: (i32, i32),
        goal: Vec3,
        complete: bool,
    ) -> Option<Path> {
        self.stats.paths_computed = self.stats.paths_computed.saturating_add(1);
        self.search.open.clear();
        self.search.records.clear();
        let node_size = config.node_size;
        // The start node's own cost is intentionally not charged: the agent is
        // already standing there.
        self.search.records.insert(
            start_node,
            Record {
                g: 0.0,
                parent: start_node,
                closed: false,
            },
        );
        self.search.open.push(OpenNode::new(
            self.heuristic(start_node, target, config),
            start_node,
        ));

        let mut expanded = 0u32;
        let mut reached = false;
        let mut best: Option<((i32, i32), f32)> = None;
        while let Some(top) = self.search.open.pop() {
            let node = (top.x, top.y);
            let Some(record) = self.search.records.get(&node).copied() else {
                continue;
            };
            if record.closed {
                continue;
            }
            if let Some(entry) = self.search.records.get_mut(&node) {
                entry.closed = true;
            }
            expanded += 1;
            self.stats.path_nodes_expanded = self.stats.path_nodes_expanded.saturating_add(1);

            if node == target {
                reached = true;
                break;
            }
            let h = self.heuristic_plain(node, target, config);
            if best.is_none_or(|(_, best_h)| h < best_h) {
                best = Some((node, h));
            }
            if expanded >= config.max_iterations || self.search.records.len() >= config.max_nodes {
                break;
            }

            let here = node_centre(node, node_size);
            let here_height = streamer.height_at(here);
            for (dx, dy) in NEIGHBOURS {
                let next = (node.0 + dx, node.1 + dy);
                let centre = node_centre(next, node_size);
                let Some(step_cost) = node_cost(streamer, config, centre) else {
                    continue;
                };
                let diagonal = dx != 0 && dy != 0;
                if diagonal {
                    // No corner cutting: both orthogonal neighbours must be
                    // usable, or the agent would clip the corner of a wall.
                    let a = node_centre((node.0 + dx, node.1), node_size);
                    let b = node_centre((node.0, node.1 + dy), node_size);
                    if node_cost(streamer, config, a).is_none()
                        || node_cost(streamer, config, b).is_none()
                    {
                        continue;
                    }
                }
                let distance = if diagonal {
                    node_size * core::f32::consts::SQRT_2
                } else {
                    node_size
                };
                let height = streamer.height_at(centre);
                let rise = (height - here_height).abs();
                if rise / distance > config.max_slope {
                    continue;
                }
                let slope_penalty = 1.0 + rise / distance;
                let tentative = record.g + step_cost * distance * slope_penalty;
                let known = self.search.records.get(&next);
                match known {
                    Some(existing) if existing.closed || existing.g <= tentative => continue,
                    _ => {}
                }
                self.search.records.insert(
                    next,
                    Record {
                        g: tentative,
                        parent: node,
                        closed: false,
                    },
                );
                let f = tentative + self.heuristic(next, target, config);
                self.search.open.push(OpenNode::new(f, next));
            }
        }

        if !reached && complete {
            // The goal exists but is unreachable: a wall of deep water, or a
            // house with no door. Refusing is the honest answer.
            return None;
        }
        let end = if reached {
            target
        } else {
            // A partial path to the closest node the search managed to reach.
            best.map(|(node, _)| node)?
        };
        let mut chain: Vec<(i32, i32)> = Vec::new();
        let mut cursor = end;
        let mut guard = 0usize;
        while cursor != start_node {
            chain.push(cursor);
            let Some(record) = self.search.records.get(&cursor) else {
                break;
            };
            let parent = record.parent;
            if parent == cursor {
                break;
            }
            cursor = parent;
            guard += 1;
            if guard > config.max_nodes {
                break;
            }
        }
        chain.reverse();
        let mut waypoints: Vec<Vec3> = Vec::with_capacity(chain.len() + 1);
        for node in chain {
            let centre = node_centre(node, node_size);
            waypoints.push(Vec3::new(centre.x, streamer.height_at(centre), centre.z));
        }
        let reached_goal = reached && end == target;
        if reached_goal {
            let last = waypoints.last().copied().unwrap_or(from);
            if ground_distance(last, goal) > 0.05 {
                waypoints.push(Vec3::new(goal.x, streamer.height_at(goal), goal.z));
            }
        }
        let cost = if reached_goal {
            self.search
                .records
                .get(&target)
                .map_or(0.0, |record| record.g)
        } else {
            self.search.records.get(&end).map_or(0.0, |record| record.g)
        };
        if waypoints.is_empty() {
            // The agent is already inside the target node; walk straight there.
            return Some(Path::direct(from, goal));
        }
        Some(Path {
            waypoints,
            cost,
            complete: reached_goal && complete,
        })
    }

    /// Pulls a goal that is outside the streamed region back towards the agent.
    fn clamp_goal(
        &self,
        streamer: &WorldStreamer,
        config: &PathConfig,
        from: Vec3,
        to: Vec3,
    ) -> Option<Vec3> {
        if streamer.chunk_at(to).is_some() {
            return Some(to);
        }
        let node_size = config.node_size;
        let delta = from - to;
        let length = ground_distance(from, to);
        if length <= node_size {
            return None;
        }
        let dir = Vec3::new(delta.x / length, 0.0, delta.z / length);
        let steps = ((length / node_size).ceil() as u32).min(MAX_CLAMP_STEPS);
        for step in 1..=steps {
            let candidate = to + dir * (step as f32 * node_size);
            if streamer.chunk_at(candidate).is_some()
                && node_cost(streamer, config, candidate).is_some()
            {
                return Some(candidate);
            }
        }
        None
    }

    /// The closest traversable node to `blocked`, searching outwards in rings.
    fn nearest_traversable(
        &self,
        streamer: &WorldStreamer,
        config: &PathConfig,
        blocked: (i32, i32),
    ) -> Option<(i32, i32)> {
        for ring in 1..=GOAL_SNAP_RINGS {
            let mut best: Option<((i32, i32), f32)> = None;
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dy.abs() != ring {
                        continue;
                    }
                    let node = (blocked.0 + dx, blocked.1 + dy);
                    let centre = node_centre(node, config.node_size);
                    if node_cost(streamer, config, centre).is_none() {
                        continue;
                    }
                    let d = ground_distance(node_centre(blocked, config.node_size), centre);
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((node, d));
                    }
                }
            }
            if let Some((node, _)) = best {
                return Some(node);
            }
        }
        None
    }

    /// Octile distance to the target, with the heuristic weight applied.
    fn heuristic(&self, node: (i32, i32), target: (i32, i32), config: &PathConfig) -> f32 {
        self.heuristic_plain(node, target, config) * config.heuristic_weight
    }

    /// Octile distance in metres between two nodes.
    fn heuristic_plain(&self, node: (i32, i32), target: (i32, i32), config: &PathConfig) -> f32 {
        let dx = (node.0 - target.0).unsigned_abs() as f32;
        let dy = (node.1 - target.1).unsigned_abs() as f32;
        let (min, max) = if dx < dy { (dx, dy) } else { (dy, dx) };
        (min * core::f32::consts::SQRT_2 + (max - min)) * config.node_size
    }

    /// Marks a cached entry as recently used.
    ///
    /// The previous ordering record is dropped before the new one is inserted,
    /// so the index holds exactly one record per cached route and can never
    /// grow while the cache is only being read.
    fn touch(&mut self, key: (i32, i32, i32, i32)) {
        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.stamps.get(&key).copied() {
            self.order.remove(&previous);
        }
        self.order.insert(self.clock, key);
        self.stamps.insert(key, self.clock);
    }

    /// Caches a path, evicting the least recently used entry when full.
    fn store(&mut self, key: (i32, i32, i32, i32), path: Path) {
        if PATH_CACHE_CAPACITY == 0 {
            return;
        }
        self.touch(key);
        self.cache.insert(key, path);
        while self.cache.len() > PATH_CACHE_CAPACITY {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.cache.remove(&oldest);
            self.stamps.remove(&oldest);
        }
    }

    /// A snapshot of the pathfinding counters.
    ///
    /// `paths_computed` counts searches actually run, so a cache hit is free;
    /// `path_nodes_expanded` counts heap pops across every search.
    #[must_use]
    pub fn stats(&self) -> NpcStats {
        NpcStats {
            paths_computed: self.stats.paths_computed,
            path_nodes_expanded: self.stats.path_nodes_expanded,
            flow_fields_cached: self.cache.len(),
            ..NpcStats::default()
        }
    }

    /// How many routes are cached.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Drops every cached route.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.stamps.clear();
        self.order.clear();
        self.clock = 0;
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let cached: usize = self
            .cache
            .values()
            .map(|path| path.memory_bytes() + 32)
            .sum();
        cached
            + self.stamps.len() * 24
            + self.order.len() * 24
            + self.search.records.capacity() * 48
            + self.search.open.capacity() * core::mem::size_of::<OpenNode>()
            + core::mem::size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_basics() {
        let path = Path {
            waypoints: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 4.0),
            ],
            cost: 7.0,
            complete: true,
        };
        assert_eq!(path.len(), 3);
        assert!(!path.is_empty());
        assert_eq!(path.next_waypoint(0), Some(Vec3::new(0.0, 0.0, 0.0)));
        assert_eq!(path.next_waypoint(3), None);
        assert!((path.length() - 7.0).abs() < 1e-4);
        assert!(Path::empty().is_empty());
        assert!(Path::empty().complete);
        assert_eq!(path.advance(0, Vec3::new(0.1, 0.0, 0.1), 0.6), 1);
        assert_eq!(path.advance(0, Vec3::new(3.0, 0.0, 4.0), 0.6), 0);
        // Standing on the last waypoint finishes the path.
        assert_eq!(path.advance(2, Vec3::new(3.0, 0.0, 4.0), 0.6), 3);
        assert_eq!(path.advance(2, Vec3::new(f32::NAN, 0.0, 0.0), 0.6), 3);
        assert_eq!(path.advance(0, Vec3::ZERO, f32::NAN), 1);
        assert!(path.memory_bytes() > 0);
    }

    #[test]
    fn direct_paths_point_at_the_destination() {
        let path = Path::direct(Vec3::ZERO, Vec3::new(3.0, 0.0, 4.0));
        assert_eq!(path.len(), 1);
        assert_eq!(path.next_waypoint(0), Some(Vec3::new(3.0, 0.0, 4.0)));
        assert!((path.cost - 5.0).abs() < 1e-5);
        assert!(Path::direct(Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0)).is_empty());
    }

    #[test]
    fn simplify_collapses_a_staircase_and_keeps_the_ends() {
        let mut waypoints = Vec::new();
        for i in 0..10 {
            waypoints.push(Vec3::new(i as f32, 0.0, i as f32));
        }
        let path = Path {
            waypoints: waypoints.clone(),
            cost: 10.0,
            complete: true,
        };
        let simplified = path.simplify(0.05);
        assert_eq!(simplified.len(), 2, "{:?}", simplified.waypoints);
        assert_eq!(simplified.waypoints[0], waypoints[0]);
        assert_eq!(
            simplified.waypoints[1],
            *waypoints.last().expect("last waypoint")
        );
        assert!(simplified.complete);
        assert_eq!(simplified.cost, path.cost);
        let untouched = path.simplify(0.0);
        assert_eq!(untouched.len(), path.len());
        let generous = path.simplify(f32::NAN);
        assert_eq!(generous.len(), path.len());
    }

    #[test]
    fn simplify_keeps_a_corner_that_leaves_the_line() {
        let path = Path {
            waypoints: vec![
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 5.0),
                Vec3::new(10.0, 0.0, 5.0),
            ],
            cost: 0.0,
            complete: true,
        };
        let simplified = path.simplify(0.5);
        assert_eq!(simplified.len(), 3, "the corner is 4 m off the line");
        assert_eq!(path.simplify(5.0).len(), 2);
        assert_eq!(Path::empty().simplify(1.0).len(), 0);
        assert_eq!(Path::direct(Vec3::ZERO, Vec3::X).simplify(1.0).len(), 1);
    }

    #[test]
    fn config_sanitises_broken_values() {
        let broken = PathConfig {
            max_nodes: 0,
            max_iterations: 0,
            node_size: -1.0,
            max_slope: f32::NAN,
            water_cost: f32::NAN,
            road_cost: -2.0,
            heuristic_weight: 0.0,
        }
        .sanitised();
        assert!(broken.max_nodes >= 2);
        assert!(broken.max_iterations >= 2);
        assert_eq!(broken.node_size, 2.0);
        assert_eq!(broken.max_slope, 0.8);
        assert_eq!(broken.water_cost, 8.0);
        assert_eq!(broken.road_cost, 0.5);
        assert_eq!(broken.heuristic_weight, 1.1);
    }

    #[test]
    fn default_config_matches_the_contract() {
        let config = PathConfig::default();
        assert_eq!(config.max_nodes, 4096);
        assert_eq!(config.max_iterations, 8192);
        assert_eq!(config.node_size, 2.0);
        assert_eq!(config.max_slope, 0.8);
        assert_eq!(config.water_cost, 8.0);
        assert_eq!(config.road_cost, 0.5);
        assert_eq!(config.heuristic_weight, 1.1);
    }

    #[test]
    fn grid_helpers_round_trip() {
        let size = 2.0;
        let p = Vec3::new(3.0, 0.0, -1.0);
        let node = node_of(p, size);
        assert_eq!(node, (1, -1));
        let centre = node_centre(node, size);
        assert_eq!(centre, Vec3::new(3.0, 0.0, -1.0));
        assert_eq!(node_of(Vec3::new(f32::NAN, 0.0, 0.0), size), (0, 0));
        assert_eq!(node_of(Vec3::new(1e30, 0.0, 0.0), size).0, 10_000_000);
        assert!((ground_distance(Vec3::ZERO, Vec3::new(3.0, 0.0, 4.0)) - 5.0).abs() < 1e-5);
        let d = segment_distance(
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 2.0),
        );
        assert!((d - 2.0).abs() < 1e-5);
        let d = segment_distance(Vec3::ZERO, Vec3::ZERO, Vec3::new(3.0, 0.0, 4.0));
        assert!((d - 5.0).abs() < 1e-5);
    }

    #[test]
    fn heuristic_is_octile() {
        let finder = Pathfinder::new(PathConfig::default());
        let h = finder.heuristic_plain((0, 0), (2, 0), &PathConfig::default());
        assert!((h - 4.0).abs() < 1e-4);
        let h = finder.heuristic_plain((0, 0), (1, 1), &PathConfig::default());
        assert!((h - 2.0 * core::f32::consts::SQRT_2).abs() < 1e-4);
        let weighted = finder.heuristic((0, 0), (1, 0), &PathConfig::default());
        assert!((weighted - 2.2).abs() < 1e-4);
    }

    #[test]
    fn open_node_orders_as_a_min_heap_of_bits() {
        let mut heap = BinaryHeap::new();
        heap.push(OpenNode::new(5.0, (0, 0)));
        heap.push(OpenNode::new(1.0, (1, 1)));
        heap.push(OpenNode::new(1.0, (0, 1)));
        assert_eq!(heap.pop().map(|n| (n.x, n.y)), Some((0, 1)));
        assert_eq!(heap.pop().map(|n| (n.x, n.y)), Some((1, 1)));
        assert_eq!(heap.pop().map(|n| (n.x, n.y)), Some((0, 0)));
        assert_eq!(OpenNode::new(f32::NAN, (0, 0)).f_bits, f32::MAX.to_bits());
        assert!(
            OpenNode::new(1.0, (0, 0))
                .partial_cmp(&OpenNode::new(1.0, (0, 0)))
                .is_some()
        );
    }
}
