# Attaching a window

> **Update.** The engine now ships this. `crates/noxel-window` is a real crate
> with a `window` feature that opens a window, presents the framebuffer and reads
> player input — see [ADR 0011](../adr/0011-windowing.md) and run
> `./scripts/run-window.sh`. Use this guide when you want your *own* host instead:
> a different toolkit, a custom loop, or an embedded target. Everything below is
> about the seam, which is the same either way.


The engine has no *built-in* windowing layer: `App` produces a linear HDR
`Framebuffer` and consumes an `InputState`, and presenting that buffer is the
host's job. That is still true of every crate except `noxel-window`, which is one
such host, written and shipped so you do not have to write the first one. This is a
consequence of `adr/0002-no-dependencies.md`: a window needs a platform API, and
the engine will not carry one for every platform it might run on.

This guide shows the whole host loop, with `winit` as the example.
(`winit` is a dependency of *your game*, not of the engine. Add it to your own
crate's `Cargo.toml`.)

---

## The contract

```text
your host
  ├── fill app.input_mut() from platform events
  ├── app.step(real_dt)
  ├── let image = app.resolve()      // sRGB bytes, width * height * 4
  └── upload image.pixels to a texture and present
```

Three things to get right, and they are all easy to get wrong:

1. **Step with the real elapsed time, not a constant.** `App::step` feeds the
   fixed-step clock, which spreads the time into 0..n fixed updates. Passing a
   constant makes the game run at the wrong speed on a slow machine.
2. **Clamp a long stall.** `GameClock` already caps the substeps and drops the
   excess, so a debugger pause does not teleport the player.
3. **Resolve once per frame.** `resolve()` allocates an image; a host that calls
   it twice per frame doubles that cost for nothing.

---

## The loop

```rust,no_run
use noxel_app::{App, AppConfig};
use noxel_render::framebuffer::ResolveSettings;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(AppConfig::default())?;

    // ... create a window and a surface with your platform library of choice ...

    let mut last = std::time::Instant::now();
    loop {
        let now = std::time::Instant::now();
        let dt = (now - last).as_secs_f32().min(0.25);
        last = now;

        // 1. Input: translate the platform's events into the engine's state.
        //    `app.input_mut().press(key)`, `.release(key)`, `.mouse`, `.scroll`.
        for event in /* your event source */ std::iter::empty::<()>() {
            let _ = event;
            // match event { KeyPressed(k) => app.input_mut().press(k), ... }
        }

        // 2. Simulate and render one frame.
        app.step(dt);

        // 3. Present.
        let image = app.resolve();
        // upload `image.pixels` (RGBA8) to your swapchain, then present.
        let _ = &image.pixels;

        if /* window closed */ false {
            break;
        }
    }
    Ok(())
}
```

---

## Pixel-perfect presentation

The engine renders at `AppConfig::internal` (320x180 by default) and you upscale.
Two rules make the result look like pixel art rather than like a blurry
low-resolution 3D render:

- **Nearest-neighbour filtering.** A linear filter between the internal buffer and
  the window turns every hard pixel edge into a soft one.
- **An integer scale factor.** At 1.5x, some source pixels cover two destination
  pixels and some cover one, and the art shimmers as the window resizes.

`Viewport` in `noxel-render` computes the integer scale for you:

```rust
use noxel_render::renderer::Viewport;

let viewport = Viewport {
    width: window_width,
    height: window_height,
    internal_width: 320,
    internal_height: 180,
    pixel_perfect: true,
};
let scale = viewport.integer_scale();          // 1, 2, 3, ...
let (w, h) = viewport.scaled_size();           // the letterboxed image size
let (ox, oy) = viewport.letterbox_offset();    // where to place it
```

Letterbox the remainder rather than stretching: an aspect ratio of 16:9 at
320x180 cannot fill every window shape, and stretching breaks the art.

---

## Input

`InputState` is deliberately a plain data structure, not an event queue:

```rust
let input = app.input_mut();
input.press(65);          // held from now on
input.release(65);        // released; `was_released` is true for this frame
input.mouse = (x, y);     // in internal pixels
input.mouse_delta = (dx, dy);
input.scroll = ticks;
input.mouse_buttons[0] = true;
```

The per-frame edges (`was_pressed`, `was_released`, `mouse_delta`, `scroll`) are
cleared by `App::step` at the end of the frame, and `is_held` survives. On a focus
loss call `release_all()` — otherwise the character keeps walking while the window
is in the background.

`movement_axis(up, down, left, right)` returns a normalised `(x, z)` pair, which
is what a top-down game wants.

---

## Aspect ratio and the camera

The camera needs the aspect ratio to build its projection:

```rust
let aspect = 320.0 / 180.0;          // the INTERNAL aspect, not the window's
let view = app.camera_mut().view(aspect);
```

Using the window's aspect instead means the visible world changes as the window
resizes, which makes a pixel-perfect zoom impossible. The engine's convention is
that the internal resolution defines the view, and the window is a viewport onto
it.

---

## Where the app already does this

`examples/town-demo` is the same loop with the window replaced by a PNG writer:

```rust,no_run
# use noxel_app::{App, AppConfig};
# let mut app = App::new(AppConfig::default()).unwrap();
let report = app.run_headless(600);      // no window, real frames
println!("{}", report.summary());
```

Swapping in a window is a matter of replacing `run_headless` with the loop above.

---

## Common mistakes

**"It runs at double speed."**
You are stepping with a constant `dt` while the vsync presents at 60 Hz on a
144 Hz monitor. Pass the measured elapsed time.

**"The player teleports after alt-tab."**
A single frame with a 30-second `dt`. `GameClock` caps the substeps so the
simulation does not run for 30 seconds, but clamp the input `dt` too (`0.25` is a
reasonable ceiling).

**"The image is blurry."**
Linear filtering, or a non-integer scale. See above.

**"The world looks squashed."**
You passed the window's aspect to `camera.view`. Pass the internal aspect.

**"Input sticks after alt-tab."**
Nothing told the engine the keys came up. Call `input.release_all()` on a focus
loss.
