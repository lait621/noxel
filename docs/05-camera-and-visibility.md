# Camera and visibility

`noxel-camera` builds the view; `noxel-visibility` decides what is worth drawing
inside it. This guide covers both, plus the debugging workflow for a missing
object.

## `TopDownCamera`

The rig holds *intent* — where the player is, how far the camera lags, how much
it shakes — and produces a `CameraView` on demand. It never touches the scene, so
a cutscene, an interior and the gameplay camera are the same type configured
differently.

```rust
use noxel_camera::TopDownCamera;
use noxel_core::math::Vec3;

let mut camera = TopDownCamera::pixel_art(320, 180); // straight down, pixel snapping
camera.set_ortho_height(20.0);                       // 20 m of world on screen
camera.set_smoothing(0.05);                          // 5% of the gap left per second
camera.set_deadzone(1.6, 1.2, 0.0);                  // world units
camera.set_look_ahead(0.22, 2.5);                    // seconds of motion, capped in metres
camera.set_clip_planes(1.0, 300.0);

// Once per frame, after the player has moved:
camera.follow_with_velocity(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 1.0 / 60.0);
let view = camera.view(320.0 / 180.0);
```

| Constructor | Result |
|---|---|
| `new(ProjectionMode)` | focus at the origin, pitch straight down, distance 40 m, smoothing 0.05, no deadzone or look-ahead, near 0.1, far 400 |
| `pixel_art(w, h)` | orthographic `height: 20.0` plus `PixelPerfect::at(w, h)` |
| `perspective_tilted(fov_y, pitch)` | a perspective rig at the given pitch |

### Follow, deadzone, smoothing

`follow_with_velocity(target, velocity, dt)` does four things in order:

1. **Look-ahead** — `velocity * look_ahead`, capped at `max_look_ahead` metres. A
   small lead (0.1–0.3 s) keeps a running character centred instead of dragging
   at the deadzone edge.
2. **Deadzone** — the focus stays put until the target leaves a box of
   `deadzone.x` by `deadzone.y` (plus `deadzone_y` vertically), then moves only
   to that box's edge. Without one, a moving target looks like it is dragging the
   camera on a rubber band.
3. **Smoothing** — `focus += (desired - focus) * damp_factor(smoothing, dt)`.
4. **Rotation**, when enabled — the short way round, by a fraction of the
   *remaining* angle.

`damp_factor(smoothing, dt)` is `1 - smoothing^dt`, which makes the smoothing
frame-rate independent: two half-second steps leave the same residue as one
one-second step. Two edge cases are deliberate — `dt <= 0` returns 0 (no time has
passed, so nothing may move) and `smoothing <= 0` returns 1 (snap). A
`damp_factor(0)` that returned "never move" was a real bug the tests caught; if
you write your own smoothing, use this function rather than `1 - powf(..)`.

```rust
use noxel_core::math::damp_factor;

let one = 1.0 - damp_factor(0.1, 1.0);
let two = (1.0 - damp_factor(0.1, 0.5)).powi(2);
assert!((one - two).abs() < 1e-5);
```

### Yaw, pitch, distance, zoom, bounds

- `set_yaw(yaw)` sets a target and enables rotation; `lock_rotation()` freezes it;
  `set_yaw_immediate(yaw)` writes it without smoothing (zones use this, because
  their cross-fade already spans frames and a second smoothing stage would make
  the camera arrive at a different time than the zoom).
- `set_pitch(p)` clamps to `[0.15, PI/2]`. Straight down is the pixel-art
  default; below about 9° a top-down map becomes unreadable, so it is clamped.
- `set_distance(d)` clamps to `[1, 1000]`; `zoom_by(factor)` multiplies the
  orthographic height and clamps to `[2, 200]` — a zoom wheel must be
  multiplicative or it feels wrong at both ends.
- `set_eye_offset(h)` raises the eye without changing the direction it looks.
- `set_bounds(Some(aabb))` keeps the visible rectangle inside a region: per axis,
  it clamps when the region is bigger than the view and centres the view when it
  is smaller.

### `PixelPerfect` and `view`

`view(aspect)` is the only method that mutates snapped state, so call it once per
frame and pass the result to both visibility and the renderer — they must agree.

- A non-finite focus is recovered to the origin rather than propagating into the
  view-projection matrix (which would blank the frame).
- `snap_focus` quantises the focus to whole internal pixels:
  `units_per_pixel = ortho_height / internal_height`. Orthographic only.
- The shake offset is sampled *before* snapping, so a shake does not fight the
  pixel grid.
- Near straight down (`pitch > PI/2 - 0.02`) world up is parallel to the view
  direction and `look_at_rh` cannot build a basis from it; the rig substitutes
  the yaw-derived heading, so turning the camera turns the map instead of
  snapping the roll.
- `world_units_per_pixel()`, `snapped_focus()`, `eye_position()` and
  `visible_ground_rect(aspect)` (widened along the view axis by
  `min(1/sin(pitch), 4)` when tilted) are the readouts the rest of the engine
  uses.

`PixelPerfect::at(320, 180)` enables `snap_focus` and `snap_sprites`;
`PixelPerfect::OFF` disables both. `snap_to_pixels(world_size, units_per_pixel)`
rounds a *size* while leaving position continuous — the fix for a sprite
alternating between 15 and 16 px as it moves.

### Shake

Trauma-based: `add_trauma(amount)` saturates at 1, trauma decays linearly, and
the offset is `trauma²` scaled by per-axis hash noise. Squaring is what makes a
shake start sharply and end gently; a linear falloff feels like the camera is
being pushed.

| Setting | `new()` | `explosive()` | `rumble()` |
|---|---|---|---|
| `max_offset` | 0.55 m | 1.1 m | 0.18 m |
| `max_roll` | 0.035 rad | 0.09 rad | 0.008 rad |
| `frequency` | 18 Hz | 24 Hz | 9 Hz |
| `decay` | 1.25 /s | 1.6 /s | 0.9 /s |

The noise is `shake_noise(step, axis, seed)` indexed by a step counter, not by
wall time, so a recorded trauma event replays the exact same motion. `noise1d`
interpolates between adjacent hashes with a smoothstep, so the shake reads as a
rumble rather than per-frame static. `roll()` is separate from `offset()`: a
little roll is what separates a convincing shake from an unsteady translation.

### Zones

A `CameraZone` is an axis-aligned region plus the settings it overrides:
`projection`, `yaw`, `pitch`, `smoothing`, `zoom`, `camera_bounds`,
`fixed_focus`. `ZoneKind` (`Follow`, `Fixed`, `LockedYaw`, `Interior`,
`Cutscene`) labels it for overlays and scripting.

```rust
use noxel_camera::TopDownCamera;
use noxel_camera::zone::{CameraZone, ZoneBlend, apply_zone};
use noxel_core::math::{Aabb, Vec3};

let mut camera = TopDownCamera::pixel_art(320, 180);
let house = CameraZone::interior(
    "house",
    Aabb::new(Vec3::new(10.0, -5.0, 10.0), Vec3::new(30.0, 20.0, 30.0)),
    0.0,
);
let mut blend = ZoneBlend::new();
blend.update(std::slice::from_ref(&house), Vec3::new(20.0, 1.0, 20.0), 1.0 / 60.0);
if blend.amount() > 0.0 {
    apply_zone(&mut camera, &house, blend.amount());
}
```

`select_zone(zones, point)` picks the highest-priority zone containing the point
and breaks ties by the *smaller* region — what you want when a "doorway" zone sits
inside a "town" zone. `ZoneBlend` smooths towards "in a zone" with a 0.1% snap at
both ends so the camera does not chase a difference nobody can see. Blending is
deliberate: snapping between an outdoor and an indoor zoom is the most jarring
thing a top-down camera can do.

## The three-stage culler

`VisibilitySystem::update(&VisibilityInput, &RenderSettings) -> VisibleSet`
refines the candidate set in three stages, each of which can only *remove*.

**Phase 1, per instance, in scene order:**

| Order | Test | `CullReason` |
|---|---|---|
| 1 | `!instance.visible` | `Hidden` |
| 2 | `visible_layers & instance.layer == 0` | `LayerDisabled` |
| 3 | `instance.bounds().is_empty()` | `TooSmall` (counted, no `culled` entry) |
| 4 | behind the near plane | `NearPlane` |
| 5 | outside the frustum (after `frustum_margin`) | `Frustum` |
| 6 | past the layer's distance limit | `Distance` |
| 7 | screen radius below `min_screen_radius` | `TooSmall` |
| 8 | otherwise | drawn, with its LOD |

`flags.always_visible` skips every test.

**Phase 2 — occlusion:** each survivor is tested with
`OcclusionIndex::is_occluded` when `cull_occluded` is on; a hidden one becomes
`Occluded`. `always_visible` is exempt.

**Phase 3 — fades:** blockers between camera and focus fade down, and each drawn
item's `alpha` is filled from the fade table. This stage removes nothing.

`CullCounts` accumulates `considered`, `drawn`, `frustum`, `near_plane`,
`distance`, `too_small`, `occluded`, `layer_disabled`, `hidden` and `faded`, with
`rejected()`, `draw_ratio()` and `summary()`. `CullReason::label()` gives the
short overlay text: `drawn`, `frustum`, `near`, `far`, `small`, `occluded`,
`layer`, `hidden`.

| `CullSettings` | Default | Meaning |
|---|---|---|
| `max_distance` | 250 m | drop anything further from the camera |
| `layer_distances` | all 0 | per-layer overrides; `0` means "use `max_distance`" (`set_layer_distance(bit, m)`) |
| `min_screen_radius` | 0.45 px | drop anything smaller on screen |
| `reference_viewport_height` | 180 | used when the camera reports no `world_units_per_pixel` |
| `frustum_margin` | 0.0 | expands the frustum so an object does not pop in at the edge |

`Culler::units_per_pixel` is constant for orthographic and proportional to
distance for perspective; `screen_radius` divides the bounding sphere radius by
it. The 0.45 px default sits at the pixel boundary on purpose: sub-pixel geometry
costs more to set up than it can contribute. `VisibilityConfig::large_world()`
sets a 160 m view distance; `exhaustive()` disables distance, size and occlusion
culling for golden-image tests.

## The occlusion index

`OcclusionIndex` answers two questions against one BVH of **merged occluder
volumes** — one box per building run or tree canopy, never one per voxel or
triangle, because a fade is per object (half a roof is meaningless) and
`noxel-world` emits them merged for exactly this reason. It rebuilds only when
`Scene::revision()` moves, so a static world pays `O(n log n)` once.

**Camera occlusion** casts a bundle, not a ray: one centre ray plus four at the
corners of a square of half-extent `focus_radius` (0.45 m), aimed 0.4 m above the
focus, with `ray_margin` (0.05 m) added so an occluder exactly at the focus does
not flicker. A character is a box, not a point — a roof covering one shoulder
must still fade, and a single centre ray would miss exactly the cases players
notice. `collect_blockers` returns a sorted, deduplicated handle set.

**Occlusion culling** casts from the camera to the object centre, stopping
`bounding_sphere_radius * 0.5` short so an object touching a wall does not occlude
itself through the shared boundary. Only objects at most
`cull_max_screen_radius` (24 px) are considered — culling a large object saves
little and pops visibly — and the object's own handle is excluded.
`OcclusionSettings::fade_only()` is the safe configuration while a game is being
built: fading works, nothing disappears.

| `FadeSettings` | Default | Meaning |
|---|---|---|
| `min_alpha` | 0.25 | how transparent a blocker becomes — **not** 0 |
| `speed` | 0.02 | fraction of the remaining alpha distance left after one second |
| `enabled` | true | master switch |

The fade is smoothed rather than snapped because a roof that stops blocking must
not become opaque in one frame, and an object that is *still fading* keeps fading
even when the ray no longer touches it — otherwise it pops the moment the player
steps out from under it. `min_alpha: 0.25` keeps a faint outline that still reads
as a roof; fully hidden geometry in a top-down game is disorienting. `FadeTable`
exposes the same smoothing for a game that drives fades from its own logic, and
`VisibilitySystem::fade_of(handle)` / `fading_count()` read it.

## LOD by screen radius

Thresholds are in **screen-space pixels of bounding radius**, not distance,
because that is what decides whether a swap is visible: a large object far away
and a small object nearby can need the same treatment.

| `LodLevels` | Default | Meaning |
|---|---|---|
| `full` | 12.0 px | `>= full` is LOD 0 |
| `medium` | 5.0 px | `>= medium` is LOD 1 |
| `low` | 1.5 px | `>= low` is LOD 2; below is LOD 3 |
| `viewport_height` | 180 | world-to-pixel conversion |
| `distance_bias` | 0.0 | extra distance per level; 0 disables biasing |

The renderer draws one mesh per instance, so LOD does not swap meshes; it tells
the rest of the engine how much an instance is worth spending on (the NPC system
drops distant agents to a cheaper tier; streaming decides what to keep; a game can
swap meshes at `lod >= 2`). `LodSelection::update_fraction_for` gives 1.0, 0.5,
0.25 and 0.1 for levels 0–3, and `should_update(instance_id, frame)` returns true
when `(frame + instance_id) % period == 0` — a reduced update rate with no
per-agent bookkeeping and no flicker.

## Drawing all of it

Everything is off by default, because each wireframe costs a query:

```rust
use noxel_debug::{DebugConfig, DebugSystem};

let mut debug = DebugSystem::new(DebugConfig::verbose()); // camera panel + wireframes
// Per frame, after the visibility pass:
// debug.draw(&mut framebuffer, Some(&view));
// debug.draw_occlusion(&mut framebuffer, &view, focus, &blocker_bounds);
// debug.draw_boxes(&mut framebuffer, &view, &collider_bounds);
// debug.draw_marker(&mut framebuffer, &view, player_position, 0.6, Color8::RED);
```

| Call | Shows |
|---|---|
| `draw` | statistics panel, frame-time graph, culling counters, optionally the camera line |
| `draw_occlusion` | a yellow camera-to-focus line and a magenta box per blocking volume |
| `draw_boxes` | cyan boxes, for the collision visualiser |
| `draw_marker` | a cross at a world position |

`VisibilitySystem::summary()` is the one-line readout (`visible N | drawn/considered
(...) | occluders N | fading N`). To see the rig rather than the culling, box every
`instance.flags.occluder` whose `fade_of(handle)` is below 1 — that shows *why* a
roof is see-through.

## Why is my object invisible?

1. **Is it in the scene?** `Scene::instance(handle)` returning `None` means the
   handle was removed or never created; `instance_count()` and `summary()` tell
   you what is there.
2. **`instance.visible`?** `false` is `Hidden`, and the ray tracer also skips it.
3. **Layer?** `visible_layers & instance.layer == 0` is `LayerDisabled` — this is
   how a roof layer is hidden indoors. Check `VisibilityInput::with_layers`.
4. **Empty mesh?** Empty meshes are skipped and give empty bounds, which the
   culler counts as `TooSmall` *without* a `culled` entry: absence from `culled`
   does not mean the object passed.
5. **Stale bounds?** `Instance::bounds` is private and refreshed by
   `Scene::update_bounds`/`update_all_bounds`. If you wrote `instance.transform`
   directly, the culler is testing the old box — usually the one at the origin.
6. **Behind the near plane?** `NearPlane`: `camera.depth_of(centre)` is more
   negative than `-radius - near`.
7. **Outside the frustum?** Set `frustum_margin = 1.0` to distinguish a pop at
   the edge from a permanent absence.
8. **Too far?** `Distance`. `distance_for_layer` uses `layer.trailing_zeros()` as
   its index, so a *combination* of bits silently takes the override of the
   lowest bit. One instance, one layer bit.
9. **Too small?** `TooSmall`: `bounding_sphere_radius / units_per_pixel < 0.45`.
   At 320x180 a 0.2 m object is under a pixel beyond ~5 m.
10. **Occlusion culling?** `Occluded` — confirm with `cull_occluded: false`, then
    check that your occluder volumes are merged and do not cover empty space.
11. **Faded, not gone?** A blocked occluder drops to `min_alpha` 0.25 and is still
    drawn; check `CullCounts::faded` and `fade_of`.
12. **The material?** `Cutout` discards below its threshold, `Blend`/`Additive`
    draw in the second pass without writing depth, and an unlit material with a
    black `base_color` is black. Alpha <= 0.001 is skipped outright.
13. **Drawn but behind something?** `Framebuffer::id_at(x, y)` names the instance
    that owns a pixel, which separates "culled" from "occluded by geometry".

## Common mistakes

- **"The camera jitters as the player moves."** `PixelPerfect::at(w, h)` must use
  the same internal resolution the framebuffer uses, or the snap lands between
  pixels.
- **"The camera lags more at high frame rates."** You smoothed with a hand-written
  `1 - powf(smoothing, dt)` or by multiplying by `dt`. Use `damp_factor`.
- **"`set_smoothing(0)` did not snap."** `damp_factor` returns 0 when `dt <= 0`,
  so a zero-length frame cannot teleport the camera.
- **"The camera lurches entering a building."** You passed `t = 1.0` to
  `apply_zone` instead of `ZoneBlend::amount()`.
- **"Turning the camera rolls the map."** You passed `up = Vec3::Y` at a
  straight-down pitch; use the rig (or its yaw-derived heading).
- **"A whole layer vanished."** `distance_for_layer` is indexed by the layer's
  lowest set bit; an instance should carry exactly one.
- **"The occlusion index rebuilds every frame."** Something calls `Scene::touch()`
  or mutates through a `*_mut` accessor every frame. Any mutation bumps
  `Scene::revision()`, invalidating the BVH and the fade bookkeeping; keep static
  geometry out of the per-frame path.
