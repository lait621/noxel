# ADR 0012 — A UI layer

**Status:** accepted, supersedes [ADR 0009](0009-no-ui.md)
**Applies to:** `noxel-ui`, the whole workspace

## Context

[ADR 0009](0009-no-ui.md) decided that Noxel would ship no UI toolkit, on the
grounds that the original requirement was explicit: *no UI/UX is needed; the
deliverable is documentation instead*. That reasoning was sound and is not being
revisited. What has changed is the premise.

The engine now has a game built on it (`games/noxel-valley`), and that game needs
a clock, a hotbar, an inventory, a shop, a shipping bin, a morning report and a
control reference. Every one of those is text, rectangles, hit testing and
layout — the four things ADR 0009 explicitly declined to provide.

The result of declining them was predictable and is the reason this ADR exists:
each screen re-derived its own panel drawing, its own text measuring and its own
hit testing, from `noxel_render::overlay`. The overlay is a good debug
instrument and a poor foundation:

| What a game UI needs | What `overlay` offers |
|---|---|
| lowercase, punctuation, any non-Latin script | 3x5 uppercase ASCII, one face |
| wrap a label to a panel width | no wrapping; measure and draw disagree on unknown characters |
| clip a child to its parent | no clipping at all |
| nine-slice a button at any size | filled rectangles and one-pixel borders |
| hover, press and click, in that order | no input of any kind |
| a pointer in framebuffer pixels | the window reports window pixels and nothing converts |

Three of those are not gaps but *bugs waiting to happen*: text that measures
differently from how it draws overlaps, and a panel with no clip bleeds across
the screen the first time a child is bigger than its parent.

## Decision

**Noxel ships a UI layer, as a crate: `noxel-ui`.**

It is a drawing and hit-testing layer, not an application framework. It provides:

1. **`FontSet`** — atlas-backed text with **face fallback**. One set holds a
   hand-drawn Latin face and a rasterised Chinese face, and a string mixing the
   two draws correctly without the caller knowing which character belongs to
   which. The line box is *derived* from the faces a string actually used, so
   9px Latin and 14px Chinese share one baseline instead of two.
2. **`Painter`** — clipped, integer-space drawing into the linear framebuffer:
   fills, borders, gradients, sprite blits, and nine-slice frames that deform
   correctly when the destination is smaller than its borders.
3. **`Theme`** — colours, spacing and frame styles as a **plain value**. Every
   style has a flat fallback, so a game builds its whole interface before a
   single texture exists and upgrades to authored art by filling in rectangles.
4. **`UiState`** — one frame of hover/press/click state, a queued-tooltip list,
   and the one mechanism by which a UI keeps a click away from the world:
   `pointer_over_ui()`.
5. **Widgets** — panel, label, heading, button, icon button, progress bar,
   inventory slot, list row, checkbox, stepper, divider, tooltip.
6. **`UiRect`** — integer layout arithmetic: `columns`, `rows`, `cut_top`,
   `inset`, `place`, `stack`.

And, in the engine around it:

7. **`noxel-window::Presentation`** — one derivation of the integer scale and the
   letterbox offset, used by *both* the presenter that scales the image up and
   the input path that scales the cursor down. Two independent derivations is how
   a cursor ends up off by the letterbox offset, which is invisible at one window
   size and obvious at every other.
8. **Mouse edges** — `mouse_pressed` / `mouse_released` alongside the level, and
   a cursor already converted into framebuffer pixels.

## What ADR 0009 got right, and is kept

The boundary. This crate has:

- **No screen stack and no state machine.** The game owns which screen is open.
  `noxel-ui` has no notion of a "current screen" and cannot acquire one.
- **No retained widget tree.** Every widget is a function of the frame. There is
  no place for a widget to be stale relative to the data it displays.
- **No layout engine.** Layout is `UiRect` arithmetic at the call site. A
  constraint solver would be a second thing to debug when a panel is one pixel
  off, and the arithmetic reads fine.
- **No text input, no IME, no clipboard.** A game that wants a text field builds
  it against `UiState::set_focus`.
- **No animation, no theming language, no editor.**
- **Determinism holds.** Nothing in the crate reads a clock, a thread id or a
  global. `UiState::time` advances by the frame delta the caller passes in, so a
  UI replay is reproducible like every other frame.

The debug overlay in `noxel-render` is unchanged and still the right tool for
seeing what the visibility and physics systems are doing.

## Consequences

- `noxel-ui` sits at **layer 3** in the crate DAG (core, asset, render), beside
  `noxel-camera` and `noxel-debug`. It adds no dependencies and keeps the
  workspace's zero-third-party rule intact — the font atlas is baked by a
  development-time Python tool and loaded as a PNG plus JSON through
  `noxel-asset`, exactly like every other asset.
- A game draws its UI in `Plugin::draw`, which runs **before** the sRGB resolve.
  The interface is therefore composited in the same linear space as the world,
  one tone curve applies to both, and a colour authored as `#FF0000` resolves to
  exactly `#FF0000`. Drawing after `resolve()` would break the engine's colour
  guarantee at the last possible moment.
- Text rendering is no longer a 3x5 uppercase bitmap. The bake
  (`tools/fontgen`) produces a hand-authored 5x7 Latin face plus the GB2312
  common-character set, both thresholded to one bit at an exact pixel size —
  because an antialiased glyph edge at 320x180, upscaled six times, is a grey
  blur six pixels wide.
- The cost is a subsystem to maintain. It is bounded by the six decisions above;
  anything that would grow it past them is a new ADR.

## Alternatives rejected

- **Keep no UI; let each game bring its own.** This is what happened, with a
  game, and it produced seven screens each with their own idea of what a panel
  is. The engine's own demo already contains a `hud.rs` that is 40% of a UI
  framework with none of the guarantees.
- **An immediate-mode UI library ported from elsewhere.** Every candidate is a
  dependency, which [ADR 0002](0002-no-dependencies.md) rules out, and each
  carries a layout model, a styling system and a widget set much larger than
  this game needs.
- **A retained-mode scene-graph UI.** Wrong for a HUD, and it would make "the
  inventory is out of date" a possible state — which is exactly what an
  immediate-mode UI makes impossible.
- **Draw the UI as textured quads through a second orthographic scene.** This is
  the shape `docs/guides/ui.md` sketches, and it reuses the renderer. It also
  costs a texture upload per text change, makes pixel-exact text a matter of
  getting the orthographic projection exactly right, and cannot clip a child to
  its parent without a scissor rect the renderer does not have. Drawing directly
  into the framebuffer is simpler and exact.
