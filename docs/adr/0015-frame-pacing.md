# ADR 0015 — Frame pacing, and a host that sleeps

**Status:** accepted
**Date:** 2026-10-03
**Supersedes:** nothing
**Related:** [0011](0011-windowing.md), [0008](0008-deterministic-rendering.md), [08-performance](../08-performance.md)

## Context

`noxel-window` asked for the next frame the moment the last one finished:

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    if let Some(window) = &self.window {
        window.request_redraw();
    }
}
```

`request_redraw` does not wait for anything. It is a request, not a present, and
on a platform whose compositor does not block the caller it comes back
immediately — so the loop renders at whatever rate the CPU allows. Measured on
the farming game at 480x270 in a 1440x810 window, that is **100% of a core,
permanently**: with the player standing still, with the window in the background,
with the menu open. The report was "playing makes the laptop hot", and the cause
had nothing to do with how expensive a frame was.

The engine already had every number needed to see it (`Stats::frame_times`,
`FPS` on the overlay) and every number said the frame was fine. The frame rate
was the problem, not the frame.

## Decision

**The host paces itself to a target frame rate, and sleeps in between.**

* `WindowConfig::target_fps: Option<u32>`, default `Some(60)`. `None` means "as
  fast as the machine allows", and it exists because a benchmark is the one
  caller that wants that.
* `FramePacer` — a small, public, window-free type in `noxel-window`'s root —
  owns the arithmetic: `wait_for(now)` says how long to sleep, `mark(now)`
  records that a frame ran.
* `about_to_wait` sets `ControlFlow::WaitUntil(deadline)` while a frame is not
  due, and only requests a redraw when it is.

The deadline is **grid-aligned**: one period after the *previous deadline*, not
one period after the frame finished, and a host that is behind resumes on the
next grid point instead of rendering a burst of frames to catch up. Scheduling
from the end of the frame adds the frame's own cost to every period — a 16.7 ms
target with 4 ms frames becomes 20.7 ms, and the frame rate sags away from the
number that was asked for. The alternative, catching up, is a stutter with extra
steps.

Pacing is `FramePacer`'s job and not the game's. A game that had to remember to
sleep would be a game that pins a core the first time someone writes a new host.

## Consequences

**Idle cost collapses.** The farming game went from 100% of a core to **65%** —
and the remaining 65% is real work (a software rasterizer at 480x270, a resolve
pass, and an upscale), not spin. Removing the redundant resampling from the
presentation blit took it to **57%**; see
[08-performance](../08-performance.md). A demo with a cheap frame drops to a few
percent. The number a player actually notices — fan noise while a menu is open —
is gone.

**Gameplay speed is unchanged.** The host still measures the real elapsed time
and hands it to `Host::step`, and `GameClock` still spreads it into fixed
updates. A paced host is not a host that lies about time; it is a host that
stops *starting* frames early.

**Input latency is unchanged.** Events are still processed the moment they
arrive, between frames; only rendering waits. A key pressed mid-period is
recorded and acts on the next frame, exactly as the input contract describes.

**The frame rate is now a number the engine has an opinion about.** A
run-to-run comparison — `--frames N`, golden frames, a `HeadlessReport` — has
never been affected by this, because the headless paths do not pace: they run as
fast as they can, which is what makes them measurements. Only the windowed host
sleeps, and only ever between frames it has already finished.

**A paced host can miss its period.** A frame that takes longer than its period
will not be "made up", and the frame rate sags below the target rather than
stuttering. That is the correct failure mode for a game, and the opposite of
what a benchmark wants, which is why `None` exists.

### Rejected

* **`ControlFlow::Wait` alone.** It is what the host did, and it is not enough:
  `Wait` waits for *events*, and the host was generating its own event by
  requesting a redraw.
* **Relying on vsync.** `softbuffer` does not guarantee a blocking present, and
  "the compositor throttles it" is a platform behaviour rather than a contract.
  A frame budget that only exists on some machines is not a budget.
* **`thread::sleep` inside the frame.** It blocks the event loop, so input is
  ignored for the duration of the sleep and the window stops responding to a
  resize.
* **Dropping frames to hit the target.** Rendering 120 and showing 60 wastes the
  work instead of removing it.
