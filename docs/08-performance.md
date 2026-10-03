# Performance

Noxel targets 320x180 to 960x540 with a few thousand triangles per frame, on the
CPU, with no GPU and no dependencies. At that scale the frame budget is spent in
a handful of places, and almost all of the tuning is a choice about *how much
work to do* rather than how fast to do it.

This guide covers where the time goes, how to measure it, which levers actually
move it, and how to write a performance test that does not fail on a slower
machine.

## Where the frame time goes

`noxel-debug` seeds one budget per section of the frame. These names are the real
ones — `Stats::new` creates exactly this list, and the report and the overlay
print them:

| Budget | Allowance | What it covers |
|---|---|---|
| `update` | 4.0 ms | the fixed-update phase: plugin `update` hooks (gameplay) |
| `physics` | 4.0 ms | `PhysicsWorld::step` |
| `visibility` | 1.5 ms | frustum, distance, size, occlusion and fades |
| `stream` | 3.0 ms | `WorldStreamer::update` — chunk generation and eviction |
| `npc` | 4.0 ms | the crowd step (`docs/07-npcs.md`) |
| `render` | 6.0 ms | the whole render pass: geometry, shading, post |

Two things about that table. First, the allowances **overlap**: `update` is the
whole fixed-update phase, so `physics` is inside it, and `render` is the entire
render call, not one stage of it. They are per-section alarm thresholds, not
slices of a pie, and they add up to more than a 60 fps frame on purpose so a
single section can be diagnosed without moving the others.

Second, only some of them are recorded by the engine today. `DebugSystem` fills
`render` (`record_render` calls `stats.spend("render", stats.ms_total, 0.0)`),
and a game fills the rest by calling `record_section(name, ms)`. The other
`FrameSample` fields — `visibility_ms`, `stream_ms`, `npc_ms`, `physics_ms` —
exist and are printed nowhere unless a game sets them; `App` records chunk counts
as counters instead. If you want per-section numbers, record them yourself at the
call site:

```rust
let started = noxel_core::time::Stopwatch::start();
// ... do the work ...
let ms = started.elapsed_ms() as f32;
app.debug_mut().record_section("physics", ms);
```

`FrameSample::unattributed_ms()` is `frame_ms - update_ms - render_ms`, clamped at
zero, which is the number to look at when the frame is slow and no budget is
over.

## How to measure

| Tool | What it gives you |
|---|---|
| `cargo run -p town-demo -- --stats` | one settled frame plus the full `DebugSystem::report()` text |
| `cargo run -p town-demo -- --world-info` | what the seed generates, no rendering — the cheapest way to isolate generation cost |
| the frame-time graph | `DebugConfig::frame_graph`, drawn by `DebugSystem::draw` from `Stats::frame_times` |
| `HeadlessReport` | mean, p95, worst, fps, fragments, rays, peak instances, resident chunks, chunk bytes, frames dumped |
| `FrameDumper` | PNG, PNG+depth, or raw RGBA per frame, plus `dump::compare` and `diff_image` |
| `Stopwatch` / `Profiler` | per-section and hierarchical CPU timing |

`App::run_headless(frames)` returns a `HeadlessReport`. `p95_frame_ms` is the
number to trust, not `mean_frame_ms`: a build can average 60 fps while stuttering
twice a second, and the stutter is the only thing a player notices.
`HeadlessReport::within_budget(ms)` is the assertion form, and `summary()` is the
one-line log form.

```rust
let mut app = noxel_app::App::new(noxel_app::AppConfig::headless())?;
// ... add plugins ...
let report = app.run_headless(600);
assert!(
    report.within_budget(16.67),
    "p95 {:.2} ms — {}",
    report.p95_frame_ms,
    report.summary()
);
```

`FrameDumper` writes `frame_000123.png` and can also write a depth table
(`DumpFormat::PngAndDepth`) or raw little-endian RGBA (`DumpFormat::Raw`) for a
byte-exact comparison. `dump::compare(a, b)` returns an `ImageDiff` with
`differing`/`total`, `max_channel_delta` and `squared_error`, and
`ImageDiff::within(max_ratio, max_channel_delta)` is the golden-image assertion.
Because rendering is deterministic (`docs/adr/0008-deterministic-rendering.md`),
`within(0.0, 0)` is a legitimate assertion rather than a flaky one.

### Profiling a section

```rust
use noxel_core::time::Profiler;

let mut profiler = Profiler::new();          // 16.67 ms budget, enabled
// Per frame:
profiler.begin_frame(0.0);
{
    let token = profiler.scope_begin("visibility");
    // ... work ...
    profiler.scope_end(token);
}
profiler.scope("physics", || {
    // ... work ...
});
profiler.end_frame();
if profiler.over_budget() {
    println!("{}", profiler.last_frame().to_table());
}
```

`Profiler` is hierarchical: `ScopeSample` carries `depth`, `parent`, `calls` and
`ms`, `FrameProfile::scope(name)` looks one up, `walk` visits every scope with
its depth, and `to_table()` renders the indented report. `Profiler::disabled()`
records nothing, so a shipping build pays nothing.

## The levers, in order

Work down this list. The first three are worth more than everything after them.

| # | Lever | Where | Rough benefit | Cost |
|---|---|---|---|---|
| 1 | **Internal resolution** | `AppConfig::internal` | quadratic: 480x270 → 320x180 is 2.25× fewer pixels | sprites get chunkier; the letterbox changes with `Viewport::integer_scale` |
| 2 | **View distance** | `WorldConfig::view_distance_chunks` | quadratic in chunks loaded: 6 → 4 is 81 chunks instead of 169 | the world ends visibly closer; raise `keep_distance_chunks` for hysteresis or chunks thrash |
| 3 | **`min_screen_radius`** | `CullSettings` | proportional to the instance count below the threshold | small objects pop out entirely; 0.45 px is the pixel boundary, below that you are culling things that could not be seen anyway |
| 4 | **Occlusion culling** | `OcclusionSettings::cull_occluded` | large in a dense town, ~0 in open country | "why is that missing?" bugs; use `cull_max_screen_radius` to keep big objects out of it |
| 5 | **LOD / update fraction** | `LodLevels`, `LodSelection` | 2–10× on the crowd for tiers 1–3 | distant agents update less often; visible only if the thresholds are set too aggressively |
| 6 | **NPC tiers** | `docs/07-npcs.md` | proportional to how many agents leave the near tier | steering quality at a distance |
| 7 | **Chunk mesh granularity** | your terrain plugin | fewer, larger instances: one mesh per chunk beats one per tile | coarser culling granularity and larger rebuild spikes when a chunk arrives |
| 8 | **Shading mode** | `ShadingMode` | `Raster` → `Hybrid` is the expensive direction; `Hybrid` → `Raster` is the cheap one | ray-traced shadows and AO |
| 9 | **Shadow-map size** | `RenderSettings::shadow_map_size` | quadratic in texels: 2048 → 1024 is 4× fewer | softer/blockier shadows; also check `Light::Directional::shadow_extent`, since a big extent over a small map is the same loss with no saving |

Two levers that look like performance knobs but are not:

- **`RenderSettings::backface_culling`** is already on and already cheap; turning
  it off halves nothing and breaks closed meshes' depth behaviour.
- **`temporal_blend`** in the ray tracer trades frames for samples, not quality
  for speed. It is the right lever for a still, not for gameplay.

## Writing a performance test that is not flaky

A test that asserts a *duration* measures the machine, not the code. It passes on
your laptop, fails in CI, and tells you nothing when it fails. Assert an
**invariant** instead — a property that must hold whatever the machine:

| Instead of | Assert |
|---|---|
| "1000 agents update in 6.4 ms" | `stats.active == 1_000 && stats.last_step_ms < 8.0` |
| "frames take 4 ms" | `report.within_budget(16.67)` on p95, and `report.fragments` inside a known range |
| "streaming is fast" | `stats.generated_this_update == 13 * 13` on the first update and `0` on the second |
| "memory does not grow" | `streamer.memory_bytes()` below a bound after a 40-step walk |
| "the world is stable" | two generation orders produce equal bytes |

The suite already contains the patterns to copy:

- `second_update_at_the_same_place_is_free` asserts an *exact* work count
  (`generated_this_update == 0`, cache size unchanged) rather than a duration.
- `memory_stays_bounded_over_a_long_walk` asserts a bound across 40 updates.
- `heights_are_continuous_over_a_small_step` compares *derived* numbers: halving
  the sampling step must roughly halve the largest change. That catches a
  discontinuity without knowing the machine's speed.
- `resting_box_does_not_sink_over_six_hundred_steps` runs 600 steps and asserts a
  positional invariant (within a millimetre of the analytic height).
- `determinism_is_bit_exact` runs the same scenario twice and compares state, and
  `parallel_and_sequential_rendering_agree` compares two rendering paths.

When a duration genuinely is the thing under test, measure a **ratio** between
two configurations on the same machine in the same run (coarse vs fine, 1000 vs
100 agents) and assert the ordering, not the absolute value. That survives a slow
CI box; an absolute threshold does not.

### Measuring a change honestly

1. **Use the release profile.** The workspace's `dev` profile is `opt-level = 1`
   with `overflow-checks = true`; `release` is `opt-level = 3`, fat LTO, one code
   unit and no overflow checks. A debug number is not a smaller version of the
   real number, it is a different number.
2. **Skip the warm-up.** The first frames generate chunks and build the BVH. The
   demo runs 12 warm-up frames before it starts reporting, for exactly this
   reason; do the same.
3. **Hold everything else fixed.** Same seed, same internal resolution, same
   `fixed_dt`, same frame count, same machine, same power state.
4. **Compare p95, not mean.** A change that trades a rare spike for a better mean
   may still feel worse.
5. **Change one lever at a time**, and note the settings with the numbers. A
   performance figure without the seed and the resolution is not a measurement.

## Common mistakes

- **Benchmarking a debug build.** `cargo run` uses the `dev` profile: `opt-level
  1` plus overflow checks. Use `cargo run --release` and expect a large multiple.
- **Reporting the first frame.** It pays for chunk generation, the BVH build and
  the shadow-map allocation. Warm up, then measure.
- **Trusting `mean_frame_ms`.** A mean hides the hitch. `p95_frame_ms` and
  `worst_frame_ms` are in the same report — read them.
- **Trusting `HeadlessReport::fps` as a frame-rate.** `run_headless` renders as
  fast as it can and never sleeps; its `fps` is throughput, not the pacing a
  player would see. For pacing, run `RunMode::Realtime { seconds, fps }`, which
  still does not sleep — the *host* owns the real clock.
- **Measuring with the debug overlay on.** The overlay costs fragments, a
  per-frame panel build and, with `wireframes`, a query per drawn box.
  `DebugConfig::minimal()` or `disabled()` for measurement.
- **Forgetting that `record_section` only works when `record_sections` is on.**
  `DebugConfig::disabled()` sets it false and your measurements silently
  disappear.
- **Blaming the renderer for a slow frame.** Check the counters first: a large
  `culled/distance` count with a small `drawn` count means visibility is doing its
  job; a huge `considered` count means the scene has more instances than it
  should, which is an authoring problem, not a rendering one.
- **Growing `max_cached_chunks` to "smooth out" streaming.** It raises memory
  without changing the generation cost, and the hard cap exists precisely so a
  long walk cannot allocate without bound. Tune `view_distance_chunks` and
  `keep_distance_chunks` together instead.
- **Tuning `min_screen_radius` below ~0.45 px on a pixel-art target.** At 320x180
  a sub-pixel object costs a full instance of setup and contributes nothing; you
  are paying for the culler not to work.
