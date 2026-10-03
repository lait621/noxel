# Physics

`noxel-physics` is a fixed-timestep, deterministic, safe-Rust rigid-body engine
tuned for a top-down RPG: a player who feels good to move, a town full of props
that settle instead of jittering, and results that replay bit-for-bit.

## `PhysicsWorld` lifecycle

```rust
use noxel_core::math::{Aabb, Vec3};
use noxel_physics::{BodyDesc, ColliderShape, PhysicsConfig, PhysicsWorld};

let mut world = PhysicsWorld::new(PhysicsConfig::default());
world.insert_static_aabb(
    Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)),
    0, // user_data, echoed back by query hits
);
let crate_ = world.insert(
    BodyDesc::dynamic(ColliderShape::Box { half_extents: Vec3::splat(0.5) })
        .at(Vec3::new(0.0, 4.0, 0.0)),
);

for _ in 0..240 {
    world.step(1.0 / 60.0); // dt clamped to `max_step_delta`
}
assert!((world.body(crate_).unwrap().position.y - 0.5).abs() < 0.01);
```

| Method | Notes |
|---|---|
| `new(config)` | the config is sanitised (see the tuning table) |
| `insert(BodyDesc) -> BodyHandle` | `BodyHandle::INVALID` once `max_bodies` (4096) is reached — always check `contains()` |
| `insert_static_aabb(Aabb, user_data)` | the usual way to turn level collision into physics geometry |
| `remove(handle) -> bool` | drops the body, its broadphase entry, its contacts and its pair history |
| `clear()` | removes everything and resets the statistics |
| `body(handle)` / `body_mut(handle)` | `Option<&Body>` / `Option<&mut Body>` |
| `bodies()` | every live body in slot order — the deterministic iteration order |
| `set_position` / `set_rotation` / `set_velocity` | validate, wake the body, refresh the broadphase immediately |
| `translate_kinematic(handle, delta)` | moves a kinematic (or dynamic) body; refuses static |
| `set_gravity(Vec3)`, `step(dt)` | a non-finite or non-positive `dt` is ignored entirely |
| `contacts()`, `events()`, `clear_events()`, `stats()`, `last_step_delta()` | per-step readouts |

## `BodyDesc` and `BodyKind`

```rust
use noxel_core::math::{Quat, Vec3};
use noxel_physics::{BodyDesc, ColliderShape, LAYER_ALL, LAYER_PROP};

let desc = BodyDesc::dynamic(ColliderShape::Sphere { radius: 0.5 })
    .at(Vec3::new(1.0, 2.0, 3.0))
    .with_velocity(Vec3::X)
    .with_mass(2.0)
    .with_friction(0.4)
    .with_restitution(0.2)
    .with_damping(0.1, 0.2)
    .with_gravity_scale(1.0)
    .with_layer(LAYER_PROP, LAYER_ALL)
    .with_rotation(Quat::IDENTITY)
    .with_user_data(9);
let _ = desc;
```

| Builder | Default | Effect |
|---|---|---|
| `dynamic(shape)` / `static_body(shape)` / `kinematic(shape)` | — | the kind (`static` is spelled `static_body`) |
| `at(Vec3)` | `Vec3::ZERO` | world position |
| `with_rotation(Quat)` | identity | orientation; boxes use it in full |
| `with_velocity` / `with_angular_velocity` | zero | initial velocities |
| `with_mass(f32)` | `volume * DEFAULT_DENSITY` (1 kg/m³), min 1e-3 | ignored for static and kinematic bodies |
| `with_friction(f32)` | `0.5` | Coulomb coefficient, clamped to `[0, 1]` |
| `with_restitution(f32)` | `0.0` | bounciness, clamped to `[0, 1]` |
| `with_damping(linear, angular)` | `0, 0` | exponential per-second decay |
| `with_gravity_scale(f32)` | `1.0` | `0` makes the body float |
| `with_layer(layer, mask)` | `LAYER_WORLD`, `LAYER_ALL` | |
| `with_user_data(u64)` | `0` | echoed by query hits and contacts |
| `as_sensor()` | off | reports overlaps, never pushes back |
| `sleeping(bool)` | off | only dynamic bodies can actually sleep |

`Body` exposes all of it as public fields (`position`, `rotation`,
`linear_velocity`, `angular_velocity`, `mass`, `inv_mass`, `restitution`,
`friction`, `linear_damping`, `angular_damping`, `gravity_scale`, `is_sensor`,
`sleeping`, `sleep_timer`, `layer`, `mask`, `user_data`, `shape`) plus `aabb()`,
`is_dynamic()`/`is_static()`/`is_kinematic()`, `can_collide_with()`, `wake()` and
`apply_impulse()`.

| `BodyKind` | Gravity | Impulses | Pushed by others | Sleeps |
|---|---|---|---|---|
| `Dynamic` | yes | yes | yes | yes |
| `Static` | no | no (`inv_mass` 0) | no | never |
| `Kinematic` | no | no | no — it pushes dynamic bodies | never |

`BodyDesc::build` sanitises before the body exists: a non-finite position,
velocity or rotation becomes zero/identity, friction and restitution are
clamped, and a missing or non-positive mass is derived from the shape's volume.

## Colliders: which shapes are exact

```rust
use noxel_core::math::Vec3;
use noxel_physics::ColliderShape;

let cube   = ColliderShape::Box { half_extents: Vec3::splat(0.5) };
let ball   = ColliderShape::Sphere { radius: 0.5 };
let person = ColliderShape::Capsule { radius: 0.3, half_height: 0.5 }; // along local Y
let barrel = ColliderShape::Cylinder { radius: 0.4, half_height: 0.6 };
let _ = (cube, ball, person, barrel);
```

A shape is defined in the **local space of its body** — centred on the body's
origin, placed by its `position` and `rotation`. Every shape has an exact `aabb`,
support function, `contains_point` and ray test. The narrowphase reduces each pair
to a single closest-feature pair:

| Pair | Method |
|---|---|
| sphere / sphere | analytic |
| sphere / box | closest point on the box, including the centre-inside case |
| sphere / capsule | closest point on the axis segment |
| capsule / capsule | closest points between two segments |
| capsule / box | exact closest point between a segment and an AABB |
| box / box | 15-axis SAT with a minimum translation vector |
| **any / cylinder** | **the cylinder is replaced by its local bounding box** (`r`, `hh`, `r`) |

The cylinder is the one documented inexactness: it keeps one consistent contact
model for every non-sphere pair, at the cost of a barrel colliding slightly wider
at the corners than its mesh. Use a capsule or a box when that matters. One
contact point is generated per pair (at the centroid of the incident box's
penetrating vertices for box/box), which is enough at this scale.

### Layers and `QueryFilter`

Layers are single bits, and a pair collides only when **both** agree:
`(self.layer & other.mask) != 0 && (other.layer & self.mask) != 0`. A one-sided
mask is a classic bug — the bodies do not merely stop colliding, they fall through
the floor.

| Constant | Bit | | Constant | Bit |
|---|---|---|---|---|
| `LAYER_WORLD` | `1 << 0` | | `LAYER_TRIGGER` | `1 << 4` |
| `LAYER_PLAYER` | `1 << 1` | | `LAYER_CAMERA` | `1 << 5` |
| `LAYER_NPC` | `1 << 2` | | `LAYER_ALL` | `u32::MAX` |
| `LAYER_PROP` | `1 << 3` | | | |

```rust
use noxel_physics::{QueryFilter, LAYER_NPC};

// `caster` is the handle of the body doing the querying.
let filter = QueryFilter::with_mask(LAYER_NPC).ignoring(caster);
let all = QueryFilter::ALL.with_sensors(false).with_static(true);
let _ = (filter, all);
```

`QueryFilter::default()` sees all solid, non-sensor geometry **including** static
bodies — deliberately not the derived all-zero value, which would reject
everything and let a line-of-sight ray pass through the level.

## The step, in order

1. recover any non-finite state left by gameplay code,
2. integrate velocities (gravity + damping, dynamic awake bodies only),
3. sync the spatial hash and collect overlapping pairs,
4. narrowphase: rebuild the contact list,
5. solve velocities (sequential impulses),
6. integrate positions (semi-implicit Euler, using the solver's velocities),
7. correct positions (Baumgarte projection),
8. wake bodies touched by moving neighbours, then update sleep timers,
9. emit collision/trigger/sleep events,
10. recover again, so nothing non-finite survives the step.

Everything iterates the body slot map in slot order, or a `BTreeSet` of handles;
nothing in the step iterates a `HashMap`. `dt` is clamped to `max_step_delta`
(0.05 s), so a hitch cannot teleport a fast body through a wall.

## The solver

Warm-start free: every step rebuilds the contact set, the effective masses and
the restitution targets, then runs `solver_iterations` (4) Gauss-Seidel sweeps.

- The accumulated normal impulse is clamped non-negative.
- The accumulated tangential impulse is clamped to the Coulomb cone
  `|jt| <= mu * jn`; tangential speed below `FRICTION_EPSILON` (1e-5 m/s) is not
  solved at all.
- **Restitution applies only above `RESTITUTION_THRESHOLD` (1.0 m/s).** Without
  that gate a resting body bounces on its own numerical noise forever.
- **Contacts exchange yaw only.** A crate can never be tipped over by a friction
  impulse at one corner. `angular_velocity`'s X and Z still exist and are
  integrated, so gameplay code can tumble a body deliberately — contacts simply
  never create them.
- Baumgarte correction runs after the velocity sweep with `position_correction`
  (0.2) and a per-contact cap of `MAX_CORRECTION` (0.2 m), so a deeply
  overlapping pair does not teleport apart.

**Sleeping** uses `sleep_linear_threshold` 0.05 m/s, `sleep_angular_threshold`
0.05 rad/s and `sleep_time_required` 0.5 s; on sleeping, velocities are zeroed. A
sleeping body acts as immovable until:

- gameplay calls a world setter, `Body::wake()` or `Body::apply_impulse` (each
  emits `BodyWoke`), or
- an **awake, non-static** neighbour touches it and either the contact is *new*
  this step or that neighbour is still moving after the solve.

The two triggers are what stop a resting stack from buzzing (its contacts persist
and its post-solve velocities are zero) while still catching a projectile whose
impact the solver already absorbed, or a kinematic platform that was already
touching a sleeper when it started moving.

## The character controller

`move_character(handle, desired_motion, up)` sweeps the body's own collider and
moves the body. `desired_motion` is a **displacement** in metres (velocity ×
frame time), not a velocity.

| Constant | Value | Meaning |
|---|---|---|
| `MAX_SLOPE_ANGLE` | 46° | steeper than this acts as a wall |
| `STEP_HEIGHT` | 0.4 m | how high a lip the character walks up |
| `GROUND_SNAP_DISTANCE` | 0.2 m | how far down it looks for ground when already grounded |
| `MAX_SLIDES` | 4 | surface slides per horizontal move |
| `STEP_PROGRESS_EPSILON` | 1e-3 m | a stepped move must beat the direct move by this much |

1. **Up.** The upward part is swept first, so a jump cannot push the character
   through a ceiling (`hit_ceiling` is set instead).
2. **Across.** The horizontal part is swept and slid along up to `MAX_SLIDES`
   surfaces. A *walkable* surface slides along its true normal, which lifts the
   character up a ramp; a too-steep surface cancels only the horizontal component,
   so it cannot be climbed. If the direct move is blocked, the controller retries
   from `STEP_HEIGHT` higher and drops back; the stepped result wins only if it
   makes strictly more progress, and head-room is checked first.
3. **Down.** The downward part plus a ground-snap probe. A non-ascending
   character probes `GROUND_SNAP_DISTANCE` below itself, which keeps it glued to
   floors, stairs and slopes and lets a fresh spawn report `grounded` without a
   separate settling frame. A jumping character is never pulled back down.

**Platform carry:** a character that was standing on a moving body inherits that
body's displacement (`velocity * last_step_delta`), but only when not moving
upwards. That is what makes a kinematic platform carry the player.

`CharacterMove` reports `translation` (what was applied), `velocity`
(`translation / last_step_delta()`, zero before the first step), `grounded`,
`ground_normal`, `ground_body`, `hit_ceiling`, `hit_wall` and `hit_count`.

### A worked controller

```rust
use noxel_core::math::{Aabb, Vec3};
use noxel_physics::{BodyDesc, ColliderShape, LAYER_PLAYER, PhysicsConfig, PhysicsWorld};

let mut world = PhysicsWorld::new(PhysicsConfig::default());
world.insert_static_aabb(
    Aabb::new(Vec3::new(-30.0, -1.0, -30.0), Vec3::new(30.0, 0.0, 30.0)),
    0,
);
// Capsule centre 1.35 m up: radius 0.35 + half_height 1.0.
let player = world.insert(
    BodyDesc::kinematic(ColliderShape::Capsule { radius: 0.35, half_height: 1.0 })
        .at(Vec3::new(0.0, 1.35, 0.0))
        .with_layer(LAYER_PLAYER, u32::MAX),
);

let dt = 1.0 / 60.0;
world.step(dt);
let wish = Vec3::new(1.0, 0.0, 0.0) * 3.4 * dt; // displacement, not velocity
let moved = world.move_character(player, wish, Vec3::Y);
assert!(moved.translation.x > 0.0 && moved.grounded);
```

Notes that save time: the controller is intended for `BodyKind::Kinematic` bodies
(it will move a dynamic one, but the solver then fights it for the position); call
it **after** a `step` so `last_step_delta()` and the carry are current (`App` does
it the other way round — gameplay updates, then `physics.step` — which costs one
frame of staleness in `velocity` and is fine, but be consistent); the body's own
`shape`, `rotation`, `mask` and `layer` are used, sensors are ignored, and the
body itself is always excluded.

## Ray and overlap queries

```rust
use noxel_core::math::{Ray, Vec3};
use noxel_physics::{QueryFilter, RaycastHit};

let ray = Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::DOWN);
if let Some(RaycastHit { body, distance, point, normal, user_data }) =
    world.raycast(&ray, QueryFilter::default())
{
    let _ = (body, distance, point, normal, user_data);
}
```

| Query | Returns |
|---|---|
| `raycast(&Ray, QueryFilter)` | the nearest hit, or `None`. The direction is normalised internally; a ray starting inside a shape reports distance 0 with a normal opposite the ray |
| `raycast_all(&Ray, QueryFilter, &mut Vec<RaycastHit>)` | every hit, sorted by distance then handle |
| `overlap_sphere(center, radius, filter, &mut Vec<BodyHandle>)` | every overlapping body, sorted by handle; touching counts |
| `overlap_aabb(bounds, filter, &mut Vec<BodyHandle>)` | the same for a box |

Every query gathers candidates from the spatial hash and then runs an **analytic**
test against the real shape, so a result is never limited by the broadphase's
approximation; candidates are sorted by handle first, so ties resolve identically
every run. Shape sweeps exist (`sweep_shape`, conservative advancement) but are
crate-private — the public sweeping API is the character controller.

## Events

`PhysicsEvent` accumulates until `clear_events()`, which you should call at the
end of the frame that consumed them:

| Variant | Fired when |
|---|---|
| `CollisionEnter { a, b, point, normal, impulse }` | two solid bodies started touching. `a` is the lower handle, `normal` points from `a` to `b`, and `impulse` is the accumulated normal impulse — a good impact-strength signal for audio or damage |
| `CollisionExit { a, b }` | they stopped touching |
| `TriggerEnter { trigger, other }` / `TriggerExit` | a sensor began or stopped overlapping |
| `BodySlept { body }` / `BodyWoke { body }` | sleep transitions |

Both directions of a pair are reported once, keyed on the canonical (lower,
higher) handle order.

## Determinism

Bodies are visited in slot-map order, broadphase pairs and contacts are sorted by
handle, and previous-frame pair sets are `BTreeSet`s rather than `HashSet`s. No
wall-clock time is read in the step. `determinism_is_bit_exact` runs a scenario
twice and compares every body's state.

## Tuning

`PhysicsConfig` defaults, and the range `sanitized()` clamps into:

| Field | Default | Clamp | Why |
|---|---|---|---|
| `gravity` | `(0, -19.62, 0)` | non-finite → zero | deliberately 2× Earth: a top-down RPG is read at a glance and falls are short, so props should settle fast |
| `broadphase_cell_size` | 4.0 m | 0.05 … 1024 | near the typical body size; changing it rebuilds the hash |
| `contact_tolerance` | 0.005 m | 0 … 0.5 | a small positive margin stops resting bodies flickering between touching and separated |
| `solver_iterations` | 4 | 1 … 64 | |
| `position_correction` | 0.2 | 0 … 1 | higher resolves overlap faster and adds energy |
| `max_step_delta` | 0.05 s | 1e-4 … 1.0 | a hitch must not tunnel |
| `sleep_linear_threshold` | 0.05 m/s | 0 … 1e3 | |
| `sleep_angular_threshold` | 0.05 rad/s | 0 … 1e3 | |
| `sleep_time_required` | 0.5 s | 0 … 1e3 | |
| `max_bodies` | 4096 | ≥ 1 | `insert` refuses beyond it |

## Common mistakes

- **"I wrote `world.body_mut(h).unwrap().position = p` and nothing moved."**
  (a) There is no `world.step(dt)` — nothing integrates without one. (b) The body
  is not `Dynamic`: static and kinematic bodies have `inv_mass == 0`, so gravity
  and impulses are ignored. (c) `insert` returned `BodyHandle::INVALID` because
  `max_bodies` was reached, so `body_mut` returned `None` and everything after it
  was a no-op. Check `world.contains(h)` after every insert.
- **"I moved a body and my raycast still hits the old position."** Writing
  `body.position` through `body_mut` does not refresh the broadphase; use
  `set_position`, which does — exactly what
  `queries_respect_change_after_set_position` asserts.
- **"Two bodies pass through each other."** The mask is one-sided; both bodies
  must list the other's layer.
- **"My crates jitter forever, or a stack slowly sinks."** Restitution is too high
  for the step (it only applies above 1 m/s), or `contact_tolerance` is 0 and the
  solver is losing to gravity with too few iterations.
- **"A barrel collides oddly at the corners."** Cylinders collide as their local
  bounding box; use a capsule for a rounded shape.
- **"The player climbs 45° ramps but not 50° ones, or sticks on a kerb."**
  `MAX_SLOPE_ANGLE` is `to_radians(46.0)` and inclusive; a lip taller than
  `STEP_HEIGHT` (0.4 m) is a wall by design.
- **"The player falls through a moving platform."** Carry needs the character to
  have been standing on that body, not moving upwards, and uses the platform's
  `linear_velocity * last_step_delta()` — a platform moved with
  `translate_kinematic` has no velocity.
- **"Events fire every frame."** Call `clear_events()` at the end of the frame
  that read them.
- **"Setting `max_step_delta` higher made fast bodies tunnel."** The clamp is a
  safety net: the *solver* is discrete, so a body moving more than its own
  thickness per step can pass through thin geometry.
