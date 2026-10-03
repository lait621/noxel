# ADR 0006 — The world is a pure function of `(seed, address)`

**Status:** accepted
**Applies to:** `noxel-world`, `noxel-npc`, `noxel-core::rng`

## Context

A large streaming world must generate chunk `(900, -400)` identically whether the
player walked there from the west, teleported in, or loaded a save. The obvious
implementation — a single global RNG advanced by every generation call — makes
generation order-dependent, and order-dependent generation breaks:

- **Save games**, because reloading replays a different sequence.
- **Streaming**, because a chunk evicted and regenerated would differ from the
  first version, so the terrain would visibly change as the player walked back.
- **Multi-threading**, because the order chunks are generated in becomes
  non-deterministic.
- **Tests**, because a golden world can only be asserted if generation is stable.

## Decision

**Every piece of generated content is a pure function of the world seed and the
content's address.** There is no shared mutable RNG in the generation path.

```rust
// One stream per (seed, label, address). Nothing else is consulted.
let mut rng = RngStream::for_chunk(config.seed, "terrain", chunk.x, chunk.z);
```

`RngStream` (`noxel-core::rng`) is a PCG32 seeded by hashing
`(seed, label, address...)`. Two consequences fall out for free:

- Chunks can be generated **in parallel** with no locking and no ordering
  requirement.
- A chunk can be generated **without its neighbours** — nothing is stored in a
  permutation table that a neighbour would have filled in.

The noise functions follow the same rule. `noxel-core::math::noise` is
**hash-based, not table-based**: `value_2d`, `perlin_2d`, `simplex_2d`,
`worley_2d`, `fbm_2d`, `ridged_2d` and `warped_fbm_2d` all derive their lattice
values from a hash of the integer coordinates, so any point is samplable in
isolation.

## Consequences

- `WorldStreamer::update` is deterministic regardless of what is already loaded;
  the test suite generates chunks in two different orders and asserts the results
  are identical.
- Same for `noxel-npc`'s spawn and schedule decisions, and for the crowd's
  tier assignment.
- A bug in generation is reproducible from the seed alone, which makes it
  reportable.
- `RngStream::fork(label)` gives a child stream for a sub-feature (a town's
  buildings, a chunk's props) without any chance of correlating with the parent.

## Alternatives rejected

- **One global RNG with a per-chunk seed derived at generation time**: works
  until a caller generates a chunk speculatively (for a preview, or for
  path-finding across an unloaded region) and consumes numbers the real
  generation needs.
- **Table-based Perlin noise**: requires a permutation table, which is fine, but
  it makes "what is the height at this exact point" a function of the table
  rather than of the coordinate, which complicates parallelism for no gain.
