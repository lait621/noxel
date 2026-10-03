# Architecture

Every crate, every module, and how a frame flows through them.

Read `00-overview.md` first for the five ideas; this document is the map.

---

## The dependency graph

Dependencies point **one way only** — downwards in this diagram. Nothing below
knows anything about anything above it.

```text
                          noxel-app
                 App, AppContext, Plugin, frame loop
                              |
     +----------+----------+--+--+----------+-----------+
     |          |          |     |          |           |
 noxel-npc  noxel-world  noxel-  noxel-   noxel-     noxel-
            (generation) physics  camera   debug      visibility
     |          |          |     |          |           |
     |          |          |     |          |           |
     +----------+----------+-----+----------+-----------+
                              |
                        noxel-render
                    (scene, raster, raytrace)
                              |
             +----------------+----------------+
             |                                 |
         noxel-ecs                        noxel-asset
             |                                 |
             +----------------+----------------+
                              |
                          noxel-core
```

Two rules keep this useful:

1. **A crate may only use crates below it.** If a lower crate needs something
   from a higher one, the abstraction belongs in the lower crate.
2. **A crate may be understood by reading it and the crates below it.** That is
   the property that makes the codebase navigable and makes parallel work
   possible.

`noxel-core` depends on nothing but `std`. `noxel-render` and `noxel-world` are
independent of each other: the renderer does not know what a chunk is, and the
world does not know what a mesh is. The app connects them.

---

## Crate by crate

### `noxel-core` — the substrate

Nothing here knows about games. Everything here is a data structure or a
function that would be equally at home in any simulation.

| Module | Contents |
|---|---|
| `math::vec` | `Vec2`, `Vec3`, `Vec4`; `Vec3::from_yaw` |
| `math::scalar` | `EPSILON`, `wrap_angle`, `angle_delta`, `rotate_towards`, `damp`, `damp_factor`, `tonemap_aces`, `linear_to_srgb`, `srgb_to_linear`, `div_floor`, `rem_euclid_i32` |
| `math::mat` | `Mat3`, `Mat4` (column-major, `get(row, col)`), `look_at_rh`, `perspective_rh` (depth `0..1`), `orthographic_rh`, `inverse` |
| `math::quat` | `Quat`: yaw/extraction, YXZ Euler, slerp, `rotate_towards`, `look_rotation` |
| `math::shapes` | `Aabb` (slab ray test, `safe_inv`), `Sphere`, `Ray` (Möller–Trumbore), `Plane`, `Frustum` (positive-vertex test), `Rect`, `Transform` |
| `math::color` | `Color` (linear `f32`), `Color8` (sRGB bytes), `Palette`, `blend_over` |
| `math::noise` | `value_2d`, `perlin_2d`, `simplex_2d`, `worley_2d`, `fbm_2d`, `ridged_2d`, `warped_fbm_2d`, `value_3d`, `tileable_value_2d`, `hash01_2d/3d`, `distance_to_segment_2d` |
| `math::mod` | `GridPos`, `ChunkPos` (**note: `y` is Z**), coordinate conversions |
| `rng` | `Pcg32`, `RngStream` (`for_chunk`, `for_grid3`, `fork`, `value_at`), `ShuffleBag`, FNV-1a hashing |
| `time` | `GameClock` (fixed step, substep cap, `alpha` interpolation), `FrameTiming`, `Stopwatch`, `Profiler` |
| `jobs` | `JobPool` (scoped threads), `parallel_for`, `parallel_chunks`, `join`, `Progress` |
| `pool` | `Handle<T>` (generational), `SlotMap` (`get_two_mut`), `FreeList`, `BitSet` |
| `spatial` | `UniformGrid<T>`, `SpatialHash<T>`, `Bvh<T>` (query ray/AABB/frustum, `any_hit_segment`) |
| `events` | `EventBus<E>` (`emit` this frame, `defer` next), `EventReader`, `EventLog` |

The noise functions are **hash-based, not table-based**: any coordinate is
samplable without its neighbours. That single choice is what makes parallel
world generation possible (`adr/0006-deterministic-generation.md`).

### `noxel-ecs` — entities

A sparse-set store in one flat `Vec<Box<dyn ErasedStorage>>`, which is what
allows safe split borrows for the multi-component iteration methods.

| Module | Contents |
|---|---|
| `entity` | `Entity` (index + generation), `EntityMeta`, conversion to and from a slot-map handle |
| `storage` | `Storage<T>` sparse set: insert, remove, `retain_alive`, change counters |
| `world` | `World`: spawn, despawn, components, resources, `for_each*` families, change detection |
| `scheduler` | `Scheduler<C>`: named stages, systems, per-system statistics |

`World::for_each2_mut2` and friends exist because the borrow checker cannot see
that two component types live in different `Box`es. The world proves it once,
with `pair_mut_by_index`, and the iteration API is safe from then on.

### `noxel-asset` — files

| Module | Contents |
|---|---|
| `json` | insertion-ordered parser and writer, line/column errors, depth limit |
| `png` | from-scratch inflate and deflate, CRC-32, Adler-32, every colour type, filters, `tRNS` |
| `image` | `Image`: the RGBA pixel buffer, blits, flips, rotate, crop, scale, outline, palette snap |
| `texture` | `Texture`: nearest, bilinear and pixel-art sampling, wrap modes, alpha-weighted mips |
| `atlas` | deterministic shelf packing with padding |
| `format` | `TileSet`, `TileDef`, `TileFlags`, `Prefab`, `SpriteFile`, `PaletteFile`, `AssetManifest` |
| `db` | `AssetDb`: resolve, `Arc` cache, hot reload, `write_atomic` |

The PNG codec is a real one — developed against CPython's `zlib` for
cross-verification, with bomb and Adam7 guards.

### `noxel-render` — turning a scene into pixels

| Module | Contents |
|---|---|
| `framebuffer` | linear HDR colour + depth + instance id; `resolve` to sRGB with tone mapping, dithering and palette snap |
| `mesh` | `Vertex`, `Mesh`, and every procedural primitive with verified outward winding |
| `material` | `Material`, `MeshHandle`, `TextureHandle`, `AlphaMode`, the library |
| `light` | `Light` (directional/point/spot), `Falloff`, `Ambient`, `Fog` |
| `scene` | `Scene`: meshes, materials, textures, instances, lights, revision counter |
| `renderer` | `Renderer` trait, `CameraView`, `VisibleSet`, `RenderSettings`, `RenderStats` |
| `raster` | tile binning, band parallelisation, near-plane clipping, fill rule, depth test, shadow map + PCF, transparent pass |
| `raster::post` | gain/lift, desaturate, vignette, scanlines, separable bloom |
| `raytrace` | triangle BVH, deterministic sampling, the integrator, the hybrid pipeline |
| `overlay` | lines, boxes, rays, screen rects, a built-in 3x5 font, panels |

### `noxel-camera`, `noxel-visibility`

| Crate | Module | Contents |
|---|---|---|
| camera | `lib` | `TopDownCamera`: focus, deadzone, smoothing, look-ahead, bounds, `PixelPerfect` |
| camera | `shake` | `CameraShake`: trauma, decay, deterministic hash noise |
| camera | `zone` | `CameraZone`, `ZoneBlend`: per-region overrides with a cross-fade |
| visibility | `cull` | `Culler`, `CullSettings`, `CullReason`: frustum, distance, screen size |
| visibility | `lod` | `LodLevels`, `LodSelection`: levels by screen radius, plus an update fraction |
| visibility | `occlusion` | `OcclusionIndex` over merged occluder volumes, `FadeTable` |

### `noxel-physics` — movement

| Module | Contents |
|---|---|
| `shape` | `ColliderShape`, analytic ray tests, support functions |
| `body` | `Body`, `BodyDesc`, `BodyKind`, `BodyHandle`, `LAYER_*` |
| `narrow` | closest features, 15-axis OBB SAT, capsules, contact generation |
| `solver` | sequential impulses, Coulomb friction, Baumgarte projection |
| `query` | `QueryFilter`, `RaycastHit`, rays, overlaps, conservative-advancement sweeps |
| `character` | `CharacterMove` and `move_character`: slide, step-up, slope limit, ground snap, platform carry |
| `world` | `PhysicsWorld`: broadphase, integration, sleeping, events, statistics |

### `noxel-world` — the world

| Module | Contents |
|---|---|
| `biome` | `BiomeId`, `Biome`, `BiomeTable` |
| `chunk` | `Chunk`: tiles, heights, slopes, colliders, props, buildings |
| `gen` | `WorldConfig`, `WorldGenerator`, `GenStats` |
| `road` | `RoadSegment`, `RoadNetwork`: the macro lattice |
| `town` | `TownPlan`, `TownStyle`, `BuildingPlot`, `BuildingInstance` |
| `stream` | `WorldStreamer`: LRU chunk cache and the queries built on it |

### `noxel-npc` — crowds

| Module | Contents |
|---|---|
| `agent` | `NpcAgent`, `NpcId`, `NpcKind`, `NpcState`, `NpcStats` |
| `crowd` | `CrowdManager`, `CrowdTier`, `TierConfig` |
| `path` | `Pathfinder` (A* over the walkable grid), `Path` |
| `flow` | `FlowField`, `FlowFieldCache` |
| `steering` | separation, alignment, cohesion, seek, obstacle avoidance, road bias |
| `schedule` | `DailySchedule`, `ScheduleEntry`, `Activity` |
| `spawn` | `NpcSpawner`: where and what to spawn, deterministically |

### `noxel-debug`, `noxel-app`

| Crate | Module | Contents |
|---|---|---|
| debug | `stats` | `Rolling` windows with percentiles, `Budget`, `Counter`, `FrameSample`, `Stats` |
| debug | `overlay` | `DebugPanel`, `FrameGraph`, `PanelSlot` |
| debug | `dump` | `FrameDumper` (PNG/depth/raw), `ImageDiff`, `compare`, `diff_image` |
| app | `app` | `AppConfig`, `AppContext`, `InputState`, `App` |
| app | `plugin` | `Plugin`, `PluginRegistry` |
| app | `runner` | `RunMode`, `HeadlessReport`, `App::step`/`run_headless` |

---

## A frame, in detail

```text
App::step(frame_dt)
│
├── debug.begin_frame(frame_dt)
│
├── fixed_update(frame_dt)
│   │  clock.begin_frame(frame_dt)
│   │  while clock.step() {                       // 0..max_substeps times
│   │      plugins.update(app, FIXED_DT)          // gameplay
│   │      physics.step(FIXED_DT)
│   │  }
│   └  returns the number of substeps
│
├── frame_update(frame_dt)
│   ├── plugins.frame(app, frame_dt)
│   ├── plugins.pre_cull(app, frame_dt)           // move the camera here
│   ├── camera.follow_with_velocity(focus, vel, frame_dt)
│   ├── view = camera.view(aspect)
│   ├── streamer.update(focus)                    // load/unload chunks
│   └── visible = visibility.update(input, settings)
│
├── render()
│   └── raster | hybrid | raytrace -> framebuffer
│
├── draw_overlays()
│   ├── plugins.draw(app, &mut framebuffer)
│   └── debug.draw(&mut framebuffer, Some(&view))
│
├── dump_frame(framebuffer)                       // if a dumper is attached
└── debug.end_frame(frame_ms)
```

### Why this order

**Gameplay before the camera.** The camera reads the player's position, so the
player has to have moved first.

**Camera before streaming.** The streamer loads chunks around the camera. Run it
first and it loads around last frame's camera, so the world arrives a frame late
and the player sees chunks appear behind them at speed.

**Streaming before visibility.** Visibility culls against the camera's frustum
*and* the occlusion index. Culling a chunk that has not loaded is meaningless,
and culling against a scene that is about to change wastes the work.

**Visibility before rendering.** The renderer takes the `VisibleSet` as an
argument; it never decides for itself what to draw. That is what lets the
visibility system be tested on its own, and what lets a GPU renderer be added
without touching it.

**Overlays last.** They draw into the framebuffer after the image is shaded, so
they cannot be occluded or lit. The resolve then converts the whole thing to sRGB
in one pass.

**Fixed updates inside the frame, not the other way round.** A frame that took a
second must not become sixty physics steps. `GameClock` caps the substeps and
reports the time it dropped.

---

## Threading

Noxel uses `std::thread` through `JobPool` and nothing else. There is no async
runtime and no thread-local state.

Two places are parallel today:

- **Chunk generation.** Deterministic by construction (`adr/0006`), so chunks can
  be generated in any order, in parallel, with no coordination.
- **Rasterization.** The framebuffer is split into disjoint row bands; each band
  rasterizes its own tiles into its own slices. No locks, no atomics on the pixel
  path, and no floating-point reassociation, so the parallel and sequential
  results are bit-identical — asserted by test.

`JobPool::parallel_for` takes `&mut [T]` and hands each worker a disjoint `&mut`,
so the borrow checker proves the absence of data races rather than a reviewer
having to.

---

## Extension points

| You want to | Implement |
|---|---|
| A new renderer (GPU, headless, ASCII) | `renderer::Renderer` |
| Gameplay | `app::Plugin` |
| A new component | any `'static` type, via `World` |
| A new physics shape | `shape::ColliderShape` (and a narrowphase arm) |
| A new biome | `biome::BiomeTable` entry |
| A new block/tile type | a `TileDef` in a `TileSet` asset |
| A new camera behaviour | `camera::zone::CameraZone`, or a plugin's `pre_cull` |
| A new culling rule | a stage in `visibility::VisibilitySystem::update` |

---

## Common mistakes

**"I added a dependency from `noxel-render` to `noxel-world`."**
That is a cycle through `noxel-app` and it will not compile. The directional rule
is not stylistic: it is what keeps the world generator independent of the
renderer, which is what lets a headless statistics run exist.

**"I put gameplay in `Plugin::draw`."**
`draw` runs once per rendered frame; `update` runs once per *fixed* step, and a
frame may contain zero or several. Gameplay in `draw` runs at the wrong rate and
breaks determinism.

**"I read `Scene` in `Plugin::pre_cull` and mutated it in `Plugin::draw`."**
Between them the visibility pass ran. Mutating the scene after culling means the
`VisibleSet` describes a scene that no longer exists. Mutate in `pre_cull`, or
accept a frame of latency.

**"I made `World` a resource."**
`World` owns the storages; a `Resource` lives inside it. The two are distinct
concepts and mixing them produces a borrow error that reads as a type error.
