# Drawing a UI without a UI toolkit

Noxel ships no UI system (`adr/0009-no-ui.md`), but it does ship the primitives a
UI is built from, and they are exact: an overlay draws into the **linear**
framebuffer using the linear value of the sRGB colour you asked for, so a pixel
drawn in `#FF0000` resolves back to exactly `#FF0000`.

There are three levels you can work at.

---

## 1. Text and boxes — `noxel_debug::overlay`

```rust
use noxel_debug::overlay::{DebugPanel, PanelSlot};
use noxel_core::math::Color8;

let panel = DebugPanel::new(
    PanelSlot::BottomRight,
    vec!["HP 12/20".to_string(), "GOLD 143".to_string()],
);
panel.draw(&overlay, framebuffer, 0, 1.0);
```

`PanelSlot` anchors to a corner; `DebugPanel::draw` returns the next free y, so
panels stack. `DebugPanel::size` gives the pixel size if you need to lay something
out yourself.

---

## 2. Raw primitives — `noxel_render::overlay::Overlay`

| Call | Draws |
|---|---|
| `text(fb, x, y, text, color)` | a string in the built-in 3x5 font |
| `text_scaled(fb, x, y, text, color, scale)` | the same at an integer scale |
| `screen_rect(fb, x, y, w, h, color, filled)` | a rectangle |
| `screen_line(fb, x0, y0, x1, y1, color)` | a line |
| `panel(fb, x, y, &lines, fg, bg)` | a filled panel with a border |
| `crosshair(fb, x, y, size, color)` | a cross |
| `line/aabb/ray/cross(fb, camera, ...)` | 3D primitives, projected |

`Overlay::set_depth_test(false)` makes 3D primitives draw through the world, which
is what a debug view wants and what a world-space health bar does *not*.

The font is a 3x5 bitmap covering uppercase ASCII, digits and common punctuation.
It is deliberately not enough for dialogue: a game that needs real text brings a
font atlas (`noxel-asset::atlas` packs one) and draws quads.

---

## 3. A real UI — your own quads

For a menu with frames and layout, build meshes and draw them through the scene
with an orthographic camera. A 2D UI layer is:

```rust
use noxel_camera::TopDownCamera;
use noxel_camera::ProjectionMode;
use noxel_render::framebuffer::Framebuffer;
use noxel_render::scene::Scene;

/// A second camera looking at the UI plane.
fn ui_camera(height: f32) -> TopDownCamera {
    let mut camera = TopDownCamera::new(ProjectionMode::Orthographic { height });
    camera.set_smoothing(0.0);
    camera
}

fn draw_ui(app: &mut noxel_app::App, framebuffer: &mut Framebuffer) {
    // 1. Render the UI scene with an orthographic camera whose view height is
    //    the internal height in pixels, so one world unit is one pixel.
    // 2. Or, for a HUD, draw directly with `Overlay` after `app.render()`.
    let _ = (app, framebuffer);
}
```

For a HUD that never moves, the overlay path is simpler and faster. For a menu
with nine-slice panels, animated transitions and text boxes, a second scene with
an orthographic camera is the right shape: it reuses the renderer, the material
system and the texture atlas, and it costs one extra `Renderer::render` call into
the same framebuffer with the depth buffer cleared.

---

## Scaling the UI with the window

The internal resolution is fixed, so the UI is authored in internal pixels. A
health bar is 24 pixels wide at 320x180 and stays 24 internal pixels wide at any
window size — which is exactly what a pixel-art game wants.

If the game has a UI scale setting, scale the *authoring* numbers, not the
projection, or the text becomes a blurry upscale.

---

## Common mistakes

**"My text is invisible."**
The framebuffer is linear and starts at zero; a dark colour on a dark background
is invisible. Draw a filled background first (`screen_rect(..., true)`).

**"The colour is wrong."**
You passed a linear colour where an sRGB `Color8` was expected, or the reverse.
`Color8` is sRGB bytes and `Color` is linear radiance; `Color8::to_linear` and
`Color::to_srgb8` convert.

**"My overlay is drawn under the world."**
The overlay writes into the framebuffer *after* the scene, so it is on top by
construction. If it looks buried, the depth test is rejecting it: call
`set_depth_test(false)`.

**"The HUD scales with the window but the game does not."**
You are drawing with the window's dimensions instead of the internal ones. Use
`framebuffer.width()` and `framebuffer.height()`.
