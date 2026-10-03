# ADR 0002 — Zero third-party dependencies

**Status:** accepted
**Applies to:** the whole workspace

## Context

A game engine is the piece of software most likely to outlive its dependencies.
The Rust ecosystem's crates are excellent, but each one brings a version range, a
compile-time cost, a licence, an audit surface and a chance of an unmaintained
transitive dep. An engine that vendors thirty crates is an engine whose build
breaks when any one of them releases a breaking change.

There is also a workflow argument. This project is written substantially by AI
agents, and an agent that can read every line it depends on can reason about
behaviour end to end. An agent facing an unfamiliar crate's undocumented edge
cases cannot.

## Decision

**No third-party runtime dependencies.** The workspace depends only on `std`.
Even the PNG codec is written from scratch: `noxel-asset::png` contains a
complete DEFLATE inflater (stored, fixed-Huffman and dynamic-Huffman blocks) and
a fixed-Huffman compressor, plus CRC-32 and Adler-32.

Every crate declares:

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]
```

## What this buys

- `cargo build` works on a fresh machine with no network.
- The whole workspace builds in seconds, so an AI agent can iterate quickly.
- No `unsafe` anywhere: every slice index is checked, every `as` cast is
  considered, and the borrow checker is the only UB defence needed.
- The PNG codec is verified against an independent implementation: the test suite
  cross-checks round-trips, and development verified that our compressor's output
  decompresses byte-exactly with CPython's `zlib`.

## What it costs

Real work, done once, in-tree:

| Normally a crate | Here |
|---|---|
| `png` / `image` | `noxel-asset::png`, `noxel-asset::image` (1782 + 840 lines) |
| `serde_json` | `noxel-asset::json`, an insertion-ordered parser with line/column errors |
| `glam` | `noxel-core::math` |
| `rand` / `fastrand` | `noxel-core::rng` (PCG32 + addressable streams) |
| `rayon` | `noxel-core::jobs`, a scoped job pool over `std::thread` |
| `winit` / `SDL2` | **not replaced** — see below |
| `noise` | `noxel-core::math::noise`, hash-based so any coordinate is samplable |

## The exception: windowing

A window needs a platform API, and reimplementing `winit` is out of scope.
Noxel therefore has **no windowing layer**. An `App` produces a
`Framebuffer` and consumes an `InputState`; presenting the buffer is the host's
job. `docs/guides/windowing.md` shows how to attach `winit` or `SDL2` behind an
optional feature flag, and the `town-demo` example writes PNG frames so the
engine is fully usable — and fully testable — with no display server at all.

## Consequences

- A GPU backend (see ADR 0007) is the one place where a dependency would be
  genuinely hard to avoid; it is specified as **feature-gated**, so the default
  build stays dependency-free.
- Development is slower in the short term: writing a DEFLATE encoder is not free.
- The `#![forbid(unsafe_code)]` bar has a real cost too — `PaletteFile::to_palette`
  has to `Box::leak` entry-name strings because `Palette::push` wants
  `&'static str` and there is no way to build one from owned data without unsafe.
  The leak is bounded by the number of palette entries and is documented in
  place.

## Alternatives rejected

- **Use the ecosystem, vendor with `cargo vendor`**: solves reproducibility, not
  auditability, and the build still depends on the crates compiling.
- **Allow a small allowlist (`glam`, `png`)**: the allowlist grows; there is no
  natural stopping point.
