# Noxel

A lightweight, modular, high-performance **3D top-down pixel-art RPG engine**,
written in Rust with **no third-party dependencies**.

```text
$ cargo run -p town-demo                # generate a world, walk a town, write PNG frames
$ cargo test --workspace                # ~900 tests, a few seconds, no display needed
$ cargo run -p noxel-gen -- generate    # regenerate every asset in examples/town-demo/assets
```

---

## What it is

Noxel renders a 3D world from a top-down camera at a pixel-art internal
resolution (320x180 by default), upscaled to the window. It is built for games
where the world is large, the camera is far away, and there are a lot of things
moving: top-down RPGs, colony sims, tactical games, anything with a crowd.

| Requirement | Where it lives |
|---|---|
| Pixel-art 3D top-down renderer | [`noxel-render`](crates/noxel-render) — tiled software rasterizer + ray tracer |
| Lightweight, cross-platform | Zero dependencies, no `unsafe`, pure `std` |
| Large-world generation and runtime | [`noxel-world`](crates/noxel-world) — deterministic generation, chunk streaming |
| Easy towns and roads | [`noxel-world`](crates/noxel-world) — road lattice, town plans, procedural buildings, prefabs |
| Accurate camera motion | [`noxel-camera`](crates/noxel-camera) — deadzone, look-ahead, pixel-perfect snapping, shake |
| Auto-transparency when objects occlude the view | [`noxel-visibility`](crates/noxel-visibility) — ray-driven camera occlusion fades |
| Cull occluded objects | [`noxel-visibility`](crates/noxel-visibility) — frustum, distance, size, occlusion |
| Physics with rich player interaction | [`noxel-physics`](crates/noxel-physics) — bodies, SAT, character controller, ray queries |
| Both rasterization and ray tracing | [`noxel-render`](crates/noxel-render) — three modes over one scene |
| Large numbers of simultaneous NPCs | [`noxel-npc`](crates/noxel-npc) — crowd tiers, flow-field pathfinding, steering |
| Modular and extensible | One crate per subsystem, one `Renderer` trait, one `Plugin` trait |
| Detailed documentation instead of UI | `docs/` — overview, guides, API, 10 ADRs |

---

## Quick start

```bash
# Rust 1.85+ (edition 2024)
git clone <this repo> && cd noxel
cargo run -p town-demo -- --frames 600 --dump frames
```

Then open `frames/frame_000599.png`. You should see a village: a plaza, streets, a
few dozen buildings, trees on the hills, a river, and a character walking a
scripted route with the camera trailing behind.

Switch renderers on the same world:

```bash
cargo run -p town-demo -- --mode hybrid     # raster primary + ray-traced shadows and AO
cargo run -p town-demo -- --mode raytrace   # full ray tracing (slow, beautiful)
```

---

## The five-minute tour

An engine built on Noxel is an `App` plus plugins. This is a complete one:

```rust
use noxel_app::{App, AppConfig, Plugin};
use noxel_core::math::{Transform, Vec3};
use noxel_render::mesh::Mesh;
use noxel_render::material::Material;
use noxel_render::Color;

struct Player;

impl Plugin for Player {
    fn name(&self) -> &str { "player" }

    fn build(&mut self, app: &mut App) {
        let mesh = app.scene_mut().add_mesh(Mesh::capsule(0.35, 1.7));
        let material = app.scene_mut().add_material(Material::lit("player", Color::WHITE));
        let handle = app.scene_mut().spawn("player", mesh, material, Transform::IDENTITY);
        app.context.focus = Vec3::ZERO;
        let _ = handle;
    }
}

fn main() {
    let mut app = App::new(AppConfig::default()).unwrap();
    app.add_plugin(Player);
    let report = app.run_headless(300);
    println!("{}", report.summary());
}
```

`app.run_headless(n)` renders real frames with no window. `App::step(dt)`
advances one frame if you own the loop; `docs/guides/windowing.md` shows how to
attach `winit` or `SDL2` to present the framebuffer.

---

## Architecture

```text
                                    noxel-app
                        (App, Plugin, fixed-step frame loop)
                                       |
        +--------------+---------------+---------------+--------------+
        |              |               |               |              |
   noxel-npc     noxel-world    noxel-physics   noxel-camera   noxel-debug
   (crowds,      (generation,   (bodies,        (rig, shake,   (stats,
    steering,     streaming,     SAT, queries,   zones,         overlays,
    schedules)    roads, towns)  character)      pixel-perfect) dumping)
        |              |               |               |              |
        +--------------+-------+-------+-------+-------+--------------+
                               |               |
                        noxel-visibility   noxel-render
                        (frustum, occlusion, (raster + raytrace,
                         LOD, camera fades)  framebuffer, scene)
                               |               |
                            noxel-ecs      noxel-asset
                            (entities,     (PNG, JSON, atlas,
                             components,    formats, hot reload)
                             scheduler)
                               |               |
                               +-------+-------+
                                       |
                                  noxel-core
                    (math, rng, time, jobs, pools, spatial, events)
```

Dependencies point **one way only**. `noxel-core` depends on nothing;
`noxel-app` depends on everything. Adding an edge that points backwards is a
design error, not a compile error you can work around.

| Crate | Purpose |
|---|---|
| `noxel-core` | math, deterministic RNG, fixed clock, job pool, slot maps, spatial structures, events |
| `noxel-ecs` | sparse-set entities and components, staged scheduler |
| `noxel-asset` | PNG codec, JSON, images, textures, atlases, data formats, hot-reloading database |
| `noxel-render` | framebuffer, meshes, materials, lights, scene, rasterizer, ray tracer, debug overlay |
| `noxel-camera` | top-down rig, deadzone, look-ahead, pixel-perfect snapping, trauma shake, zones |
| `noxel-visibility` | frustum/distance/size culling, occlusion culling, camera-occlusion fades, LOD |
| `noxel-physics` | bodies, SAT narrowphase, sequential-impulse solver, character controller, queries |
| `noxel-world` | deterministic terrain, biomes, road lattice, towns, chunk streaming, prefabs |
| `noxel-npc` | crowd tiers, flow-field and A* pathfinding, steering, daily schedules |
| `noxel-debug` | rolling statistics, section budgets, overlays, headless frame dumping, image diff |
| `noxel-app` | `App`, `AppContext`, `Plugin`, the fixed-timestep frame loop |
| `tools/noxel-gen` | the asset generator: textures, atlases, tilesets, prefabs, the palette |
| `examples/town-demo` | a ready-to-run village with a player, NPCs, physics and dumped frames |

---

## Design decisions

The engine's choices are documented as ADRs in [`docs/adr`](docs/adr). The ones
that shape everything else:

| ADR | Decision |
|---|---|
| [0001](docs/adr/0001-coordinate-system.md) | Right-handed, `+Y` up, `-Z` forward, radians, column-major, clip depth `[0,1]`, 1 unit = 1 m |
| [0002](docs/adr/0002-no-dependencies.md) | Zero third-party dependencies; even the PNG codec is in-tree |
| [0003](docs/adr/0003-software-renderer-first.md) | CPU rasterizer and ray tracer as the default; a GPU backend is an extension point |
| [0004](docs/adr/0004-dual-renderer.md) | One scene, one camera view, and three shading modes over the same data |
| [0005](docs/adr/0005-color-management.md) | Linear HDR working space, exactly one sRGB resolve per frame, exact palette round-trip |
| [0006](docs/adr/0006-deterministic-generation.md) | The world is a pure function of `(seed, address)` — no shared RNG anywhere |
| [0007](docs/adr/0007-gpu-backend.md) | The GPU backend is specified and feature-gated, not implemented |
| [0008](docs/adr/0008-deterministic-rendering.md) | Rendering is deterministic, which is what makes golden-image tests possible |
| [0009](docs/adr/0009-no-ui.md) | No UI toolkit — a debug overlay and documentation instead |
| [0010](docs/adr/0010-testing-strategy.md) | Every invariant has a test; a failing test means deciding whether the code or the test is wrong |

---

## Documentation

| Document | Contents |
|---|---|
| [`docs/00-overview.md`](docs/00-overview.md) | What the engine is, what it is not, and the mental model |
| [`docs/01-architecture.md`](docs/01-architecture.md) | Every crate, every module, and how a frame flows through them |
| [`docs/02-getting-started.md`](docs/02-getting-started.md) | Build, run, test, and extend — from zero |
| [`docs/03-world-generation.md`](docs/03-world-generation.md) | Terrain, biomes, roads, towns, streaming, and how to author assets |
| [`docs/04-rendering.md`](docs/04-rendering.md) | The three shading modes, materials, pixel-art rules, the shadow map |
| [`docs/05-camera-and-visibility.md`](docs/05-camera-and-visibility.md) | Camera feel, occlusion fades, culling, LOD |
| [`docs/06-physics.md`](docs/06-physics.md) | Bodies, layers, the character controller, queries, determinism |
| [`docs/07-npcs.md`](docs/07-npcs.md) | Crowd tiers, pathfinding, steering, schedules, and how to hit 1000 NPCs |
| [`docs/08-performance.md`](docs/08-performance.md) | Where the time goes and how to measure it |
| [`docs/api/`](docs/api) | Per-crate API reference |
| [`docs/contributing-for-ai.md`](docs/contributing-for-ai.md) | How to work on this codebase, written for an AI agent |

Regenerate the API docs with `cargo doc --workspace --no-deps --open`.

---

## License

Dual-licensed under MIT or Apache-2.0, at your option.
