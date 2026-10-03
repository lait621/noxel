# ADR 0001 — Coordinate system, units and handedness

**Status:** accepted
**Applies to:** every crate

## Context

An engine's coordinate conventions leak into every file that touches geometry.
Getting them wrong is expensive to undo, and "which way is up" disagreements
between the renderer, physics and world generation are the classic source of
mirrored levels and inverted normals.

## Decision

| Property | Choice |
|---|---|
| Handedness | **Right-handed** |
| Up | **`+Y`** |
| Forward | **`-Z`** (the OpenGL convention) |
| Rotation | **Radians**, counter-clockwise about the positive axis |
| Matrix layout | **Column-major**, so `Mat4::cols` matches a GLSL `mat4` upload directly |
| Clip-space depth | **`[0, 1]`** (Vulkan/WebGPU/D3D), not `[-1, 1]` |
| World unit | **1 metre** |
| Tile size | 1 world unit by default (`WorldConfig::tile_size`) |
| Yaw | `0` looks along `-Z`; `Vec3::from_yaw(y) == (-sin y, 0, -cos y)` |
| Pitch | Positive looks *down*, `PI/2` is straight down |

## Why these choices

**`-Z` forward with `+Y` up** matches the overwhelming majority of 3D content and
every tutorial a developer has read. A right-handed system with `-Z` forward
makes the camera basis `(right, up, -forward)`, which is why
`Mat3::from_forward_up` computes `right = forward.cross(hint)` — a mirrored basis
(determinant `-1`) silently flips triangle winding and makes back-face culling
reject the wrong half of the world.

**Column-major** means `Mat4` can be handed to a GPU buffer with no transpose.
`Mat4::get(row, col)` indexes as `cols[col][row]`, which is the opposite of the
field order and is the single most common source of confusion in this file; it is
implemented with an explicit `match` so it stays `const`-evaluable.

**Depth `[0, 1]`** is where every modern API is heading. A `[-1, 1]` projection
would need a remap before it could be used with WebGPU or Vulkan, and the remap
is easy to place in the wrong stage.

**Yaw of 0 looking along `-Z`** is the only convention consistent with
`Vec3::from_yaw`. `Quat::to_yaw` therefore extracts
`wrap_angle((-forward.x).atan2(-forward.z))`, and this sign has been wrong twice
during development — the test `to_yaw_roundtrips` exists because of that.

## Consequences

- A top-down camera looks along `-Y` with `up = -Z` (`CameraView::orthographic`
  with a world-up hint), which is why `noxel-camera` derives the screen-up
  vector from the yaw when the pitch approaches `PI/2`: at exactly straight down
  the world up vector is parallel to the view direction and `look_at_rh` cannot
  build a basis from it.
- Meshes wind **counter-clockwise when seen from outside**. Every procedural
  primitive in `noxel_render::mesh` has a test asserting its triangles face
  outwards.
- Physics uses the same units, so 1 m/s is a plausible walking speed and gravity
  is `-9.81 m/s^2`.

## Alternatives rejected

- **`+Z` forward, `-Y` up** (the "left-handed, Z-up" convention used by some CAD
  and older engines): unfamiliar to most artists and developers, and the extra
  mental hop costs more than it saves.
- **`[-1, 1]` depth**: cheaper to derive from the standard OpenGL projection, but
  wrong for the backends we intend to support.
- **Tiles in pixels rather than metres**: world-space pixel units make physics
  parameters resolution-dependent, so a game that changes its zoom level would
  have to retune gravity.
