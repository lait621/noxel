# ADR 0010 — The testing strategy

**Status:** accepted
**Applies to:** the whole workspace

## Context

This engine is written substantially by AI agents, in parallel, crate by crate.
That workflow has one dominant failure mode: code that compiles, looks plausible,
and is subtly wrong. An agent cannot see the screen, cannot run a playtest, and
cannot develop intuition for "that shadow looks off".

Tests are therefore not a quality nicety here; they are the only feedback loop
that exists.

## Decision

**Every crate carries unit tests inside its own modules, and every non-obvious
invariant has a test that names it.** The bar is set so that a green test suite
is meaningful evidence, not decoration.

## What the tests are for

Four categories, in increasing order of value:

1. **Contract tests.** Public API shape and defaults. Cheap, catches typos.
2. **Invariant tests.** The things that *must* hold: every procedural mesh winds
   outward; a sprite round-trips to the authored bytes; palette colours survive
   the pipeline; the parallel and sequential rasterizer agree; generation is
   order-independent.
3. **Derived-number tests.** Assert on a value the test *computes from the
   geometry*, never a number copied out of a failing run. `nearest_finds_the_closest`
   asserts the distance lies in `8.5..9.0` because the test can show the nearest
   point is 8.7 away — not because 8.7 was the answer the first time.
4. **Golden-image tests.** `noxel-debug::dump::compare` against a committed PNG,
   possible because rendering is deterministic (ADR 0008).

## The rule that matters most

**When a test fails, decide which is wrong: the code or the test.** Both happen.

- A failing test that finds a real bug is the point. During development this
  suite caught: an inverted `Ambient` hemisphere blend, a `damp_factor` that
  returned "never move" for smoothing `0`, a rasterizer with no depth test, a
  font that truncated non-ASCII codepoints to a byte, a bloom pass that
  double-blitted, shared triangle edges shaded twice, and a UV sphere with a
  degenerate pole triangle.
- A failing test whose *expectation* was wrong is also a real finding, and the
  fix is to change the test **and explain why in a comment**. Pretending the
  test was right and hacking the code until it passes is how a suite stops being
  evidence.

## Conventions

- Test names are sentences: `parallel_and_sequential_rendering_agree`, not
  `test_render_2`.
- A test that relies on an ordering asserts the ordering, not the absolute value.
- No test depends on wall-clock time except the ones that explicitly measure it,
  and those assert the ordering invariant, never a duration.
- Every `unsafe`-adjacent assumption is a test: index arithmetic, bitset
  iteration, generational handle reuse, `swap_remove` re-pointing.
- Determinism gets an explicit test in every system that has a random-looking
  input.

## Consequences

- The suite is large — 1475 tests — and runs in a few seconds, because the
  engine has no dependencies and no I/O in the hot path.
- A change that breaks an invariant is caught before it reaches a commit.
- The tests are documentation: `Storage::remove`'s comment about `swap_remove`
  returning the *removed* element exists because a test caught the corruption it
  caused.

## Alternatives rejected

- **Integration tests only**: too coarse to localise a failure, and too slow.
- **Snapshot tests everywhere**: brittle without determinism, and a snapshot
  records a bug as easily as a correct result.
