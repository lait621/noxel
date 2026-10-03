# 0011. Windowing is a crate, and it is the one exception

Status: accepted (2026-10)

## Context

The engine had no way to open a window. It rendered into a `Framebuffer` and
wrote PNGs, which is a good CI story and a poor "let me try it" story. Players
type keys, and keys arrive through a platform API.

That collides head-on with [ADR 0002](0002-no-dependencies.md). A window cannot
be opened with `std` alone: every desktop OS exposes windowing through its own
toolkit, reached by FFI. The alternatives were:

1. **Write the FFI by hand.** On macOS that is `objc_msgSend` against
   `NSApplication`, `NSWindow`, `NSView` and a run loop, plus a different
   implementation per platform. It needs `unsafe`, which every crate in the
   workspace forbids, and it is the kind of code that works on the machine it was
   written on and nowhere else.
2. **Add a windowing crate.** Correct, portable, maintained — and a dependency.
3. **Ship no window.** Defensible for the engine, useless for the user.

## Decision

**Option 2, in a new crate `noxel-window`, with the dependency optional and off
by default.**

The crate is boxed in on four sides:

1. **One crate.** It is the only member of the workspace with a third-party
   dependency. Nothing else in the tree may gain one without another ADR.
2. **Optional and off by default.** `winit` and `softbuffer` sit behind a
   `window` feature. `cargo build --workspace` and `cargo test --workspace`
   compile neither, download neither, and still work with no network at all —
   this is asserted, not assumed. The engine's default build keeps every property
   ADR 0002 bought: no downloads, no supply chain, no build scripts from
   strangers.
3. **Inverted dependency.** `noxel-window` depends on the engine; the engine does
   not depend on it. The seam is a trait, `Host`, which the crate defines and the
   caller implements. `noxel-app`, `noxel-render` and everything below them have
   never heard of `winit`.
4. **It fails loudly when it is off.** Without the feature, `run` returns
   `WindowError::FeatureDisabled`, whose `Display` prints the exact command to
   rebuild with. A caller who forgot the feature gets an instruction, not a
   missing symbol at link time.

## What the crate does, and what it deliberately does not

It does:

- Open a window sized to a whole-number multiple of the internal resolution, and
  letterbox the remainder. Pixel art that is scaled by 2.5 shimmers; this is the
  same rule as `Viewport::pixel_art`, enforced where the pixels actually land.
- Convert keyboard and mouse events into `Input`, using **the engine's own key
  codes**. `key::W == 87`, which is `b'W'`, so
  `movement_axis(87, 83, 65, 68)` means WASD in the window and in the engine with
  no translation table in between.
- Own the real-time loop: measure the elapsed wall clock, clamp it, and hand it
  to `Host::step`, which forwards it to the engine's fixed-step `GameClock`. The
  simulation rate stays independent of the frame rate, which is the property
  [ADR 0008](0008-deterministic-rendering.md) depends on.
- Release every held key when the window loses focus. Without this the character
  walks into a wall while the player is reading their mail.
- Quit on Escape and on the window's close button, because no game should have to
  implement those.

It does not:

- **Draw with the GPU.** It presents the framebuffer with `softbuffer`, a CPU
  blit. That is consistent with [ADR 0003](0003-software-renderer-first.md): the
  renderer is the reference implementation, and a window is a display, not a
  second renderer. A GPU path is [ADR 0007](0007-gpu-backend.md)'s business and
  is still unbuilt.
- **Implement the game.** There is no scene graph, no menu, no asset loading —
  those live above it. `Host` is four methods.
- **Hide the event loop.** `run` blocks until the window closes, because that is
  what a `winit` event loop does and pretending otherwise leaks the platform
  detail back up.

## Consequences

- A user with a checkout and a network connection can play the demo in one
  command: `./scripts/run-window.sh`.
- A user who only wants to work on the engine never downloads anything. The
  default gate — `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `noxel-gen verify` — is unchanged and still
  offline.
- Enabling the feature pulls in **33 crates on macOS** — `winit`, `softbuffer`
  and the objc2 bindings they need. `Cargo.lock` resolves about 190 packages
  that a default build never compiles and never downloads. That is the visible
  cost, and it is worth naming: the lockfile is no longer a complete description
  of what the binary contains. `cargo tree -e normal` with default features is,
  and it is the check that keeps this honest:

  ```text
  $ cargo tree --workspace -e normal | grep -v '^noxel-\|town-demo'
  (nothing)
  ```
- The demo is now two things at once. With nothing pressed it walks its scripted
  route and writes PNGs — the CI path. The moment a movement key goes down, the
  player takes over and it is a game. There is no mode flag; the fallback *is*
  the feature.
- The window cannot be tested headlessly. What is tested is everything around it:
  the key mapping, the blit's scaling and letterboxing arithmetic, the input
  edge handling, the fixed-step timing, and the disabled-feature error. The event
  loop itself is exercised by running the binary, which is what
  `scripts/run-window.sh` does.

## Alternatives rejected

- **A `wgpu` surface instead of `softbuffer`.** More dependencies, a graphics
  driver requirement, and a second rendering path that would immediately diverge
  from the reference rasterizer.
- **Putting the window in `noxel-app`.** It would give every engine user a
  platform dependency whether or not they want one, which is the exact trade
  ADR 0002 refuses.
- **Making it a separate repository.** Then the demo could not demonstrate the
  engine's real API, and the seam would rot.
- **`unsafe` FFI in a platform crate of our own.** A hundred lines per platform
  to reach feature parity with a crate that already exists, tested on hardware we
  do not have.
