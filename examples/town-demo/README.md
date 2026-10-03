# town-demo

A complete Noxel game loop in six source files: generate a world from a seed,
grow a village in it, drop a player into the plaza, fill the streets with a
crowd, follow the player with a top-down camera, and write every frame as a PNG.

> **Status.** The binary builds: `cargo check -p town-demo` succeeds (with
> warnings, including an unused `prop_bounds` in `src/village.rs`).
> `cargo test -p town-demo` does **not** — the demo's own test target has two
> errors from `Handle::<Instance>::default()` in `src/actors.rs` (lines 474 and
> 494), a trait implementation the engine's `Handle` does not provide. The HUD
> overlay is also not wired: `hud::draw` exists but `main.rs` only calls
> `hud::final_report`.

## What it demonstrates

| System | What the demo does with it |
|---|---|
| World generation | `WorldConfig` from a seed, streamed around the player; biomes, roads, towns and props come from `noxel-world` |
| Chunk streaming | one `WorldStreamer::update` per frame, driven by the camera focus |
| Terrain meshing | `TerrainPlugin`: one mesh per chunk, one ground quad per tile, one UV region per tile from the terrain atlas |
| Village meshing | `village.rs` turns a chunk's buildings and props into geometry: one box per building with a roof quad, a cone per tree, a box per rock — all in two extra meshes per chunk, so a chunk is three draw calls |
| Materials | ground **unlit** (the artist's tile colour survives), buildings and props **lit** so the sun and the hybrid mode's ray-traced AO give them volume |
| Collision | every `Chunk::colliders` box becomes a static physics body, added and removed as chunks stream |
| Character controller | the player is a kinematic capsule moved by `PhysicsWorld::move_character`: step-up, wall sliding and a slope limit |
| Camera | `TopDownCamera` with a deadzone, look-ahead, smoothing and clip planes |
| Crowds | `NpcSystem`, one scene instance per agent, tiered so distant agents are not occluders |
| Lighting | ambient sky/ground, a low sun, and linear distance fog |
| Debug output | counters and section budgets in `noxel-debug`, a final report, and PNG dumps |

## Commands

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo run -p town-demo -- --frames 300 --dump frames
cargo run -p town-demo -- --mode hybrid --npcs 400
cargo run -p town-demo -- --stats            # one settled frame, full statistics
cargo run -p town-demo -- --world-info       # what this seed generates, no rendering
cargo run -p town-demo -- --help
```

| Flag | Default | Meaning |
|---|---|---|
| `--seed N` | `0x4E4F5845` | world seed |
| `--frames N` | 600 | frames to simulate and render |
| `--size WxH` | `320x180` | internal resolution |
| `--mode MODE` | `raster` | `raster`, `hybrid`, `raytrace` (plus `rasterize`, `rt`) |
| `--npcs N` | 240 | target crowd size |
| `--dump DIR` | `frames` | write every frame as a PNG here |
| `--no-dump` | — | render without writing anything |
| `--stats` | — | render one frame, print its statistics, exit |
| `--world-info` | — | print the world's shape for this seed, exit |
| `-q`, `--quiet` | — | only the summary |
| `-h`, `--help` | — | the usage block |

Run it from the repository root: the demo looks for `examples/town-demo/assets`
first and `assets/` second. That tree **exists** and is generated — do not
hand-edit it. It ships a 24-colour palette, four textures (terrain, buildings,
characters, props), ten prefabs (`house_small`, `house_large`, `inn`, `barn`,
`shop`, `market_stall`, `well`, `lamp`, `tree_oak`, `tree_pine`, `fence_segment`,
`rock_cluster`), a tile set and `world/demo.json`:

```bash
cargo run -p noxel-gen -- generate --force   # rewrite the whole tree
cargo run -p noxel-gen -- verify             # check it against the generator
cargo run -p noxel-gen -- preview            # contact sheet of the textures
```

`--world-info` prints the chunk size, view distance, road and town spacing, the
loaded chunk count, five height/biome/road samples, and the towns near the origin
with their plot counts. It generates and streams but never renders.

## What you should see

`--frames 300 --dump frames` writes `frames/frame_000000.png` onward, 320x180,
one file per rendered frame, after a 12-frame warm-up so the first image is
settled. The terminal prints a header, a `HeadlessReport::summary()` line, and
the end-of-run report: frames and fps (mean and p95), the section budgets with
`last / allowance` and `ok`/`OVER`, every recorded counter (chunks, chunk
generation, chunk memory, culling counts, the player's position, grounded state),
and a "next steps" block.

In an image you should see:

- **Flat, unlit terrain** on a 1 m tile grid, 20 m of world across the screen,
  textured from the terrain atlas. Terrain is unlit on purpose: a lit surface
  would multiply the artist's tile colour by the sun. Depth cues come from the
  buildings, the props and the ray-traced AO in hybrid mode.
- **A road lattice** every 256 m, three tiles wide and graded into the terrain,
  and a village around the nearest town lattice cell: a plaza, a main cross, side
  streets, and buildings from the prefab library with gable roofs and lit faces.
- **Props** — trees, rocks, fences — never on a road, in water, on a steep slope
  or inside a building.
- **A capsule player** walking a circular scripted route, pausing briefly at each
  waypoint, with the camera trailing it.
- **A crowd of capsules** filling in around the plaza, coloured by kind.
- **Distance fog** starting at 70 m and total by 160 m.

`--mode hybrid` adds ray-traced sun shadows and ambient occlusion; `--mode
raytrace` traces everything and is meant for stills, not a frame rate.

## File-by-file map of `src/`

| File | Lines | Responsibility |
|---|---|---|
| `main.rs` | ~300 | argument handling, `App` construction, world/lighting/camera setup, plugin registration, the run loop, the world-info printout |
| `args.rs` | ~280 | the hand-written flag parser, `world_config()`, `physics_config()` and the help text |
| `terrain.rs` | ~550 | `TerrainPlugin`: chunk → ground mesh, chunk → static colliders, add/remove as chunks stream; `build_chunk_mesh`, `uv_rect`, `ground_height`, `is_walkable`, `sample_ground` |
| `village.rs` | ~360 | chunk → building and prop meshes, one mesh each, from the generator's merged boxes; lit materials |
| `actors.rs` | ~630 | `Player`/`PlayerPlugin` (route or input, `move_character`, camera focus), `CrowdPlugin` (crowd step, scene mirroring per kind, tier flags), and the three `Plugin` impls |
| `hud.rs` | ~250 | the overlay text, the occluder visualiser, the final report and a one-line status |

### How the three plugins chain

`main.rs` registers them in this order, and the order is the contract:

```text
app.add_plugin(TerrainPlugin::new());   // 1. world geometry, after streaming
app.add_plugin(PlayerPlugin::new());    // 2. spawn the player, build the route
app.add_plugin(CrowdPlugin::new(cfg));  // 3. populate and step the crowd
```

`Plugin::build` runs once per plugin, in registration order. The per-frame hooks
are where the real ordering lives:

| Plugin | Hook | Why |
|---|---|---|
| `PlayerPlugin` | `update` (fixed step) | `move_character` runs at 60 Hz with everything else; the camera focus is written here |
| `PlayerPlugin` | `pre_cull` | makes the focus final before the engine's own `frame_update` moves the camera and culls |
| `CrowdPlugin` | `update` (fixed step) | waits until `streamer.stats().loaded > 0`, then populates once and steps the crowd with `app.context.elapsed` as `world_time` |
| `TerrainPlugin` | `frame` | syncs chunk meshes and static bodies *after* `App::frame_update` has streamed the chunks, so the geometry matches what is resident this frame |

The chain that matters: **terrain owns the geometry and the static bodies, the
player owns the camera focus, and the crowd owns everything else.** `App` runs
the camera follow, streaming and visibility itself, after every plugin `update`
and after the `pre_cull` hooks.

## Turning it into a real game

Three things are missing, deliberately out of scope for the engine
(`docs/adr/0009-no-ui.md`, `docs/adr/0002-no-dependencies.md`).

### 1. Attach a window

Noxel has no windowing layer. An `App` produces a `Framebuffer` and consumes an
`InputState`; presenting the buffer is the host's job:

```rust,no_run
// Behind a feature flag, with `winit` or `SDL2` as an optional dependency.
loop {
    for event in window_events() {
        match event {
            Event::KeyDown(code) => app.input_mut().press(code),
            Event::KeyUp(code) => app.input_mut().release(code),
            Event::Resize(..) => { /* rebuild the presenter, not the app */ }
            Event::Close => break,
        }
    }
    app.step(frame_dt);              // fixed updates, camera, streaming, cull, render, overlays
    let image = app.resolve();       // linear HDR -> sRGB, once per frame
    present(image);                  // `Viewport::pixel_art` gives the integer scale
}
```

`app.step(dt)` already calls `InputState::end_frame()`, so `was_pressed`,
`was_released` and the mouse deltas are one-frame values. Do not call it yourself.

### 2. Feed `InputState`

`InputState` is plain data: `press(key)`, `release(key)`, `is_held`,
`was_pressed`, `was_released`, `movement_axis(up, down, left, right)`, plus
`mouse`, `mouse_delta`, `scroll` and `mouse_buttons`. Key codes are whatever the
host decides — the demo's free-movement branch reads WASD as `82, 81, 80, 79`,
which are host codes, not engine constants. `InputState::release_all()` exists for
a focus loss; without it a key held when the window loses focus stays held.

### 3. Replace the scripted route

`PlayerPlugin` has two movement modes. The scripted one follows
`PlayerPlugin::route`, a `Vec<Waypoint>` built by `build_route`, which walks a
ring around a centre point and nudges each candidate inwards until
`TerrainPlugin::is_walkable` accepts it. That is what makes a headless run
reproducible: no input, no randomness, the same walk every time. The plugin impl
passes `scripted = true`, with a comment saying a host that fills
`app.input_mut()` can flip it — that is the first line to change.

After that: add a vertical velocity for jumping (`Player::vertical_velocity`
exists and is unused by the scripted branch), give the crowd real schedules
instead of placeholder villagers, and add a UI plugin that draws with
`noxel_render::overlay` in the `draw` hook — or bring your own UI library.

## Known gaps

These are deliberate, and each has a reason.

- **The camera is a fixed three-quarter view.** A real game wants a zoom and a
  rotate control; `TopDownCamera` supports both (`zoom_by`, `set_yaw`), and they
  are left out so the demo's scripted route stays reproducible frame for frame.
- **The player follows a scripted circular route.** There is no window, so there
  is no input stream. `PlayerPlugin::update` already reads WASD when its
  `scripted` flag is false; flip it once a host is filling `app.input_mut()`.
- **NPCs are not drawn from the character sheet.** The demo gives each agent a
  flat-colour billboard; the generated 4-direction walk cycle in
  `assets/textures/characters.png` is described by `assets/sprites/characters.json`
  and awaits a sprite-batching renderer, which the engine does not yet have.
- **`frames/` is a scratch directory.** The demo writes one PNG per frame and
  clears stale frames on start, but it never prunes the current run. It is
  `.gitignore`d.
- **A large `--npcs` value is bounded by physics.** Tier-0 agents near the camera
  own a capsule; the rest are steered and path-followed. Raising the population
  past a few thousand moves more of the crowd into the cheaper tiers rather than
  making the frame slower, which is the intended behaviour but not obvious from
  the flag name.
