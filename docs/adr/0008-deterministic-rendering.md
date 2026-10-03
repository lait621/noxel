# ADR 0008 — Deterministic rendering and golden frames

**Status:** accepted
**Applies to:** `noxel-render`, `noxel-debug`, CI

## Context

Graphics code is the hardest part of an engine to test. A rendering regression is
usually found by a human looking at a screenshot, which means it is found late,
and only if someone looks.

A CPU renderer (ADR 0003) makes a stronger guarantee available: if no operation
depends on wall-clock time, thread scheduling, or a global random source, then two
runs of the same frame produce the **same bytes**. A test can then assert on those
bytes.

## Decision

**Rendering is deterministic.** No renderer, sampling or visibility code may
consult a clock, a thread id, or a global RNG.

Concretely:

- **Ray-traced samples** come from `sampling::Sampler`, a pure function of
  `(pixel x, pixel y, frame index, sample index)`. The test
  `determinism_two_runs_are_identical` renders the same scene twice and asserts
  the colour buffers are equal.
- **Camera shake** uses `CameraShake`'s hash noise indexed by a step counter, not
  by wall time, so replaying the trauma events replays the shake exactly.
- **Post-processing** has no temporal component when `temporal_blend == 0`.
- **Parallel rasterization** splits the framebuffer into disjoint row bands and
  performs no floating-point reassociation, so the parallel and single-threaded
  paths produce identical pixels. `parallel_and_sequential_rendering_agree`
  asserts it.
- **Visibility** produces its `VisibleSet` in scene order and snapshots cull
  reasons only when `track_cull_reasons` is set.
- **Culling and fading** are frame-count based, not time based, where it matters.

`noxel-debug` turns this into a workflow: `FrameDumper` writes
`frame_000123.png`, and `dump::compare` reports the differing-pixel ratio, the
maximum channel delta and the RMSE, with `ImageDiff::within(ratio, delta)` as the
assertion a test makes. `dump::diff_image` produces a side-by-side comparison
with differing pixels tinted magenta.

## What a golden test looks like

```rust
let report = app.run_and_capture(60);
let reference = png::decode(&std::fs::read("tests/golden/town.png")?)?;
let diff = dump::compare(&report.last_image.unwrap(), &reference);
assert!(diff.within(0.0, 0), "{}", diff.summary());
```

A zero-tolerance assertion is possible precisely because the pipeline is
deterministic.

## Consequences

- Adding a temporal effect (TAA, motion blur, frame-rate-dependent post) requires
  making it explicitly opt-in and recording the frame index it depends on.
- The ray tracer's temporal accumulation is driven by an explicit frame counter,
  and `RayTracer::reset_accumulation` exists so a caller can force a clean frame.
- A GPU backend cannot make this guarantee (ADR 0007), which is one more reason
  the software path stays the default.

## Alternatives rejected

- **Tolerance-based fuzzy image comparison**: needs a tolerance per scene, hides
  real regressions, and lets a small systematic error accumulate silently.
- **Compare structural hashes of the scene instead of pixels**: catches scene
  bugs but not shading bugs, which are the ones that actually break.
