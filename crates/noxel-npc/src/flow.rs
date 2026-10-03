//! Flow fields: one search, many agents.
//!
//! When two hundred villagers are all heading for the same market stall, giving
//! each of them an A\* result is two hundred searches for one answer. A
//! [`FlowField`] computes the answer once: a coarse grid covering a square
//! region around the goal, where every cell stores the direction that most
//! reduces the distance to the goal. Agents then read a vector instead of
//! running a search, which is why the crowd's `paths_computed` counter stays
//! flat while a whole plaza empties into the tavern.
//!
//! ```text
//!      ┌───┬───┬───┬───┐        ↘   ↓   ↓   ↓
//!      │   │   │   │ G │        ↘   ↘   ↓   ↓
//!      ├───┼───┼───┼───┤   →    ↘   ↘   ↘   ↓
//!      │   │   │   │   │        →   ↘   ↘   ↘
//!      └───┴───┴───┴───┘
//! ```
//!
//! Fields are cached by *quantised* goal, so agents whose destinations differ
//! by a few centimetres — the jitter of "stand somewhere near the stall" —
//! share one field. The cache is a strict LRU with a deterministic eviction
//! order (an ordered index, never a hash-map iteration), so the crowd behaves
//! identically on every run.

use std::collections::{BTreeMap, BinaryHeap, HashMap};

use noxel_core::math::Vec3;
use noxel_world::WorldStreamer;

use crate::path::PathConfig;

/// The largest region a flow field may cover, in metres.
pub const MAX_FLOW_EXTENT: f32 = 512.0;

/// The largest number of cells along one edge of a flow field.
pub const MAX_FLOW_SIDE: usize = 96;

/// One cell of the Dijkstra frontier.
///
/// Costs are finite and non-negative, so the raw bit pattern orders exactly like
/// the float and the node can derive `Ord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frontier {
    cost_bits: u32,
    index: u32,
}

impl Frontier {
    fn new(cost: f32, index: usize) -> Self {
        let cost = if cost.is_finite() {
            cost.max(0.0)
        } else {
            f32::MAX
        };
        Self {
            cost_bits: cost.to_bits(),
            index: index as u32,
        }
    }
}

impl Ord for Frontier {
    /// A min-heap ordering, with the cell index as the tie-break so equal-cost
    /// frontiers pop in a fixed order.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other
            .cost_bits
            .cmp(&self.cost_bits)
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A coarse direction grid covering a square region around a goal.
///
/// Cells outside the streamed world, inside a wall or in deep water carry no
/// direction; [`FlowField::direction_at`] returns `None` for them, and an agent
/// that walks into one falls back to its own path or steering.
#[derive(Clone, Debug)]
pub struct FlowField {
    goal: Vec3,
    origin: Vec3,
    cell: f32,
    side: usize,
    dirs: Vec<Vec3>,
    cost: Vec<f32>,
    usable: usize,
}

impl FlowField {
    /// Builds a field whose directions point towards `goal`.
    ///
    /// `extent` is the edge length of the square region in metres and `cell` is
    /// the edge length of one grid cell. Returns `None` when the goal itself is
    /// outside the streamed region, inside a wall or in deep water — a field
    /// with no valid destination is worse than no field at all.
    #[must_use]
    pub fn build(streamer: &WorldStreamer, goal: Vec3, extent: f32, cell: f32) -> Option<Self> {
        if !goal.is_finite() {
            return None;
        }
        let cell = sanitise_cell(cell);
        let extent = sanitise_extent(extent, cell);
        let side = ((extent / cell).ceil() as usize).clamp(1, MAX_FLOW_SIDE);
        let span = side as f32 * cell;
        let origin = Vec3::new(goal.x - span * 0.5, 0.0, goal.z - span * 0.5);
        let path_config = PathConfig::default();

        let cells = side * side;
        let mut cost = vec![f32::INFINITY; cells];
        let mut dirs = vec![Vec3::ZERO; cells];
        let mut solid = vec![false; cells];
        // The terrain is sampled once per cell and reused by every relaxation:
        // a Dijkstra touches each cell's neighbours eight times, and the world
        // query behind `terrain_cost` is far more expensive than the arithmetic
        // around it.
        let mut traverse = vec![1.0f32; cells];
        for index in 0..cells {
            let centre = cell_centre(origin, cell, side, index);
            match crate::path::terrain_cost(
                streamer,
                path_config.road_cost,
                path_config.water_cost,
                centre,
            ) {
                Some(step) => traverse[index] = step,
                None => solid[index] = true,
            }
        }

        let goal_index = index_of(origin, cell, side, goal)?;
        if solid[goal_index] {
            return None;
        }
        let mut frontier: BinaryHeap<Frontier> = BinaryHeap::new();
        cost[goal_index] = 0.0;
        frontier.push(Frontier::new(0.0, goal_index));
        while let Some(top) = frontier.pop() {
            let index = top.index as usize;
            if index >= cells {
                continue;
            }
            let base = f32::from_bits(top.cost_bits);
            if base > cost[index] {
                continue;
            }
            let (x, y) = (index % side, index / side);
            for (dx, dy) in NEIGHBOUR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= side as i32 || ny >= side as i32 {
                    continue;
                }
                let next = ny as usize * side + nx as usize;
                if solid[next] {
                    continue;
                }
                let diagonal = dx != 0 && dy != 0;
                if diagonal {
                    #[allow(clippy::unnecessary_cast)]
                    let a = y as usize * side + nx as usize;
                    #[allow(clippy::unnecessary_cast)]
                    let b = ny as usize * side + x as usize;
                    if solid[a] || solid[b] {
                        continue;
                    }
                }
                let distance = if diagonal {
                    cell * core::f32::consts::SQRT_2
                } else {
                    cell
                };
                let tentative = base + traverse[next] * distance;
                if tentative + 1e-6 < cost[next] {
                    cost[next] = tentative;
                    frontier.push(Frontier::new(tentative, next));
                }
            }
        }

        // Directions: each cell points at whichever neighbour is closest to the
        // goal, which produces a smooth diagonal flow rather than a staircase.
        let mut usable = 0usize;
        for index in 0..cells {
            if solid[index] || !cost[index].is_finite() {
                continue;
            }
            let (x, y) = (index % side, index / side);
            let mut best = cost[index];
            let mut best_offset = (0i32, 0i32);
            for (dx, dy) in NEIGHBOUR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= side as i32 || ny >= side as i32 {
                    continue;
                }
                let next = ny as usize * side + nx as usize;
                if solid[next] || !cost[next].is_finite() {
                    continue;
                }
                let diagonal = dx != 0 && dy != 0;
                if diagonal {
                    #[allow(clippy::unnecessary_cast)]
                    let a = y as usize * side + nx as usize;
                    #[allow(clippy::unnecessary_cast)]
                    let b = ny as usize * side + x as usize;
                    if solid[a] || solid[b] {
                        continue;
                    }
                }
                if cost[next] < best {
                    best = cost[next];
                    best_offset = (dx, dy);
                }
            }
            if best_offset != (0, 0) {
                dirs[index] =
                    Vec3::new(best_offset.0 as f32, 0.0, best_offset.1 as f32).normalize_or_zero();
                usable += 1;
            }
        }

        Some(Self {
            goal,
            origin,
            cell,
            side,
            dirs,
            cost,
            usable,
        })
    }

    /// The destination the field points at.
    #[inline]
    #[must_use]
    pub fn goal(&self) -> Vec3 {
        self.goal
    }

    /// The corner of the field's region with the smallest X and Z.
    #[inline]
    #[must_use]
    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    /// The edge length of one cell, in metres.
    #[inline]
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// The number of cells along one edge.
    #[inline]
    #[must_use]
    pub fn side(&self) -> usize {
        self.side
    }

    /// True when `p` lies inside the field's region.
    #[inline]
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        if !p.is_finite() {
            return false;
        }
        let span = self.side as f32 * self.cell;
        p.x >= self.origin.x
            && p.z >= self.origin.z
            && p.x < self.origin.x + span
            && p.z < self.origin.z + span
    }

    /// The direction to move in at `p`, or `None` when `p` is outside the
    /// field, on an impassable cell, or in a cell with no route to the goal.
    #[must_use]
    pub fn direction_at(&self, p: Vec3) -> Option<Vec3> {
        let index = index_of(self.origin, self.cell, self.side, p)?;
        let dir = self.dirs[index];
        if dir.length_squared() > 0.0 {
            Some(dir)
        } else {
            None
        }
    }

    /// The accumulated cost from the cell containing `p` to the goal, if it is
    /// reachable.
    #[must_use]
    pub fn cost_at(&self, p: Vec3) -> Option<f32> {
        let index = index_of(self.origin, self.cell, self.side, p)?;
        let cost = self.cost[index];
        if cost.is_finite() { Some(cost) } else { None }
    }

    /// The number of cells that hold a usable direction.
    ///
    /// This is deliberately *not* the cell count: a field whose region is half
    /// outside the streamed world is half empty, and `is_empty` should say so.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.usable
    }

    /// True when no cell has a route to the goal.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.usable == 0
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.dirs.capacity() * core::mem::size_of::<Vec3>()
            + self.cost.capacity() * core::mem::size_of::<f32>()
            + core::mem::size_of::<Self>()
    }
}

/// The eight neighbour offsets, in a fixed order for determinism.
const NEIGHBOUR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Clamps a cell size into a usable range.
fn sanitise_cell(cell: f32) -> f32 {
    if cell.is_finite() && cell >= 0.25 {
        cell.min(32.0)
    } else {
        2.0
    }
}

/// Clamps an extent into a usable range.
fn sanitise_extent(extent: f32, cell: f32) -> f32 {
    if extent.is_finite() && extent >= cell {
        extent.min(MAX_FLOW_EXTENT)
    } else {
        cell
    }
}

/// The centre of a cell, by flat index.
fn cell_centre(origin: Vec3, cell: f32, side: usize, index: usize) -> Vec3 {
    let x = index % side;
    let y = index / side;
    Vec3::new(
        origin.x + (x as f32 + 0.5) * cell,
        0.0,
        origin.z + (y as f32 + 0.5) * cell,
    )
}

/// The flat index of the cell containing `p`, if it is inside the field.
fn index_of(origin: Vec3, cell: f32, side: usize, p: Vec3) -> Option<usize> {
    if !p.is_finite() {
        return None;
    }
    let x = ((p.x - origin.x) / cell).floor();
    let y = ((p.z - origin.z) / cell).floor();
    if x < 0.0 || y < 0.0 || x >= side as f32 || y >= side as f32 {
        return None;
    }
    Some(y as usize * side + x as usize)
}

/// The key a flow field is cached under: the goal quantised to the cell grid,
/// plus the cell size, so two agents heading for the same stall share a field
/// even when their exact destinations differ by centimetres.
type GoalKey = (i32, i32, u32);

/// One cached field and the stamp of its last use.
#[derive(Clone, Debug)]
struct Entry {
    field: FlowField,
    stamp: u64,
}

/// A least-recently-used cache of flow fields, keyed by quantised goal.
///
/// Eviction order lives in an ordered index rather than in hash-map iteration,
/// so which fields survive a busy frame is a deterministic function of the
/// requests that were made — the crowd cannot drift between runs because of
/// hash ordering.
#[derive(Debug)]
pub struct FlowFieldCache {
    capacity: usize,
    entries: HashMap<GoalKey, Entry>,
    order: BTreeMap<u64, GoalKey>,
    clock: u64,
    builds: u64,
    hits: u64,
}

impl FlowFieldCache {
    /// An empty cache holding at most `capacity` fields.
    ///
    /// A capacity of zero is treated as one: [`FlowFieldCache::get_or_build`]
    /// must be able to return the field it has just built.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            builds: 0,
            hits: 0,
        }
    }

    /// The configured capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The field for `goal`, building it if it is not cached.
    ///
    /// Returns `None` when the field cannot be built — a goal outside the
    /// streamed region, inside a wall, or in water too deep to wade.
    pub fn get_or_build(
        &mut self,
        streamer: &WorldStreamer,
        goal: Vec3,
        extent: f32,
        cell: f32,
    ) -> Option<&FlowField> {
        let key = key_of(goal, cell);
        if self.entries.contains_key(&key) {
            self.touch(key);
            self.hits = self.hits.saturating_add(1);
            return self.entries.get(&key).map(|entry| &entry.field);
        }
        let field = FlowField::build(streamer, goal, extent, cell)?;
        self.builds = self.builds.saturating_add(1);
        self.touch(key);
        self.entries.insert(
            key,
            Entry {
                field,
                stamp: self.clock,
            },
        );
        while self.entries.len() > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if self.entries.len() <= self.capacity {
                break;
            }
            self.entries.remove(&oldest);
        }
        self.entries.get(&key).map(|entry| &entry.field)
    }

    /// The cached field for `goal`, without building anything.
    ///
    /// This is the call the crowd's per-step loop makes: it must never trigger a
    /// search, and it must not disturb the LRU order.
    #[must_use]
    pub fn find(&self, goal: Vec3, cell: f32) -> Option<&FlowField> {
        self.entries
            .get(&key_of(goal, cell))
            .map(|entry| &entry.field)
    }

    /// How many fields are cached.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops every cached field.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.clock = 0;
    }

    /// `(cached fields, fields built since construction)`.
    #[must_use]
    pub fn stats(&self) -> (usize, u64) {
        (self.entries.len(), self.builds)
    }

    /// How many `get_or_build` calls were served from the cache.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let fields: usize = self.entries.values().map(|e| e.field.memory_bytes()).sum();
        fields + self.order.len() * 24 + core::mem::size_of::<Self>()
    }

    /// Records a use of `key`.
    ///
    /// The previous ordering record is removed before the new one is inserted,
    /// so `order` never holds more than one record per cached field: the LRU
    /// index cannot grow without bound, and eviction always finds a live entry.
    fn touch(&mut self, key: GoalKey) {
        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.get(&key).map(|entry| entry.stamp) {
            self.order.remove(&previous);
        }
        self.order.insert(self.clock, key);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.stamp = self.clock;
        }
    }
}

/// The cache key for a goal at a given cell size.
fn key_of(goal: Vec3, cell: f32) -> GoalKey {
    let cell = sanitise_cell(cell);
    let x = if goal.x.is_finite() { goal.x } else { 0.0 };
    let z = if goal.z.is_finite() { goal.z } else { 0.0 };
    let qx = (x / cell).round().clamp(-1.0e7, 1.0e7) as i32;
    let qz = (z / cell).round().clamp(-1.0e7, 1.0e7) as i32;
    (qx, qz, cell.to_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontier_orders_as_a_min_heap() {
        let mut heap = BinaryHeap::new();
        heap.push(Frontier::new(9.0, 0));
        heap.push(Frontier::new(1.0, 2));
        heap.push(Frontier::new(1.0, 1));
        assert_eq!(heap.pop().map(|f| f.index), Some(1));
        assert_eq!(heap.pop().map(|f| f.index), Some(2));
        assert_eq!(heap.pop().map(|f| f.index), Some(0));
        assert_eq!(Frontier::new(f32::NAN, 0).cost_bits, f32::MAX.to_bits());
        assert!(
            Frontier::new(1.0, 0)
                .partial_cmp(&Frontier::new(2.0, 0))
                .is_some()
        );
    }

    #[test]
    fn sanitising_clamps_cells_and_extents() {
        assert_eq!(sanitise_cell(f32::NAN), 2.0);
        assert_eq!(sanitise_cell(0.0), 2.0);
        assert_eq!(sanitise_cell(100.0), 32.0);
        assert_eq!(sanitise_cell(1.5), 1.5);
        assert_eq!(sanitise_extent(f32::NAN, 2.0), 2.0);
        assert_eq!(sanitise_extent(0.5, 2.0), 2.0);
        assert_eq!(sanitise_extent(10_000.0, 2.0), MAX_FLOW_EXTENT);
        assert_eq!(sanitise_extent(64.0, 2.0), 64.0);
    }

    #[test]
    fn cell_indexing_round_trips() {
        let origin = Vec3::new(-10.0, 0.0, 5.0);
        let cell = 2.0;
        let side = 4;
        for index in 0..side * side {
            let centre = cell_centre(origin, cell, side, index);
            assert_eq!(index_of(origin, cell, side, centre), Some(index));
        }
        assert_eq!(
            index_of(origin, cell, side, Vec3::new(-100.0, 0.0, 0.0)),
            None
        );
        assert_eq!(
            index_of(origin, cell, side, Vec3::new(f32::NAN, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn cache_keys_quantise_the_goal() {
        let a = key_of(Vec3::new(10.0, 0.0, 10.0), 2.0);
        let b = key_of(Vec3::new(10.2, 5.0, 10.1), 2.0);
        assert_eq!(a, b, "nearby goals share a field");
        let c = key_of(Vec3::new(30.0, 0.0, 10.0), 2.0);
        assert_ne!(a, c);
        let d = key_of(Vec3::new(f32::NAN, 0.0, f32::NAN), 2.0);
        assert_eq!(d, key_of(Vec3::ZERO, 2.0));
    }

    #[test]
    fn cache_constructs_and_reports() {
        let mut cache = FlowFieldCache::new(0);
        assert_eq!(cache.capacity(), 1, "a zero capacity still holds one field");
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.stats(), (0, 0));
        assert_eq!(cache.hits(), 0);
        assert!(cache.find(Vec3::ZERO, 2.0).is_none());
        cache.clear();
        assert!(cache.memory_bytes() > 0);
    }
}
