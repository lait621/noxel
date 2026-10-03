# NPCs

`noxel-npc` simulates a town full of people: spawn and despawn around a moving
focus, level-of-detail tiers, A\* and flow-field pathfinding, steering, and a
daily schedule per kind. It is deterministic — the same seed produces the same
town — and it targets the requirement "support large numbers of simultaneous
NPCs": **1000+ resident agents inside roughly four milliseconds**.

> **Measured.** `crates/noxel-npc/tests/crowd.rs::a_thousand_agents_stay_inside_the_frame_budget`
> builds a 1000-agent town and runs it for 300 steps. On the development machine
> it reports **mean 1.26 ms per step, worst 2.16 ms, 1.26 µs per agent**, 0.9 MB
> of heap — about a third of the engine's 4 ms NPC budget, and well inside the
> 8 ms mean the test asserts. `tests/` holds five integration files
> (`crowd`, `flow_fields`, `pathfinding`, `spawner`, `steering`), plus 66 unit
> tests in the library. Everything else below is read from the source.

## The frame

```text
maintain_population   spawn/despawn so the target population is resident
refresh_pois          the local plaza, market, tavern, work and road
assign_tiers          distance from the crowd centre decides the tier
rebuild_neighbours    one spatial hash for the whole crowd
step_agents           decide (if the tier clock says so), steer, move
resolve_paths         budgeted A* for whatever asked for a route
sync_bodies           tier-0 agents own a physics capsule
```

`update` does **not** step the physics world: the application calls
`PhysicsWorld::step` in its own fixed-step loop, and the crowd's character moves
are applied on top of whatever the solver did.

## Crowd tiers, and why

A town needs hundreds of people and a frame has four milliseconds, so an agent's
simulation level is a pure function of its distance from the crowd centre
(usually the camera focus):

| Tier | `TierConfig` radius | Decision interval | Steering | Collides |
|---|---|---|---|---|
| 0 `Near` | 25 m | every step (0 s) | full | yes |
| 1 `Mid` | 60 m | 0.25 s | full | no |
| 2 `Far` | 90 m | 0.75 s | path only | no |
| 3 `Frozen` | beyond | 2.0 s | path only | no |

Only tier 0 owns a capsule, so only the agents the player can touch pay for
`move_character`; everyone else walks the same path on a coarse clock.
`TierConfig::sanitised()` orders and clamps the radii (`mid >= near`,
`far >= mid`, each ≥ 1 m) and forces the intervals finite and non-negative;
`tier_for(distance)` uses strict `<`, so exactly `near` is `Mid`. `CrowdTier` is
`Near = 0 … Frozen = 3` with `ALL`, `index()`, `from_index()`, `name()`,
`Default`.

## Using the system

```rust,no_run
use noxel_core::math::Vec3;
use noxel_npc::{NpcConfig, NpcContext, NpcSystem};

// `streamer` and `physics` come from the application; `center` is usually the
// camera focus and `world_time` is seconds since midnight.
let mut system = NpcSystem::new(NpcConfig::default());
let mut ctx = NpcContext::new(&streamer, &mut physics, camera, 8.0 * 3600.0, 1.0 / 60.0);
system.maintain_population(&mut ctx);
let stats = system.update(&mut ctx);
```

| `NpcSystem` method | Purpose |
|---|---|
| `new(config)` / `config()` / `set_config()` | the config is sanitised; `set_config` rebuilds the seed-dependent state (schedules, spawner, tiers) |
| `maintain_population(&mut ctx)` | spawn/despawn so `target_population` is resident |
| `update(&mut ctx) -> NpcStats` | one step; returns the statistics |
| `send_to(id, destination) -> bool` | assigns a destination and requests a path, served on the **next** `update` ahead of the crowd's own requests; `false` means the id is not resident or the destination is not finite |
| `crowd()` / `crowd_mut()`, `pathfinder()`, `flow_cache()`, `schedule(kind)` | the sub-systems, for readouts and tests |
| `stats()`, `memory_bytes()` | readouts |

`NpcContext` borrows the world for one step: `streamer`, `physics`, `center`,
`world_time` (seconds since the world started) and `dt`. `hour_of(world_time)`
converts to hours and wraps, so `8.5 * 3600.0` is 08:30.

## `NpcConfig`

| Field | Default | Meaning |
|---|---|---|
| `seed` | `0` | every spawn, schedule, destination and path tie descends from it |
| `max_agents` | 4096 | hard ceiling (`sanitised()` caps at `1 << 20`) |
| `spawn_radius` | 70.0 m | agents are spawned within this radius of the centre |
| `despawn_radius` | 90.0 m | agents further than this are retired; `sanitised()` raises it to at least `spawn_radius`, which is the hysteresis that stops boundary churn |
| `target_population` | 400 | what `maintain_population` keeps resident (clamped to `max_agents`) |
| `tier` | `TierConfig::default()` | the tier table above |
| `steering` | `SteeringWeights::default()` | the weights below |
| `path` | `PathConfig::default()` | the A\* tuning below |
| `schedule` | `ScheduleConfig::default()` | `enabled: true`, `retarget_margin: 0.25` h |
| `agent_height` | 1.7 m | physics capsule height (clamped 0.2 … 8) |
| `agent_radius` | 0.35 m | capsule radius and separation radius (clamped 0.05 … 4, and ≤ half the height) |
| `walk_speed` | 1.6 m/s | clamped 0.05 … 40 |
| `run_speed` | 4.2 m/s | used when fleeing (clamped to ≥ `walk_speed`, ≤ 60) |
| `arrive_distance` | 0.6 m | how close counts as arrived |
| `repath_interval` | 1.5 s | seconds between path refreshes while travelling |

`NpcConfig::sanitised()` is applied by `NpcSystem::new` and `set_config`, so a
config from a file cannot produce negative radii, non-finite speeds or a
`max_agents` of zero.

## Agents and kinds

`NpcAgent` is plain data with public fields — `id`, `kind`, `state`, `position`
(**feet**, metres), `velocity`, `yaw` (radians, `0` looks along `-Z`), `tier`,
`health`, `path_cursor`, `decision_timer`, `destination`, `blocked`,
`blocked_time` — plus `new(id, kind, position)`, `is_moving()`, `speed()`
(horizontal only), `forward()`, `face()`, `distance_to()`, `to_transform()` and
`capsule_center(height)`. `MOVING_SPEED` is 0.05 m/s, below a centimetre per frame
at 60 Hz. The record is `Send + Sync + 'static`, so it is a valid
`noxel_ecs::Component`. `NpcId(pub u32)` is monotonic and **never recycled**:
despawning retires an id for the lifetime of the manager, so a stale id can never
resolve to a different agent; `NpcId::INVALID` is `u32::MAX`.

| `NpcKind` | `speed_scale` | `max_health` | Behaviour |
|---|---|---|---|
| `Villager` | 1.0 | 100 | the baseline resident |
| `Guard` | 1.05 | 140 | faster, tougher, outdoors at dawn and dusk |
| `Merchant` | 0.9 | 100 | slower, tied to a stall |
| `Child` | 1.1 | 60 | quicker over short distances, fragile |
| `Animal` | 0.7 | 40 | slow, low health, no fixed home |

`NpcState` is `Idle`, `Walking`, `Working`, `Talking`, `Sleeping`, `Fleeing` or
`Stuck`, with `ALL`, `name()` and `is_moving()`. An agent that reports no progress
for `STUCK_TIME` (3.0 s) is marked stuck and gets up to `MAX_REPAIRS` (2) repath
attempts; below `FLEE_HEALTH` (30) it flees at `run_speed`. `CrowdManager` owns
the data, not the behaviour: `spawn(kind, position) -> NpcId` (`INVALID` when
full), `despawn(id)`, `agent`/`agent_mut`, `iter`/`iter_mut`/`ids` (**slot
order**, never hash order), `len`, `slot_count`, `slots`, `clear`. `NpcStats`
carries `active`, `spawned_this_step`, `despawned_this_step`, `tier_counts[4]`,
the path counters, `blocked`, `last_step_ms`, `mean_step_ms` and `us_per_agent`.

## Pathfinding: A\* over a coarse lattice

Paths are searched on a lattice of `PathConfig::node_size` metres (**2 m**).
Nodes are eight-connected, and a diagonal step is allowed only when **both**
orthogonal neighbours are traversable — that stops an agent cutting the corner of
a house off a road. One function, `terrain_cost`, is shared with the flow fields
so the two can never disagree:

| Terrain | Cost |
|---|---|
| walkable tile, not paved | `1.0 × (1 + slope)` |
| walkable tile with `TileFlags::road` | `road_cost` (0.5) `× (1 + slope)` |
| water shallower than `SHALLOW_WATER_DEPTH` (1.0 m) | `water_cost` (8.0) |
| deep water, cliff, wall (unwalkable non-water) | impassable |
| a chunk that is not resident | impassable |

Slope is a **multiplier**, not a gate; `max_slope` (0.8 rise over run) is applied
separately between adjacent nodes, so a cliff edge is not a cheap shortcut. A road
is half price and water eight times, so a path prefers the street network and
crosses at a ford without either being absolute. **Unloaded chunks are
impassable**, so a crowd must stay inside the streaming radius — which is why the
spawn radius must sit inside the view distance.

| `PathConfig` | Default | Sanitised to |
|---|---|---|
| `max_nodes` | 4096 | `2 … 1 << 22` |
| `max_iterations` | 8192 | `2 … 1 << 24` |
| `node_size` | 2.0 m | `0.25 … 64` |
| `max_slope` | 0.8 | `> 0 … 10` |
| `water_cost` | 8.0 | finite, non-negative |
| `road_cost` | 0.5 | finite, non-negative |
| `heuristic_weight` | 1.1 | `0.1 … 4.0` |

`heuristic_weight` above 1.0 is deliberately inadmissible: a little optimality
traded for a lot of speed, invisible at village scale. `Path` carries
`waypoints`, `cost` and `complete` (`false` when the search stopped at the closest
reachable node), with `empty()`, `direct(from, to)`, `len()`, `length()`,
`next_waypoint(cursor)`, `advance(cursor, position, radius)` and
`simplify(tolerance)`. `Pathfinder::find_path(streamer, from, to)` is the search;
`stats()`, `cache_len()` and `clear_cache()` are the readouts. Step budgets bound
the worst frame: `MAX_PATHS_PER_STEP` (24 searches), `MAX_NODES_PER_STEP` (16 384
expansions), `PATH_CACHE_CAPACITY` (256 results), `GOAL_SNAP_RINGS` (4, nudging a
goal inside a wall outwards) and `MAX_CLAMP_STEPS` (128). **Determinism:** the
frontier is a binary heap ordered by `total_cmp` on `f` and then by node
coordinates, so ties pop in a fixed order rather than in hash order —
`the_same_request_twice_is_identical` and `two_pathfinders_agree_bit_for_bit`
assert it, and `tests/pathfinding.rs` also covers deep water,
unloaded-impassable behaviour and the cache.

## Flow fields: one search, many agents

Two hundred villagers heading for one stall should not run two hundred searches.
A `FlowField` computes the answer once: a coarse grid around the goal where every
cell stores the direction that most reduces the distance to it.
`FlowField::build(streamer, goal, extent, cell)` returns `None` when the goal is
outside the streamed region, inside a wall, or in water too deep to wade;
`direction_at(p)`, `cost_at(p)` and `contains(p)` are the reads. Defaults:
`FLOW_CELL` 2.0 m, `FLOW_EXTENT` 60.0 m, `FLOW_CACHE_CAPACITY` 24 fields,
`MAX_FLOW_EXTENT` 512 m, `MAX_FLOW_SIDE` 96 cells. The cache is keyed by a
**quantised goal**, so destinations a few centimetres apart share one field, and
eviction is a strict LRU whose order lives in an ordered index rather than
hash-map iteration, so which fields survive a busy frame is a deterministic
function of the requests made. `find` never rebuilds; it is the call the per-step
loop makes.

A flow field beats A\* when many agents share a destination that is static for a
while — exactly what a daily schedule produces. A\* wins for one agent, one unique
destination, or a small crowd. `paths_computed` staying flat while a plaza empties
into the tavern is the signal that the cache is working.

## Steering

`Steering::force` sums Reynolds behaviours into an acceleration; the system adds
the two world-dependent terms before clamping a second time.

| Term | What it does | Default weight |
|---|---|---|
| seek | accelerates towards the desired direction | 1.0 |
| separation | pushes away from too-close neighbours | 1.6 |
| alignment | matches the neighbours' average heading | 0.15 |
| cohesion | drifts towards the neighbours' centre | 0.1 |
| obstacle | avoids what the probes hit | 2.0 |
| road bias | pulls towards the nearest road | 0.25 |

`max_force` is 12.0 m/s², `neighbour_radius` 2.0 m and `separation_radius` 0.9 m.
Separation grows with the crowd; the clamp does not — that is what stops twenty
agents in a doorway from launching anybody.
`SteeringWeights::sanitised()` replaces non-finite weights with the defaults.
`Steering::build_neighbours(agents, cell)` builds one spatial hash per step rather
than testing every pair. `Steering::avoid(agent, desired, streamer, probe)` probes
left, centre and right at `AVOID_ANGLE` (0.698 rad ≈ 40°) with `DEFAULT_PROBE`
(2.0 m) using the streamer's walkability, and refuses a sidestep that would climb
more than `AVOID_MAX_RISE` (0.8 m) — avoidance that respects the same slope limit
the controller does. Steering produces a **desired displacement**, not a teleport;
tier 0 hands it to `move_character`.

## Daily schedules

A day is a sorted list of `ScheduleEntry` boundaries covering `[0, 24)` exactly
once — an invariant that makes the lookup a binary search and `to_text()`
readable as a timetable (`00:00 sleep home`, `06:30 eat tavern`, `07:00 commute
road`, `08:00 work work`, `22:00 sleep home`). `Activity` (`name()`, `place()`)
is what the agent is doing and `ActivityPlace` (`name()`) is where.
`DailySchedule::for_kind(kind, seed)` makes the day a **pure function of
`(kind, seed)`** — no per-agent storage — with `activity_at(hour)`,
`current(hour)`, `next_boundary(hour)` (wrapping past midnight), `validate()`,
`to_text()`, `total_hours()`, `len()` and `memory_bytes()`. `ScheduleConfig` is
`enabled` (false makes everyone wander near the centre, which is what a steering
debug view wants) plus `retarget_margin` (default 0.25 h): an agent heads for its
next activity's place that long before the change, so the shopkeeper walks to the
shop at ten to eight rather than standing at home. Boundaries are derived from the
seed, so two villages keep slightly different hours while staying within an hour
of the authored times. The system resolves the schedule into five **local points
of interest** per crowd — `plaza`, `market`, `tavern`, `work` and the nearest
`road` — refreshed when the centre moves more than `POI_REFRESH_DISTANCE` (10 m).
Save the **seed**, not the schedules.

## The spawner

| Item | Purpose |
|---|---|
| `NpcSpawner::new(config)`, `config()`, `set_config()` | |
| `choose_spawn(streamer, center, index) -> Option<(NpcKind, Vec3)>` | a kind and a ground position within `spawn_radius`, deterministic per `(seed, index)` |
| `kind_for(streamer, position, index) -> NpcKind` | the kind a place implies |
| `classify(streamer, position) -> SpawnKind` | `Interior`, `Road`, `Town` or `Wilderness` |

A spawn point is somewhere an agent can stand (resident chunk, walkable tile,
above the waterline, not inside a building or prop), and **the place picks the
kind**: `Interior` when a building contains the point, then `Road`
(`is_road_at`), then `Town`, else `Wilderness` — a non-resident chunk reports
`Wilderness`, "the answer that claims the least". `choose_spawn` draws a preferred
place from fixed `PLACE_WEIGHTS` and retries, falling back to any usable point and
finally to a golden-angle spiral, all from `RngStream::indexed(seed, "npc/spawn",
index)`, so a repopulated town gets the same people in the same places.

## Integration with physics and the scene

| Layer | Owns |
|---|---|
| `noxel-npc` | agent state, tiers, pathfinding, steering, schedules, tier-0 capsules |
| the game | one `Scene` instance per agent, materials per kind, flags per tier |
| `noxel-physics` | the solver; the application calls `step` itself |

Keep one instance per agent keyed by `agent.id.index()`, created on first sight
and removed when the id disappears (or the crowd grows without bound as the player
walks). Give every agent `InstanceFlags::character()`, with `occluder = false`
below the near tier, or 400 agents put 400 boxes into the camera-occlusion ray
bundle every frame. `capsule_center(height)` and `to_transform()` are the two
calls a mirror needs.

## A worked "1 000 NPCs in a town"

**Measured: mean 1.26 ms per step, 1.26 µs per agent, worst 2.16 ms, 0.9 MB**, for
1000 agents over 300 steps at a 60 Hz timestep. That is the number the whole crate
is designed around — at the engine's 4 ms NPC budget, and using the rule "at a
4 ms budget, everything up to `4000 / us_per_agent` agents is free", roughly 3000
agents fit.

The harness is `crates/noxel-npc/tests/crowd.rs`, and in outline it is:

```rust,no_run
use noxel_npc::{NpcConfig, NpcContext, NpcSystem};

let mut system = NpcSystem::new(NpcConfig {
    target_population: 1_000,
    spawn_radius: 60.0,
    despawn_radius: 80.0,
    ..NpcConfig::default()
});
// ... build a streamer and a physics world, then step ~600 times ...
// let stats = system.update(&mut ctx);
// assert_eq!(stats.active, 1_000);
// assert_eq!(stats.tier_counts.iter().sum::<usize>(), 1_000);
// assert!(stats.us_per_agent * stats.active as f32 <= 8.0,
//         "1000 agents cost {:.2} ms", stats.last_step_ms);
```

Assert the **population and the tier split first**, then a generous bound
expressed through `us_per_agent`, because the per-agent cost is what the budget is
about. A test that asserts "1 000 agents in 6.4 ms" records one laptop
(`docs/08-performance.md`). Counters that reveal a regression without timing
anything: `paths_computed` and `path_nodes_expanded` (flat once fields are warm),
`flow_fields_cached`, `blocked`, and the `tier_counts` split.

## Performance knobs, in the order they buy the most

1. **Tier radii and intervals** — `Near` → `Mid` changes the decision rate from
   every step to four times a second, and off tier 0 an agent stops colliding.
2. **Flow fields instead of per-agent A\*** — per-destination instead of
   per-agent search; watch `flow_fields_cached` and the path counters.
3. **Per-step path budgets** — `MAX_PATHS_PER_STEP`, `MAX_NODES_PER_STEP` and
   `PATH_CACHE_CAPACITY` cap the worst frame, and `repath_interval` (1.5 s) caps
   how often an agent asks again.
4. **`node_size`** — a 2 m lattice is a quarter of the nodes of a 1 m one, and
   `Path::simplify` smooths the result anyway.
5. **`target_population` versus `spawn_radius`** — population multiplies
   everything; the radius decides how much of it is on screen.
6. **`collides` per tier and `occluder = false` below it** — a capsule and an
   occlusion volume per agent buy little for agents nobody can touch.
7. **`Steering::build_neighbours` cell size** — O(n) instead of O(n²) per frame.
8. **Scene instance churn** — pool instances and toggle `Instance::visible`
   rather than despawning and re-spawning every frame.

`NpcSystem` clamps `dt` to `MAX_DT` (0.1 s); feeding it the real frame delta
instead of the fixed step makes everything above nondeterministic.

## Common mistakes

- **Passing the frame delta as `dt`.** The crowd is tuned against the fixed step
  and clamps at `MAX_DT`; a variable step breaks determinism.
- **Spawning outside the streamed world.** Non-resident chunks are impassable, so
  such an agent can never path anywhere: it stands still and looks broken.
- **Hand-editing one tier radius.** Run `TierConfig::sanitised()` (the system does
  it); an unordered config otherwise yields agents that never leave tier 3.
- **Treating `agent.position` as the capsule centre.** It is the **feet**
  position; use `capsule_center(height)` for physics and `to_transform()` for the
  render instance.
- **Calling `PhysicsWorld::step` inside the crowd update**, or expecting `send_to`
  to path immediately (it is served on the next `update`, within the step budget).
- **Forgetting that deep water is impassable, not expensive.** Water cost applies
  only below `SHALLOW_WATER_DEPTH`; anything deeper is impassable.
- **Assuming A\* is optimal.** `heuristic_weight` defaults to 1.1; assert a path
  *reaches the goal without cutting corners* instead.
- **Storing schedules per agent, or writing a day that skips an hour.** `[0, 24)`
  must be covered exactly once; save the seed, not the schedules.
- **Clearing the crowd and expecting ids to stay valid**, or measuring the crowd
  with a duration instead of asserting the population, tier split and counters.
