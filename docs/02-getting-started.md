# Getting started

From an empty shell to a running village in about ten minutes.

---

## 0. Prerequisites

Rust 1.85 or newer (the workspace uses edition 2024):

```bash
rustup update stable
rustc --version     # 1.85+
```

Nothing else. No graphics drivers, no system libraries, no submodules — the
engine has zero third-party dependencies (`adr/0002-no-dependencies.md`).

> **If `cargo` is not found**, this machine keeps the toolchain outside the
> default `PATH`. Run `export PATH="$HOME/.cargo/bin:$PATH"` first, or add it to
> your shell profile.

---

## 0b. Or just run the prebuilt example

If you do not have Rust, or you just want to see the engine work, build the
relocatable example bundle on a machine that does:

```bash
./scripts/build-dist.sh
```

That produces `dist/`:

```
dist/
  town-demo      the compiled example
  noxel-gen      the asset generator
  assets/        every texture, tileset, prefab and sprite it needs
  README.txt
```

`dist/` needs no toolchain, no source tree and no network. Copy it anywhere —
to a USB stick, to a colleague's laptop — and run `./town-demo`. The binary finds
its assets beside itself, so the working directory does not matter.

```bash
cd dist && ./town-demo --frames 300
```

The rest of this guide is for working on the source.

---

## 1. Build and test

```bash
cd noxel
cargo build --workspace
cargo test  --workspace
```

The whole suite is a few hundred tests and runs in seconds, with no display
server. If a test fails on a fresh checkout, that is a real finding — read
`adr/0010-testing-strategy.md` before changing anything.

---

## 2. Generate the assets

```bash
cargo run -p noxel-gen -- generate
```

That writes every texture, atlas, tileset, prefab, sprite sheet and manifest into
`examples/town-demo/assets/`. The generator is deterministic: running it twice
changes no bytes, so re-running it after a pull is a no-op.

```bash
cargo run -p noxel-gen -- verify      # exits non-zero if a file was tampered with
cargo run -p noxel-gen -- list        # what it can produce
cargo run -p noxel-gen -- preview --out frames/atlas   # a contact sheet of every image
```

You do **not** have to generate assets to run the engine. A missing asset tree
falls back to procedural content and a placeholder texture, which is what lets
the test suite build a working `App` in a millisecond.

---

## 3. Run the demo

```bash
cargo run -p town-demo -- --frames 300 --dump frames
```

Then open `frames/frame_000299.png`. You should see a village: a plaza, streets,
a few dozen buildings, trees, and a character following a scripted route with the
camera trailing behind. The HUD in the corner names the seed and the frame.

Useful variations:

```bash
# Ray-traced shadows and ambient occlusion on the same world.
cargo run -p town-demo -- --mode hybrid --frames 60

# A bigger crowd.
cargo run -p town-demo -- --npcs 600 --frames 300

# One frame, every statistic, no files written.
cargo run -p town-demo -- --stats

# What does this seed actually generate?
cargo run -p town-demo -- --seed 12345 --world-info

# A different world.
cargo run -p town-demo -- --seed 0xC0FFEE --frames 300
```

`--help` lists everything.

---

## 4. Your first app

An engine built on Noxel is an `App` plus plugins. This is complete and runnable:

```rust,no_run
use noxel_app::{App, AppConfig, Plugin};
use noxel_core::math::{Color, Transform, Vec3};
use noxel_render::material::Material;
use noxel_render::mesh::Mesh;

/// Spawns a cube and spins it.
struct Spinner {
    instance: Option<noxel_render::scene::InstanceHandle>,
    angle: f32,
}

impl Plugin for Spinner {
    fn name(&self) -> &str { "spinner" }

    fn build(&mut self, app: &mut App) {
        let mesh = app.scene_mut().add_mesh(Mesh::cube(2.0));
        let material = app.scene_mut().add_material(Material::lit("cube", Color::WHITE));
        self.instance = Some(
            app.scene_mut().spawn("cube", mesh, material, Transform::IDENTITY),
        );
    }

    fn update(&mut self, app: &mut App, dt: f32) {
        self.angle += dt;
        if let Some(instance) = self.instance {
            app.scene_mut().set_transform(
                instance,
                Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)).with_yaw(self.angle),
            );
        }
    }
}

fn main() {
    let mut app = App::new(AppConfig::default()).unwrap();
    app.add_plugin(Spinner { instance: None, angle: 0.0 });
    for _ in 0..120 {
        app.step(1.0 / 60.0);
    }
    // `app.resolve()` is now an sRGB image; write it, or blit it to a window.
    let image = app.resolve();
    println!("{}x{}", image.width, image.height);
}
```

Three things to notice:

- `App::step(dt)` runs one whole frame: fixed updates, camera, streaming,
  culling, render, overlays.
- `Scene` hands out handles, not indices. A stale handle is skipped rather than
  silently pointing at the wrong object, which is what makes streaming teardown
  order irrelevant.
- `resolve()` is where linear HDR becomes sRGB bytes. Nothing before it is a
  colour you can show anyone.

---

## 5. Seeing what is happening

Noxel has no UI (`adr/0009-no-ui.md`), but it has a debug system:

```rust,no_run
# use noxel_app::{App, AppConfig};
# let mut app = App::new(AppConfig::default()).unwrap();
app.debug_mut().record_counter("npcs", 240.0);
app.debug_mut().record_section("physics", 2.4);
app.step(1.0 / 60.0);
println!("{}", app.debug().report());
```

The report has rolling means, p50 and p95 per section, and a budget line that
reads `OVER` when a section spends more than its allowance. A mean frame time
hides a stutter; a p95 does not.

For a picture, dump frames:

```rust,no_run
# use noxel_app::{App, AppConfig};
# let mut app = App::new(AppConfig::default()).unwrap();
let report = app.run_and_capture(60);
// `report.last_image` is a decoded PNG-ready image.
```

and compare two runs:

```rust,no_run
# use noxel_debug::dump;
# fn example(a: &noxel_asset::image::Image, b: &noxel_asset::image::Image) {
let diff = dump::compare(a, b);
println!("{}", diff.summary());
assert!(diff.within(0.0, 0), "the frame changed");
# }
```

A zero tolerance is possible because rendering is deterministic
(`adr/0008-deterministic-rendering.md`).

---

## 6. Adding a window

The engine never opens one. `App` produces a `Framebuffer`; presenting it is the
host's job, which keeps `noxel-app` free of platform code.

```text
your host loop
  ├─ fill app.input_mut() from the platform's events
  ├─ app.step(real_dt)
  ├─ let image = app.resolve();          // sRGB bytes, width*height*4
  └─ upload `image.pixels` to your texture and swap buffers
```

`guides/windowing.md` has a worked `winit` example, and `examples/town-demo`
is the same app with the window replaced by a PNG writer.

---

## 7. Where to go next

| Goal | Document |
|---|---|
| Understand the whole engine | `01-architecture.md` |
| Generate terrain, roads and towns | `03-world-generation.md` |
| Materials, lighting, pixel-art rules | `04-rendering.md` |
| Camera feel and occlusion | `05-camera-and-visibility.md` |
| Movement and collisions | `06-physics.md` |
| A crowd of NPCs | `07-npcs.md` |
| Hit a frame budget | `08-performance.md` |
| Work on the engine | `contributing-for-ai.md` |

---

## Common mistakes

**"Everything is black."**
The framebuffer is linear and starts at zero. Either add a light
(`scene.add_light(Light::sun())`), raise the ambient term, or set `scene.background`.
An unlit material with a black base colour is also black, which is easy to do by
accident.

**"My object is missing."**
Work through `05-camera-and-visibility.md`'s checklist. The usual causes, in
order: it is outside the frustum; it is past `max_distance`; its bounding sphere
is under `min_screen_radius`; its bounds were never computed (call
`Scene::update_all_bounds` after spawning); or an occluder swallowed it.

**"The colours are wrong / washed out."**
You are looking at the linear framebuffer. Call `resolve()`.

**"Nothing moves."**
`Scene::set_transform` returns `false` for a stale handle. Also check that the
plugin's `update` is what you think: `Plugin::update` runs at the *fixed*
timestep, not the frame rate.

**"`app.physics_mut().body_mut(h).position = x` did nothing."**
Direct field writes are picked up on the next `step`, but the body's cached
broadphase state is not. Use `set_position`.

**"The world looks the same for every seed."**
Check you passed the seed through `AppConfig::seed` *and* that `--seed` reached
`WorldConfig`. `App::new` overwrites `world.seed` from `AppConfig::seed`
deliberately, so set one of them, not both.
