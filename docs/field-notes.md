# Field notes: building a game on Noxel

Everything here was paid for. Each entry is a thing that cost hours, and most
of them cost hours **without producing an error message** — which is what makes
them worth writing down. Read this before you start, not after you are stuck.

The document is in three parts: the traps, the techniques, and the process
lessons that generalise. If you only read one section, read
[Verification](#verification), because that is where the expensive mistakes
live.

---

## Traps

### The invisible world

**Symptom.** The scene renders. The HUD draws. The world is exactly the
background colour. `scene().instance_count()` is correct, the visibility pass
accepted every instance, and nothing anywhere reports a problem.

**Cause.** Quads wound `(x0,z0) → (x1,z0) → (x1,z1)` have a normal of `-Y`. The
rasterizer culls backfaces
(`crates/noxel-render/src/raster/mod.rs`, `backface_culling && !double_sided &&
area < 0.0`), so a straight-down camera sees the back of every tile in the
world. Wound `(x0,z0) → (x0,z1) → (x1,z1)`, the normal points up and everything
appears.

**Why it cost so long.** Culling accepting an instance is not the same as the
rasterizer drawing it. Every check short of counting pixels passes.

**Do this.** Assert on the mesh, not on the scene:

```rust
for index in 0..mesh.triangle_count() {
    assert!(mesh.triangle_normal(index).y > 0.5, "the camera is looking at its back");
}
```

and assert on **drawn pixels** for the end-to-end check:

```rust
let lit = framebuffer.color_slice().chunks_exact(3).filter(|c| c[0] > 0.08).count();
assert!(lit > 10_000, "the farm passed culling but drew {lit} pixels");
```

### A tint of `WHITE` means "as authored", not "multiply by one"

The interface atlas stores coverage in alpha with white RGB. `blit` special-cases
`Color8::WHITE` and draws the texel unchanged. The nine-slice **stretch** helpers
did not, and used the tint as the colour — so a white-tinted frame drew as a
blank white box while its corners, which went through `blit`, stayed correct.
A white box with a coloured border is a very specific look and it means exactly
this bug.

If you add a drawing helper, decide what `WHITE` means and route it through
`color_of` so there is one answer. There is a test for both halves:
`a_nine_slice_stretches_the_source_colour_not_the_tint` and
`a_tinted_nine_slice_replaces_the_colour_everywhere`.

### Per-frame flags must be reset per frame

`UiState::modal` was set by a scrim and lowered by the screen that drew it.
Five screens drew a scrim from before that contract existed and never lowered
it. The first time a player opened the bag, the flag went up and stayed up —
and `interact` refuses to hover *anything* while it is set, so **every button in
the game was dead from that moment**, with a frame that looked completely
normal.

`begin_frame` now clears it, and `Screen::owns_the_frame` is the predicate.
The rule: if a flag describes *this frame*, the frame machinery clears it, not
the code that raised it. Compare `pointer_over_ui`, `hot`, `consumed_click` —
all cleared in `begin_frame` for the same reason.

### Screen-space effects do not survive a camera

Rain drawn as streaks at random screen positions does not move when the camera
does, has no depth, and never lands. It is twenty lines and it is always wrong,
and the description is always "it looks like an animation playing on the
screen". See [`adr/0014-weather.md`](adr/0014-weather.md).

Anything that exists *in* the world — rain, a held tool, a highlight — has to be
simulated in world space and projected at draw time with the renderer's own
arithmetic.

### Anything unoccluded is a choice, and the wrong choice is a bug

The aim highlight **must** be in the interface layer: a world sprite would need
a depth very slightly above the thing it highlights, and getting that wrong
makes the cursor disappear behind a potato.

The held tool **must** be in the world layer: in the interface layer it is over
every tree, every building and the tile being aimed at, and the report is
"the tool blocks the view".

There is no general answer. There is only "what should be able to hide this?",
asked deliberately, once per thing.

### A `match` with a `_` arm that guesses

```rust
let atlas = match which {
    "terrain" => ...,
    "crops" => ...,
    "characters" => ...,
    _ => assets.atlases.props.as_ref(),   // <- "ui" fell in here
};
```

Forgetting a case does not fail. It samples the wrong picture at the right
coordinates, and the held hoe comes out looking like a crate. When a fallback
is *plausible* rather than obviously wrong, it will be hit and it will be
silent.

### Build scripts drift away from the tree

A packaging script still pointed at `games/noxel-valley/assets` from when it
lived one directory up. That path did not exist, so an "if the art is missing,
generate it" branch **created** it — containing the sprites and no fonts — and
every bundle was assembled from that stray tree. The game ran, responded to
input, and had no words on it anywhere.

The fix is not the path. The fix is that the script now **asserts** that every
required file exists, and verifies afterwards that the built binary can find
its font. A warning would not have been enough: this failure looks like a broken
game, not a missing file.

### Two copies of one crate

A game depending on the engine by `git` revision while also depending on a
new engine crate by `path` gets **two** `noxel-core` crates, and therefore two
distinct `Vec3` types that are structurally identical and do not unify. The
error is `expected Vec3, found Vec3`, which is confusing for about ten minutes.

While the engine and the game change together, point **every** dependency at
one or the other. Never mix.

### The interface can be green and the game unplayable

Every UI test started *after* the input conversion and *after* the frame loop.
So a bug that made every button in the game unpressable sat behind a passing
suite, and the report was "the buttons don't work" for three rounds.

The tests that matter drive the real path:

```rust
valley.borrow_mut().input = ui_input(&window_input((x, y)));
app.step(1.0 / 60.0);
```

and assert on an outcome a player would recognise — the screen changed, the gold
went down. There is one for the start menu and one for the shop's buy button,
found by **sweeping the whole screen** rather than by trusting a layout constant.

### Assets are not in the repository until you check

`--check` exists because two runs of a deterministic generator can still differ
from what is committed. When the art generator was first moved into the game,
the committed PNGs were stale relative to the final code — the JSON matched (so
every region name and size was right) and three PNGs did not. Names and sizes
matching is not the same as pixels matching.

---

## Techniques

### The projection, and only the projection

One world unit is exactly one tile is exactly `TILE` pixels, and the camera's
focus is the centre of the frame:

```rust
let centre_x = (world.x - focus.x) * TILE + width  / 2;
let centre_y = (world.z - focus.z) * TILE + height / 2;
```

Write it **once**, as a function, and use it for everything that has to land on
a tile: the aim highlight, weather, a held item, a spawn marker. Every caller
that derives its own copy will drift, and the drift is invisible in the middle
of the screen and obvious at the edges.

**Project through the camera's `snapped_focus()`, never the player's position.**
The camera smooths and snaps to whole sixteenths, so the two differ by up to
half a tile while walking — which is exactly the offset between a highlight and
the tile it is supposed to point at. That was a real bug report.

`aim_rect` in the game is this function, and it has tests: a tile centred on the
focus lands centred on the frame; moving one tile moves the outline exactly one
tile; a tile below the focus is drawn below the focus (get this sign wrong and
the highlight mirrors about the middle of the screen, which still looks
plausible in the centre); and the result is always whole pixels.

### Depth without a depth sort

Everything is a flat quad on the ground plane. With a straight-down camera,
world `z` *is* screen depth — so give each quad a tiny `y` offset proportional
to its `z` and let the depth buffer do the painter's algorithm:

```rust
const DEPTH_STEP: f32 = 0.001;
fn depth_of(z: f32) -> f32 { DEPTH_STEP * (z + 1.0) }
```

A held item two thousandths in front of the player reads as held rather than as
standing next to them. A tree south of the player has a larger `z` and hides
whatever is behind it.

### Integer-only layout: measure, do not guess

Every soft edge in an interface comes from a layout constant that was right for
a different font, a different size, or a different language.

* Line pitch is `font.measure("操作", &style).1`, not `11`.
* A heading's height is measured, because a scale-2 Chinese title is 24 pixels
  and a scale-2 Latin one is 18 — reserving 14 for both draws the title through
  the first row.
* Panel widths follow their contents. Three panels had hard-coded widths that
  overflowed the moment the font grew 17%, and the fix was to stop hard-coding.
* Rounded, never truncated: `.round()` on a screen coordinate. A fractional one
  is a highlight that shimmers by a pixel as the camera moves.

Related, and painful: `FontSet::measure` and `FontSet::draw` must agree about
unknown glyphs, or a string measures one width and draws another.

### One frame, one run of text

`FontSet::draw_text` aligns a run to the baseline of the faces **that run**
used. A Latin-only run and a Chinese run drawn at the same `y` do not share a
baseline: the Chinese one, being taller, sits lower. Put mixed content on
separate rows, or accept that it will look like two sizes of the same line.

(The underlying rule is that a glyph's offset is measured from its **own face's**
baseline, and the renderer shifts by `line_baseline - face_baseline`. Counting
the baseline twice put Latin six pixels below the Chinese beside it.)

### Data, not callbacks

The interface has no business reading the farm, and the farm has no business
knowing how a hint is drawn. The game computes a sentence and hands it over:

```rust
self.game_ui.set_hint(screens::hint(&self.state, &self.map, &self.player));
```

This kept a whole feature — a contextual tutorial — from threading a `&FarmMap`
through nine draw calls, and it means the hint is testable without a frame.
When you are tempted to pass a new borrow into `Ui::draw`, pass a value
instead.

### Deterministic scatter without state

Particles and decoration usually want a random layout and no bookkeeping. Two
co-prime strides off a counter give you one:

```rust
let seed = index as f32 * 37.7;
let x = (seed * 7.3).fract() * width;
let y = (seed * 3.1 + phase * speed).fract() * height;
```

No state, no allocation, and the same frame draws the same picture twice — which
is what lets a recording be replayed and a test assert on a buffer.

### Verification

This is the section worth reading twice.

**Test the path the player takes, not the path that is convenient.** A test that
starts after the input conversion cannot find a bug in the input conversion.
A test that starts after the frame loop cannot find a bug in the frame loop.

**Assert on what a person would see.** "The mesh has vertices" passes while the
world is invisible. "The instance was accepted by culling" passes while every
triangle is rejected. "The button reports a click" passes while the click
handler is never reached. Count pixels; compare gold; check which screen is
open.

**Every fix gets a test that fails without it.** Not "the suite is still green",
but: revert the fix, watch the new test go red, put it back. If you cannot make
it fail, you have not understood the bug and the test is decoration.

**A fix that reduces a symptom is not a fix.** The held tool was reported as
blocking the view; it was made smaller and left in the same layer, and reported
again. The second report was correct and the first "fix" was a way of not
understanding the first report.

**When someone says "X does not work", find what they see, not what the code
does.** "I bought seeds and cannot farm" was not a bug in buying — it was that
selecting a seed puts `Tool::Hand` in hand, the field starts as bare dirt, and
nothing anywhere said the hoe was the missing step. The code was correct. The
game was unplayable.

**Read the counts.** `cargo test --workspace` reporting `1704 passed, 0 failed`
is not evidence that the game works; it is evidence that the code does what the
tests say. Those are different claims, and the gap between them is where every
bug in this document lived.

### Assets

* **Generate them, do not draw them.** `noxel-gen` and the game's
  `valley-artgen` are deterministic: a second run changes no bytes, so a diff in
  `assets/` means something real changed.
* **`--check` is not optional.** It regenerates into scratch and byte-compares.
  Run it before committing art.
* **Never hand-edit a generated file.** Fix the generator.
* **One-bit fonts need supersampling.** Rasterise at 4× with antialiasing,
  box-downsample with exact area averaging, *then* threshold. The middle step is
  the fix: a hinted rasteriser at 12 px snaps a stem onto a half pixel and draws
  it at 40% coverage, which dies at any threshold that keeps the rest of the
  glyph whole. Averaging first measures real coverage, and that stem arrives at
  60% and survives.
* **Raising the threshold *erodes* glyphs after averaging**, because coverage is
  spread across pixel boundaries. This is the opposite of what it does on a
  direct render, and it inverted the search.
* **Check the bake by looking at it.** Render the hard characters, upscale by a
  whole number with nearest sampling, and look. Numbers will not tell you that
  日 and 曰 came out identical.

### Features and dependencies

* Everything optional is a **feature**, off by default, so `cargo build`
  downloads nothing. `noxel-window` boxes `winit` and `softbuffer`;
  `noxel-audio` boxes `cpal`.
* **A feature that implies another must say so.** A game's `window` feature
  pulled in `noxel-audio` without its `output` feature, so every
  `#[cfg(feature = "audio")]` block was compiled out — including the one that
  started the stream and the one that told the music director what the weather
  was doing. Nothing failed. The code was not there.
* **`cargo build` succeeding is not evidence a code path exists.** `strings
  target/release/game | grep -c 'noxel-audio:'` is.

---

## The shape of the work

If you are asked to build something playable on this engine, the order that
worked was:

1. **Get one thing on screen.** A ground plane, a camera, a frame dump. Prove
   the world draws before anything else exists.
2. **Build the projection helper and test it.** Everything depends on it and it
   is cheap to get right.
3. **Make a rule testable before making it pretty.** `action::use_tool` was
   moved out of the binary into the library specifically so the farming loop
   could be tested without a window, and the test found two real balance bugs
   immediately.
4. **Then the interface.** It is the easiest part to iterate on and the hardest
   part to get right, and it is where every complaint will land.
5. **Play it, in the real build, with a mouse.** There is no substitute. Every
   bug in the first section of this document was found by somebody using the
   thing, not by a test.

---

## Where to look next

| | |
|---|---|
| [`contributing-for-ai.md`](contributing-for-ai.md) | the rules and invariants of the codebase |
| [`00-overview.md`](00-overview.md) | what the engine is and is not |
| [`adr/`](adr/) | why each decision was made, including the rejected alternatives |
| [`../games/noxel-valley`](https://github.com/lait621/Noxel-valley) | a worked example: a real game on this engine |
| [`guides/`](guides/) | task-shaped how-tos |
