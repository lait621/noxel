# Contributing as an AI agent

This repository is written substantially by AI agents, one crate at a time, in
parallel. That workflow has one dominant failure mode: **code that compiles,
looks plausible, and is subtly wrong**. This document is the operating manual for
avoiding it.

If you read only one section, read "Deciding whether the code or the test is
wrong" and the pitfalls list at the end.

## The crate DAG

Dependencies point **one way**, downwards. A crate may depend on anything above
it in this list and nothing below it, and there are no cycles.

| Layer | Crate | Depends on |
|---|---|---|
| 0 | `noxel-core` | — |
| 1 | `noxel-ecs` | core |
| 1 | `noxel-asset` | core |
| 1 | `noxel-physics` | core |
| 2 | `noxel-render` | core, asset |
| 2 | `noxel-world` | core, asset |
| 3 | `noxel-camera` | core, render |
| 3 | `noxel-ui` | core, asset, render |
| 3 | `noxel-debug` | core, asset, render |
| 4 | `noxel-visibility` | core, ecs, render, camera |
| 5 | `noxel-npc` | core, ecs, physics, world |
| 6 | `noxel-app` | all of the above |
| — | `tools/noxel-gen` | core, asset |
| — | `examples/town-demo` | all of the above |
| — | `games/noxel-valley` | all of the above |

```text
core ─┬─ ecs ─────────────────────────────┐
      ├─ asset ─┬─ render ─┬─ camera ─────┤
      │         │          ├─ ui ─────────┤
      │         │          ├─ debug       │
      │         └─ world ──┼─ npc ────────┤
      └─ physics ──────────┘              └── app
```

Rules that fall out of the DAG:

- **Nothing below may know about anything above.** `noxel-physics` cannot know
  what a `Scene` is; `noxel-world` cannot know what a camera is. When a lower
  layer seems to need something from a higher one, the answer is a data type in
  the lower crate (a `ColliderShape`, an `Aabb`, a `&[RoadSegment]`) or a
  callback.
- **`noxel-core` is the only crate everyone shares.** Anything you put there is
  paid for by every other crate, so it has to be general and heavily tested.
- **Adding an edge is a design decision**, not a convenience. If you need a new
  dependency, check whether an existing lower-layer type already carries the data;
  if it does not, the type probably belongs in `noxel-core`.

Verify the graph rather than trusting this table:

```bash
grep -A12 '^\[dependencies\]' crates/*/Cargo.toml
```

## Hard rules

Every crate's `lib.rs` starts with:

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]
```

| Rule | Consequence for you |
|---|---|
| `forbid(unsafe_code)` | there is no escape hatch. Slice indices, casts and invariants must be expressed in safe Rust. Where that is genuinely awkward, the crate documents the workaround (see `PaletteFile::to_palette`, which leaks entry names on purpose, bounded by the palette size) |
| `deny(missing_docs)` | **every public item needs a doc comment**, and `cargo doc` failures are build failures. Module-level `//!` docs are expected to explain *why* the module exists, not restate its name |
| `warn(clippy::all)` | CI treats clippy warnings as errors; see the command below |
| zero third-party dependencies | `[workspace.dependencies]` lists only Noxel crates and `std`. No `serde`, no `glam`, no `rand`, no `rayon` — see `docs/adr/0002-no-dependencies.md`. A GPU backend is *specified* as feature-gated (`docs/adr/0007-gpu-backend.md`) but no `[features]` table exists yet |
| edition 2024, `rust-version` 1.85 | `rust-toolchain.toml` pins `stable` with `rustfmt` and `clippy`. Edition 2024 makes `gen` a reserved keyword (see the pitfalls) |
| `rustfmt.toml` | `max_width = 100`, `hard_tabs = false`, `tab_spaces = 4`, Unix newlines, `reorder_imports`, `use_small_heuristics = "Default"` |
| no application framework | `docs/adr/0012-ui-layer.md`: `noxel-ui` draws, hit-tests and lays out; it has no screen stack, no retained widget tree, no layout engine and no text input. A screen is the game's own enum, and the UI cannot acquire one |
| determinism | no renderer, sampling, generation or NPC code may read a clock, a thread id or a global RNG — `docs/adr/0008-deterministic-rendering.md` and `docs/adr/0006-deterministic-generation.md` |

The build profiles matter when you are reasoning about performance:

| Profile | Settings |
|---|---|
| `dev` | `opt-level = 1`, `debug = true`, `overflow-checks = true` — **not** a performance measurement |
| `release` | `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"`, symbols stripped |
| `bench` | inherits release, with debug info and symbols kept |

## Commands

`cargo` is installed under `~/.cargo/bin`, which is **not on `PATH` in a fresh
non-login shell on this machine**. Every command below starts with:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Then, from the repository root:

```bash
cargo fmt                                       # format everything
cargo fmt --check                               # what CI would reject
cargo clippy --all-targets -- -D warnings       # lints, warnings are errors
cargo test                                      # the whole suite
cargo test  -p noxel-world --lib                # one crate, one target
cargo test  --workspace --exclude town-demo --no-fail-fast   # keep going past a failure
cargo doc   --workspace --no-deps --open        # the API reference
cargo check --workspace --all-targets           # the fastest "does it build" signal
```

Notes that save time:

- `cargo test` stops at the first failing *crate*. Use `--no-fail-fast` when you
  want the whole picture.
- `cargo test -- --nocapture` shows `println!` output from tests.
- The dev profile has `overflow-checks = true`, which is deliberate: an
  arithmetic overflow panics in a test instead of wrapping silently.
- Measure performance in `--release` only. In `dev`, `opt-level = 1` is roughly
  an order of magnitude off the real number, and it is a different number rather
  than a smaller one.

`.github/workflows/ci.yml` runs exactly these steps on push and on a pull
request: format, build, clippy with `-D warnings`, test, doc, `noxel-gen verify`
and a headless demo render. `scripts/check.sh` runs the same gate locally, with
`--fast` to skip the release build and the demo. Chat about a command here and
in the workflow, or the two will drift.

## Conventions from the testing strategy

`docs/adr/0010-testing-strategy.md` is the authoritative document. The parts that
change how you write code:

1. **Every crate carries unit tests inside its own modules**, and every
   non-obvious invariant has a test that names it.
2. **Test names are sentences**: `parallel_and_sequential_rendering_agree`, not
   `test_render_2`.
3. **Assert on a value the test computes from the geometry**, never a number
   copied out of a failing run. `nearest_finds_the_closest` asserts a distance in
   `8.5..9.0` because the test can show the nearest point is 8.7 away.
4. **A test that relies on an ordering asserts the ordering**, not an absolute
   value.
5. **No test depends on wall-clock time** except the ones that explicitly measure
   it, and those assert an ordering invariant, never a duration.
6. **Every `unsafe`-adjacent assumption is a test**: index arithmetic, bitset
   iteration, generational handle reuse, `swap_remove` re-pointing.
7. **Determinism gets an explicit test** in every system with a random-looking
   input. The canonical shapes: `two_generation_orders_agree`,
   `determinism_is_bit_exact`, `parallel_and_sequential_rendering_agree`.

Four categories, in increasing order of value: contract tests (API shape and
defaults), invariant tests (what must hold), derived-number tests, and
golden-image tests (`noxel-debug::dump::compare` against a committed PNG).

## Deciding whether the code or the test is wrong

**When a test fails, decide which is wrong: the code or the test. Both happen.**
This is the most important judgement call in the repository, and getting it
backwards is how a suite stops being evidence.

Work through it in this order:

1. **Read the failure, not the test name.** The assertion message and the values
   on both sides tell you what the code believes.
2. **Ask what the test is asserting and where that claim comes from.** A test
   that asserts a documented invariant ("an unlit sprite round-trips to the
   authored bytes", "every procedural mesh winds outward") is stating a contract.
   The code is wrong.
3. **Ask whether the expectation was derived or guessed.** If the expected number
   is a literal someone pasted from a previous run, the test is measuring a
   coincidence. Re-derive it and rewrite the test with the derivation in a
   comment.
4. **Check the units.** A surprising number of "bugs" are `radians` versus
   `degrees`, `[0, 1]` versus `[-1, 1]` depth, or a yaw sign.
5. **Check whether the test is asserting more than the code promises.** A test
   that requires exact float equality on a smoothed value is wrong; a test that
   requires exact equality on a *round trip through the palette* is right.
6. **If you change the test, explain why in a comment.** The next agent will
   otherwise assume the test was right and "fix" it back.

### Worked examples where the test was right

These are real regressions this suite caught during development. Each one is a
shape you should recognise.

| Bug | Symptom | What the test asserted |
|---|---|---|
| Inverted ambient hemisphere blend | a surface facing up was darker than one facing down | `ambient_hemisphere_blend`: an up normal gets more sky than ground |
| `damp_factor(0)` freezing instead of snapping | the camera never arrived, and a `dt == 0` frame teleported it | `smoothing_zero_snaps` (camera) plus the composition property in `damp_factor`'s own doctest |
| A rasterizer with no depth test | the far surface drew over the near one | `depth_buffer_keeps_the_nearer_surface`: two planes, the nearer must win |
| A font truncating non-ASCII to a byte | `text_width` and the pen disagreed on multi-byte characters, so text overlapped | `font_missing_glyph_is_none` and `text_width_math`: width is a function of `chars()`, not bytes |
| A bloom pass double-blitting | the glow was roughly twice as bright as intended | `bloom_is_energy_positive_but_bounded`: adding bloom must not multiply the image |
| Shared triangle edges shaded twice | a dark quilt along a tessellated floor's seams and a dark diagonal on transparent quads | the fill rule itself, plus `parallel_and_sequential_rendering_agree` (the two paths must not disagree at the seams) |
| A UV sphere pole artefact | the pole ring collapses to one point, so the winding test saw zero-area triangles with meaningless normals | `sphere_faces_wind_outwards` |

The last row is the useful counter-example: the *test* was wrong there, not the
code. `Mesh::sphere` genuinely emits degenerate triangles at both poles (a UV
sphere's top and bottom rings collapse to a point), and that is harmless — the
rasterizer drops them via `ScreenTriangle::is_degenerate` (`area.abs() < 1e-7`)
before they reach a pixel. The fix was to skip zero-area triangles in the winding
test, with a comment saying why, rather than to "fix" the mesh. If you see a
failing test on degenerate geometry, check whether the degeneracy is inherent to
the construction first.

## Pitfalls

Each of these has cost someone real time in this codebase.

| Pitfall | Detail |
|---|---|
| **Column-major indexing** | `Mat4::get(row, col)` indexes `cols[col][row]`, the opposite of the field order. It is the single most common source of confusion in `math/mat.rs`, which is why it is implemented with an explicit `match` so it stays `const`. When a transform looks transposed, check this first |
| **`-Z` is forward** | Right-handed, `+Y` up, `-Z` forward, yaw `0` looks along `-Z` (`Vec3::from_yaw(0) == (0, 0, -1)`), positive pitch looks down, `PI/2` is straight down. A `Quat::to_yaw` sign error has happened twice; `quat_yaw_round_trips` exists because of it (`docs/adr/0001-coordinate-system.md`) |
| **`ChunkPos::y` is Z** | A chunk coordinate is `{ x, y }` where `y` is the world **Z** axis. `chunk.pos.y` is not a height, and `size.x`/`size.z` on an `Aabb` are the horizontal extents while `size.y` is up |
| **`gen` is a reserved keyword in edition 2024** | the generator module is declared `pub mod r#gen;` and reached as `noxel_world::r#gen`. The crate root re-exports its public items, so callers normally never type the raw identifier |
| **`Scene::Instance::bounds` is private** | `Instance` exposes `bounds()` as a method; the field cannot be assigned. Write `instance.transform`, then call `Scene::update_bounds(handle)` or `update_all_bounds()`, or the renderer and the culler will keep using the old box |
| **`Light` has no builder methods** | `Light` is an enum with public fields. There is no `with_direction`/`with_intensity`; construct `Light::Directional { .. }` or match on the variant. The same is true of `Ambient` and `Fog` (though `Fog`, `Ambient::night` and `Ambient::interior` are presets) |
| **`AlphaMode::Additive` does not add** | the raster fragment path composites it source-over, like `Blend`, with alpha clamped to 1.0. `Framebuffer::add` exists but is used only by the debug tile overlay. Do not build a glow on `Additive` without checking this |
| **`AppConfig::mode` does not set the tone curve** | `App::resolve()` calls `self.config.render.resolve()`, and `AppConfig::with_mode` only writes `config.mode`. A ray-traced app with the default `render` settings resolves with `ToneMap::None` |
| **Section budgets are opt-in** | `DebugSystem::record_section` only records when `DebugConfig::record_sections` is true, and `DebugConfig::disabled()` sets it false. Measurements that "disappeared" are usually this |
| **Half the `FrameSample` fields are unfilled** | `release`, `visibility_ms`, `stream_ms`, `npc_ms` and `physics_ms` exist but nothing in the engine writes them; a game fills them through the budgets named `physics`, `visibility`, `stream` and `npc` |
| **`--all-targets` is the only honest check** | `cargo check --workspace` compiles the libraries and skips every test and example target, so it will happily pass while `cargo test` fails to build. Use `cargo clippy --workspace --all-targets -- -D warnings`, which is what CI runs. |
| **`tools/noxel-gen` writes byte-stable output** | JSON keeps the author's key order, atlas packing sorts by height then name, and PNG encoding is a fixed algorithm. A change that reorders keys or entries produces a huge diff for no reason |
| **A quad's winding decides whether you see it** | The rasterizer culls backfaces and the game camera looks straight down, so a ground quad wound `(x0,z0) -> (x1,z0) -> (x1,z1)` has a normal of `-Y` and the entire world is invisible — with no error anywhere. `world.rs` has a test that asserts every ground triangle faces up |
| **A font offset is measured from the baseline, for every face** | `noxel-ui` puts several faces on one baseline by shifting each glyph by `line_baseline - face_baseline`. A bake that folds the face's own baseline into the glyph offset gets that shift applied twice, and Latin text sinks below the Chinese beside it. `tools/fontgen/README.md` has the arithmetic |
| **`Color8::WHITE` as a tint means "draw the texel as authored"** | Not "multiply by one". The two agree on white coverage glyphs and disagree on coloured art: a nine-slice drawn with a white tint once came out blank while its corners stayed correct, because only the stretched pieces used the tint as the colour |

## Definition of done

Before you report a change as finished:

1. `cargo fmt` and `cargo clippy --all-targets -- -D warnings` are clean.
2. `cargo test` passes, or every failure is one you can explain in one sentence.
3. New public items have doc comments, and new invariants have tests that name
   them.
4. New code obeys the DAG: no dependency added without a reason, no upward
   reference.
5. Any number you put in a report is reproducible from a command you ran, and you
   say which command. A claim like "this is faster" without a before/after
   measurement in the same configuration is not a result.
