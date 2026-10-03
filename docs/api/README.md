# API index

`cargo doc` is the reference for every item below. This page is a map: which
crate does what, what to import first, and where to look next.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo doc --workspace --no-deps --open     # writes target/doc/ and opens it
```

The generated pages are also reachable directly from this file:
[`target/doc/noxel_core/index.html`](../../target/doc/noxel_core/index.html).
Replace `noxel_core` with any crate name from the table.

## Crates

| Crate | Purpose | Prelude | Five types you will touch first |
|---|---|---|---|
| [`noxel-core`](../../target/doc/noxel_core/index.html) | maths, deterministic RNG, timing, job pool, slot maps, spatial structures, event bus. Every other crate depends on it and nothing below it | `noxel_core::prelude` | `Vec3`, `Mat4`, `Transform`, `RngStream`, `GameClock` |
| [`noxel-ecs`](../../target/doc/noxel_ecs/index.html) | sparse-set entity/component world with a staged scheduler | `noxel_ecs::prelude` | `World`, `Entity`, `Scheduler`, `Component`, `Resource` |
| [`noxel-asset`](../../target/doc/noxel_asset/index.html) | JSON, a from-scratch PNG codec, images, textures, atlas packing, data formats (`TileSet`, `Prefab`, palettes, sprites) and a hot-reloading database | `noxel_asset::prelude` | `AssetDb`, `Image`, `Texture`, `TileSet`, `Prefab` |
| [`noxel-render`](../../target/doc/noxel_render/index.html) | one scene, two renderers: tile-binned software rasterizer and CPU ray tracer, plus the hybrid; linear HDR framebuffer with one sRGB resolve | `noxel_render::prelude` | `Scene`, `Material`, `Mesh`, `Framebuffer`, `RenderSettings` |
| [`noxel-camera`](../../target/doc/noxel_camera/index.html) | the top-down rig: focus, deadzone, frame-rate-independent smoothing, look-ahead, zoom, shake, zones, pixel-perfect snapping | — | `TopDownCamera`, `ProjectionMode`, `PixelPerfect`, `CameraShake`, `CameraZone` |
| [`noxel-visibility`](../../target/doc/noxel_visibility/index.html) | three-stage culling (frustum, distance/size, occlusion), the camera-occlusion fade table and LOD selection | — | `VisibilitySystem`, `VisibilityInput`, `CullSettings`, `OcclusionSettings`, `LodLevels` |
| [`noxel-physics`](../../target/doc/noxel_physics/index.html) | fixed-step deterministic rigid bodies, a sequential-impulse solver, layers, queries and the kinematic character controller | — | `PhysicsWorld`, `BodyDesc`, `ColliderShape`, `BodyHandle`, `CharacterMove` |
| [`noxel-world`](../../target/doc/noxel_world/index.html) | procedural terrain, biomes, roads, towns, props, chunk data and LRU streaming, all a pure function of `(seed, position)` | — | `WorldConfig`, `WorldGenerator`, `WorldStreamer`, `Chunk`, `BiomeId` |
| [`noxel-npc`](../../target/doc/noxel_npc/index.html) | crowd tiers, A\*/flow-field pathfinding, steering and daily schedules, driven by one `NpcSystem` over a `WorldStreamer` and a `PhysicsWorld` | — | `NpcSystem`, `NpcConfig`, `NpcContext`, `CrowdManager`, `NpcAgent` |
| [`noxel-debug`](../../target/doc/noxel_debug/index.html) | rolling statistics with percentiles, section budgets, the overlay panels and frame dumping/diffing | — | `DebugSystem`, `DebugConfig`, `Stats`, `FrameDumper`, `ImageDiff` |
| [`noxel-window`](../../crates/noxel-window) | presents the framebuffer on screen and reads player input; the one crate with an optional third-party dependency |
| [`noxel-app`](../../target/doc/noxel_app/index.html) | the runtime host: fixed-step frame loop, shared context, plugin registry, headless runner | — | `App`, `AppConfig`, `AppContext`, `Plugin`, `InputState` |

`tools/noxel-gen` is a binary, not a library: run it with
`cargo run -p noxel-gen -- --help`. It writes the asset tree the engine loads
(tile sets, prefabs, palettes, sprites, terrain).

## Which crate do I want?

| Task | Start at |
|---|---|
| Move a character with collisions | `noxel_physics::PhysicsWorld::move_character`, `docs/06-physics.md` |
| Decide what to draw | `noxel_visibility::VisibilitySystem`, `docs/05-camera-and-visibility.md` |
| Make a lit scene look right | `noxel_render::Material`, `Light`, `docs/04-rendering.md` |
| Generate or query the world | `noxel_world::WorldGenerator`, `WorldStreamer`, `docs/03-world-generation.md` |
| Wire a game together | `noxel_app::App`, `Plugin`, `docs/01-architecture.md` |
| Find out why a frame is slow | `noxel_debug::DebugSystem`, `docs/08-performance.md` |
| Fill a town with people | `noxel_npc::NpcSystem`, `docs/07-npcs.md` |

## Where the rest of the documentation is

| Path | Contents |
|---|---|
| `docs/00-overview.md` | what Noxel is and what it is for |
| `docs/01-architecture.md` | the crate DAG, the frame, and the rules between layers |
| `docs/02-getting-started.md` | build, run and first frame |
| `docs/03-world-generation.md` … `docs/08-performance.md` | the module guides |
| `docs/adr/` | the design record, one decision per file |
| `docs/contributing-for-ai.md` | how to work in this repository without breaking it |
| `examples/town-demo/README.md` | the runnable example |

Every crate is `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, and the
workspace has no third-party dependencies (`docs/adr/0002-no-dependencies.md`).
If `cargo doc` shows a missing item, that is a bug in the crate, not a gap in
this index.
