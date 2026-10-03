# ADR 0003 — Software rendering first

**Status:** accepted
**Applies to:** `noxel-render`

## Context

Noxel targets pixel-art top-down RPGs. The reference resolutions are 320x180,
480x270 and 960x540 — between 58k and 518k pixels per frame, one to two orders of
magnitude below a modern 3D game. A frame contains a few thousand triangles.

An engine at that scale does not need a GPU, and paying for one costs three
things that matter a great deal here.

## Decision

**Ship a CPU rasterizer and a CPU ray tracer as the default renderers.** A GPU
backend is an explicit extension point behind a feature flag (ADR 0007), not a
requirement.

## Why

**Determinism.** A CPU renderer with no floating-point reassociation produces the
same bytes on every machine. That is what makes golden-image testing possible:
`noxel-debug::dump::compare` can assert a frame is *identical* to a committed
reference, and a rendering regression fails CI rather than being noticed by a
player three weeks later. A GPU's rounding, its driver's optimisation and its
vendor-specific precision make that assertion impossible to portably write.

**Testability without a display.** `App::run_headless(n)` renders real frames in
CI, on a build server, inside a container with no GPU. Every subsystem —
visibility, NPC crowds, streaming — is exercised against a real image.

**Zero dependencies.** The software path needs no windowing library, no shader
compiler, no swapchain. It is the only way the no-dependency rule (ADR 0002) can
hold for the default build.

## How it stays fast

A naive software rasterizer at 320x180 is fine; at 960x540 with 3000 triangles it
is not. Four things make it fast enough:

1. **Tile binning.** Triangles are binned into 32x32 screen tiles with a counting
   sort (`TileBins`). A tile's triangle list is contiguous, so rasterizing a tile
   touches a handful of cache lines instead of walking the whole scene.
2. **Disjoint band parallelisation.** The framebuffer is split into row bands
   whose `&mut` slices are provably disjoint, so `JobPool::parallel_for` can
   rasterize bands concurrently with no locks and no atomics on the pixel path.
   The parallel and sequential paths share the same `raster_tile_in_band`
   function, and a test asserts they produce identical pixels.
3. **A fill rule.** Shared triangle edges are claimed exactly once
   (`tie_owner`), which is what stops a tessellated ground plane from shading its
   interior seams twice — visible as a dark quilt on a large floor and as
   over-dark transparency along a quad's diagonal.
4. **Correct early-outs.** Back-face culling in screen space, near-plane clipping
   before projection, and a depth test per fragment.

## Consequences

- Shadow maps, post-processing and the ray tracer all run on the CPU. At 320x180
  this is a few milliseconds; a full ray-traced frame is a "still image" mode,
  which is exactly what it is for.
- The hybrid mode (raster primary + ray-traced shadows/AO) is the recommended
  high-quality path: it keeps the crisp pixel-aligned primary image and spends the
  ray budget where it shows.
- `Renderer` is an object-safe trait, so a GPU renderer is a drop-in
  implementation rather than a rewrite of the systems above it.

## Alternatives rejected

- **GPU-only**: loses determinism, testability and the zero-dependency property,
  and buys performance this project does not need.
- **GPU-first with a software fallback**: the fallback rots; two code paths with
  one exercised daily is one code path plus a liability.
