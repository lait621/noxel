# ADR 0007 — The GPU backend is a feature-gated extension point

**Status:** accepted (design), unimplemented
**Applies to:** `noxel-render`

## Context

ADR 0002 forbids third-party runtime dependencies, and ADR 0003 makes the
software renderer the default. A GPU backend needs `wgpu` (or an equivalent),
which is a large dependency tree and, on some platforms, link-time requirements
the default build should not carry.

At the same time, a 3D engine that cannot ever use the GPU is hard to take
seriously, and the two stated requirements — "support rasterization and ray
tracing" and "high performance" — both point at one eventually.

## Decision

**The GPU backend is specified now and implemented behind a feature flag.**

- `noxel-render` exposes `Renderer` as an **object-safe trait**. A GPU renderer is
  an implementation of that trait; nothing above it (visibility, app, npc,
  debug) changes.
- The trait's inputs are already GPU-shaped: one `Scene` with slot-map handles,
  one `CameraView` carrying `view`, `projection` and `view_projection`, one
  `VisibleSet` that is a flat list of `(instance handle, alpha, lod, distance)`,
  and one `Framebuffer` with linear HDR colour. Uploading a `VisibleSet` is an
  instance-buffer write; `Mat4` is column-major so it uploads without a
  transpose; clip depth is `[0, 1]` so it matches WebGPU and Vulkan directly
  (ADR 0001).
- The feature would be declared as below. **No `[features]` table exists in the
  workspace today** — this is the shape the implementation will take, not a
  description of the current tree, and adding `wgpu` would be the first
  third-party dependency the project has ever had:

```toml
[features]
default = []
gpu = ["dep:wgpu"]        # not enabled by default; see ADR 0002
```

- The intended implementation is a tiled software-style deferred pipeline:
  a G-buffer pass writing position/normal/albedo/material id, then a lighting
  pass over the same `VisibleItem` list, reusing `noxel-render::light` unchanged
  so lights behave identically to the CPU path.
- Ray tracing on the GPU (DXR/Vulkan RT) is **out of scope**: the CPU tracer is
  the reference, and hardware ray tracing is not portable enough to be the only
  path.

## What would break

- **Determinism** (ADR 0008) is not achievable on a GPU. A GPU renderer must be
  excluded from golden-image tests, and a test that needs a reference image must
  run on the software path.
- **Headless CI** without a GPU would need a lavapipe/software-Vulkan path. That
  is a supported configuration, but slower than the CPU rasterizer at these
  resolutions, which weakens the case for the GPU backend in the first place.
- **Zero dependencies** holds only for the default build.

## Consequences

The decision to specify rather than implement is deliberate. The port point is
real and tested (the trait is object-safe and `App` already switches between two
implementations at runtime), so adding a third is mechanical. Writing it before
it is needed would mean maintaining a path that CI cannot exercise — exactly the
fallback rot ADR 0003 warns about.

## Alternatives rejected

- **`wgpu` as a hard dependency**: breaks ADR 0002 for a backend most users of a
  pixel-art engine do not need.
- **A hand-written Vulkan/Metal/D3D12 backend**: three platform APIs, no
  dependencies, and a maintenance burden far beyond the rest of the engine
  combined.
