# ADR 0013 — Audio, and a second exception to the dependency rule

**Status:** accepted
**Date:** 2026-10-03
**Supersedes:** nothing
**Related:** [0002](0002-no-dependencies.md), [0006](0006-deterministic-generation.md), [0012](0012-ui-layer.md)

## Context

The engine plays no sound. ADR 0002 makes that easy to state and hard to
revisit: the crate graph has no third-party dependencies at all, with exactly
one deliberate exception — `noxel-window` boxes `winit` and `softbuffer` behind
an off-by-default feature, because a window is a platform facility that cannot
be written from scratch in reasonable time.

Audio is the same kind of thing. A game with no music is a game that feels
unfinished in a way players notice immediately and cannot always name.

## Decision

Add `noxel-audio`, structured exactly like `noxel-window`:

* The **synthesiser** (`synth`) and the **composer** (`music`) depend on nothing
  but `core` and `noxel-core`. They contain all the behaviour: oscillators,
  envelopes, filters, the chord progression, the note chooser, the mix.
* Only `output` — the device stream — needs a platform crate (`cpal`), and only
  when the `output` feature is on.

The default `cargo build` still downloads nothing. `cargo test -p noxel-audio`
runs 30 tests with no device, no feature, and no sound card.

## Consequences

**The music is generated, not shipped.** There is no audio file in the
repository and no decoder in the crate. A `Mood` is a dozen `const` numbers —
key, scale, tempo, density, four levels — and the composer turns it into
samples.

That is a bigger decision than it looks, and it is the one that makes the
feature affordable:

| | Recorded tracks | Generated |
|---|---|---|
| Varies with weather and season | one asset per combination, or crossfades | one field |
| Reviewable in a diff | no | yes |
| Deterministic for a given seed | only by shipping the file | yes |
| Fits ADR 0006 | by accident | by construction |

The cost is that the music is *simple*. A real composer with a resonant filter,
a sampled instrument and a phrase memory would be several times the size. For a
gentle bed under a farming game, four moods of marimba and pad is the right
trade, and the module says so in its own documentation rather than pretending
otherwise.

**The audio callback never allocates and never locks.** The composer moves into
the callback; only atomics cross the boundary — a volume, a mute, and a mood
index. A `Mutex<Composer>` shared with the game thread would drop out whenever a
frame held it for a millisecond, which is the most common way a game's audio
stutters.

**A machine with no device still plays the game.** `output::start` returns
`Result`, the game logs the failure and carries on in silence.

## Alternatives rejected

**Ship recorded music.** Rejected on ADR 0006 grounds rather than size: a
soundtrack that must be re-authored per weather combination is a soundtrack that
will not vary per weather combination. The first rainy day would sound like the
first sunny one, and the feature the request asked for would quietly not exist.

**Write the output layer for each platform by hand.** The engine forbids
`unsafe` (`#![forbid(unsafe_code)]`), and every platform audio API is an unsafe
FFI. This is the same wall ADR 0002 hit with windowing, and it is answered the
same way.

**Make audio a dependency of `noxel-app`.** Rejected: most games built on this
engine will not need it, and a crate that has to be opted out of is not opt-in.
