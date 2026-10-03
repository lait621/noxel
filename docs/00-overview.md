# Overview

Noxel is a lightweight 3D engine for **top-down pixel-art RPGs**. You describe a
world in three dimensions; the player sees it from above at an internal
resolution of 320x180 (or whatever you choose), upscaled to the window, so the
result reads as pixel art rather than as a low-poly 3D game.

This document is the mental model. It answers "what is this, what is it not, and
what are the five ideas everything else follows from". Read it before the guides.

---

## What Noxel is not

Being explicit about the edges saves everyone time.

| Not this | Because |
|---|---|
| A general-purpose 3D engine | Coordinates, camera and lighting all assume a top-down view. It will render a first-person scene, badly and slowly. |
| A UI toolkit | No menus, no text input, no layout. See `adr/0009-no-ui.md`. |
| A windowing layer | No platform API. You present `Framebuffer` yourself; see `guides/windowing.md`. |
| A model importer | There is no glTF/OBJ loader. Geometry is procedural primitives and voxel prefabs; detail lives in textures. See `adr/0006`-adjacent notes in `04-rendering.md`. |
| A scripting host | There is no embedded language. Gameplay is Rust. |
| An editor | `tools/noxel-gen` generates assets from code, which is diffable and reviewable; there is no GUI. |
| Dependency-friendly | The default build has **zero** third-party crates. See `adr/0002-no-dependencies.md`. |

---

## The five ideas

Everything in this codebase follows from five decisions. If you internalise these,
the rest is detail.

### 1. One world unit is one metre, `+Y` is up, `-Z` is forward

A right-handed, metres-based, radians-based world with clip depth in `[0, 1]`.
Every subsystem — noise, physics, pathfinding, streaming — speaks this one
language, so there is never a "which units is this in?" question.
→ `adr/0001-coordinate-system.md`

### 2. The world is a pure function of `(seed, address)`

Chunk `(900, -400)` generates identically whether the player walked there or
teleported in, on any machine, in any order, in parallel. There is no shared
mutable random number generator anywhere in the generation path.
→ `adr/0006-deterministic-generation.md`

This is what makes streaming, saving, threading and testing all work at once. It
is the single most load-bearing decision in the engine.

### 3. Rendering is deterministic too

No renderer consults a clock, a thread id or a global RNG. The same frame
produces the same bytes. That turns a rendering regression from "a player noticed
three weeks later" into a failing test — golden-image comparison against a
committed PNG.
→ `adr/0008-deterministic-rendering.md`

### 4. The CPU renders, and there are three ways to do it

A tiled software rasterizer is the default; a ray tracer is available; the
**hybrid** mode rasterizes primary visibility and ray-traces the secondary rays
(shadows, ambient occlusion), which is where the quality per unit cost actually
is.
→ `adr/0003-software-renderer-first.md`, `adr/0004-dual-renderer.md`

### 5. Colour is linear until the very end

Everything is linear `f32` radiance until one resolve per frame converts to sRGB.
An unlit sprite therefore round-trips to **exactly** the bytes the artist drew —
asserted by test in both renderers.
→ `adr/0005-color-management.md`

---

## How a frame works

```text
                App::step(dt)
                      |
   +------------------+-------------------+
   |                                      |
fixed_update (0..n)                    frame_update
   |                                      |
   |  plugins.update(dt)                  |  camera.follow(focus, dt)
   |  physics.step(dt)                    |  streamer.update(focus)
   |  <- FIXED rate, so gameplay          |  visibility.update(...) -> VisibleSet
   |     is deterministic                 |
   +------------------+-------------------+
                      |
                    render
                      |
        +-------------+-------------+
        |             |             |
     Raster       Hybrid        Raytrace
        |             |             |
        +-------------+-------------+
                      |
                  overlays
                      |
                  resolve  ->  sRGB Image  ->  your window or a PNG
```

Two orderings are not arbitrary:

- **The camera moves before streaming and visibility.** Otherwise the world
  arrives a frame late and the player sees chunks pop in behind them.
- **Gameplay runs at a fixed rate and rendering at whatever rate the machine
  manages.** Even with no dependencies there is no reason to make physics depend
  on frame time.

---

## The data flow, and who owns what

```text
  seed + WorldConfig
        |
        v
  WorldGenerator  ---- pure function of (seed, chunk) ---->  Chunk
        |                                                      |
        |  sample_height / biome_at / is_road_at                |  tiles, heights,
        |  (answerable for ANY coordinate, no chunk needed)     |  colliders, props,
        v                                                       v  buildings
  WorldStreamer  <--- LRU around the camera --->  loaded chunks
        |                                                      |
        |                                                      |
        v                                                      v
  tile queries, walkability, colliders          scene meshes per chunk
        |                                                      |
        +-----------+--------------------+                     |
                    |                    |                     |
             PhysicsWorld          NpcSystem              Scene (instances)
             (bodies,      (crowds, pathing, steering)          |
              queries)                    |                     |
                    |                    |                     |
                    +----------+---------+---------------------+
                               |
                          AppContext
                               |
                        VisibilitySystem  ->  VisibleSet
                               |
                          Renderer  ->  Framebuffer  ->  Image
```

`AppContext` is the one place every subsystem is reachable. A plugin receives
`&mut App` and reads `app.physics`, `app.streamer`, `app.scene` and so on.

---

## The crate map

| Crate | One line | Depends on |
|---|---|---|
| `noxel-core` | math, RNG, clock, jobs, pools, spatial structures, events | — |
| `noxel-ecs` | sparse-set entities and components, staged scheduler | core |
| `noxel-asset` | PNG, JSON, images, textures, atlases, data formats, hot reload | core |
| `noxel-render` | framebuffer, meshes, materials, lights, scene, rasterizer, ray tracer, overlay | core, asset |
| `noxel-camera` | top-down rig, smoothing, pixel-perfect snapping, shake, zones | core, render |
| `noxel-visibility` | frustum/distance/size/occlusion culling, fades, LOD | core, ecs, render, camera |
| `noxel-physics` | bodies, SAT, solver, character controller, queries | core |
| `noxel-world` | terrain, biomes, roads, towns, streaming, prefabs | core, asset |
| `noxel-npc` | crowd tiers, A*, flow fields, steering, schedules | core, ecs, physics, world |
| `noxel-debug` | statistics, budgets, overlays, frame dumping, image diff | core, render, asset |
| `noxel-app` | `App`, `Plugin`, the fixed-timestep frame loop | all of the above |
| `tools/noxel-gen` | the asset generator | core, asset |
| `examples/town-demo` | a complete village | everything |

Dependencies point **one way only**. That is a rule, not a coincidence: it means
any crate can be understood by reading it and the crates below it, and no crate
can be broken by a change above it.

---

## What you get for free

Things you would otherwise write yourself, in the order you will notice them:

- **A camera that feels right.** Deadzone, frame-rate-independent smoothing,
  look-ahead, pixel-perfect snapping, bounds clamping, and trauma-based shake
  that is reproducible rather than time-based.
- **A camera you can see through.** When a roof or a tree stands between the
  camera and the player, it fades out; when it stops blocking, it fades back in.
  This is the difference between a top-down game that works indoors and one that
  does not.
- **A world that costs nothing off-screen.** Frustum, distance, screen-size and
  occlusion culling, with a culling report that says exactly which stage rejected
  what.
- **A crowd.** Tiered simulation, shared flow fields, steering, and daily
  schedules — with a measured microseconds-per-agent number in the test suite.
- **Physics you can build a game on.** Character controller with step-up and a
  slope limit, ray and overlap queries, layers, sleeping, deterministic stepping.
- **A debug story.** Rolling percentiles per section (not means), a frame-time
  graph with a budget line, and headless frame dumping with an image diff, so a
  rendering regression fails CI.
- **Assets from code.** `noxel-gen` produces every texture, atlas, tileset and
  prefab deterministically, so the art is diffable.

---

## Where to go next

| You want to | Read |
|---|---|
| Build and run something | `02-getting-started.md` |
| Understand every crate and module | `01-architecture.md` |
| Make a world | `03-world-generation.md` |
| Make it look right | `04-rendering.md` |
| Make the camera feel good | `05-camera-and-visibility.md` |
| Move things around | `06-physics.md` |
| Fill a town with people | `07-npcs.md` |
| Make it fast | `08-performance.md` |
| Know why something is the way it is | `adr/` |
| Work on the engine itself | `contributing-for-ai.md` |
