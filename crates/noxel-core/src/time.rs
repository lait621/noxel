//! Frame timing, fixed-step scheduling and profiling.
//!
//! Noxel runs a **fixed simulation step** with an interpolated render, the
//! arrangement that keeps physics, NPC steading and pathfinding reproducible
//! while still rendering smoothly at whatever rate the display allows.
//!
//! ```no_run
//! use noxel_core::time::GameClock;
//!
//! let mut clock = GameClock::new(60.0); // 60 Hz simulation
//! // Each frame:
//! // clock.begin_frame(real_delta_seconds);
//! // while clock.step() { simulate(clock.fixed_dt()); }
//! // render(clock.alpha()); // blend between the last two states
//! ```

use std::collections::VecDeque;
use std::time::Instant;

/// The canonical simulation rate for the engine and the demo (60 Hz).
pub const DEFAULT_FIXED_HZ: f32 = 60.0;

/// Largest real-time delta accepted in one frame, in seconds.
///
/// Anything larger is treated as a hitch (or a debugger pause) and clamped, so a
/// breakpoint cannot teleport the player through a wall.
pub const DEFAULT_MAX_FRAME_DELTA: f32 = 0.25;

/// A fixed-step simulation clock with an accumulator.
#[derive(Clone, Debug)]
pub struct GameClock {
    fixed_dt: f32,
    accumulator: f32,
    elapsed: f64,
    tick: u64,
    time_scale: f32,
    paused: bool,
    max_frame_delta: f32,
    max_substeps: u32,
    steps_this_frame: u32,
    dropped_time: f64,
    alpha: f32,
}

impl GameClock {
    /// Creates a clock running at `hz` simulation steps per second.
    #[must_use]
    pub fn new(hz: f32) -> Self {
        Self::with_fixed_dt(1.0 / hz.max(1.0))
    }

    /// Creates a clock with an explicit fixed step.
    #[must_use]
    pub fn with_fixed_dt(fixed_dt: f32) -> Self {
        Self {
            fixed_dt: fixed_dt.max(1e-5),
            accumulator: 0.0,
            elapsed: 0.0,
            tick: 0,
            time_scale: 1.0,
            paused: false,
            max_frame_delta: DEFAULT_MAX_FRAME_DELTA,
            max_substeps: 16,
            steps_this_frame: 0,
            dropped_time: 0.0,
            alpha: 0.0,
        }
    }

    /// The fixed simulation step in seconds.
    #[inline]
    #[must_use]
    pub fn fixed_dt(&self) -> f32 {
        self.fixed_dt
    }

    /// The fixed rate in Hz.
    #[inline]
    #[must_use]
    pub fn fixed_hz(&self) -> f32 {
        1.0 / self.fixed_dt
    }

    /// The interpolation factor in `[0, 1)` between the previous and current
    /// simulation states. Renderers lerp transforms by this.
    #[inline]
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Simulation time in seconds (scaled, excludes dropped time).
    #[inline]
    #[must_use]
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Number of completed fixed steps since construction.
    #[inline]
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Time scale: `0.0` freezes, `1.0` is real time, `2.0` is double speed.
    #[inline]
    #[must_use]
    pub fn time_scale(&self) -> f32 {
        self.time_scale
    }

    /// Sets the time scale.
    #[inline]
    pub fn set_time_scale(&mut self, scale: f32) {
        self.time_scale = scale.max(0.0);
    }

    /// True when the clock is paused.
    #[inline]
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pauses or resumes the clock without losing accumulated time.
    #[inline]
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Accumulated real time that was discarded because the substep budget ran
    /// out. A healthy engine keeps this at zero; a non-zero value means the
    /// simulation cannot keep up and is a signal to spawn fewer NPCs.
    #[inline]
    #[must_use]
    pub fn dropped_time(&self) -> f64 {
        self.dropped_time
    }

    /// Sets the maximum number of fixed steps executed per frame.
    #[inline]
    pub fn set_max_substeps(&mut self, n: u32) {
        self.max_substeps = n.max(1);
    }

    /// Sets the largest accepted real-time delta.
    #[inline]
    pub fn set_max_frame_delta(&mut self, dt: f32) {
        self.max_frame_delta = dt.max(self.fixed_dt);
    }

    /// Feeds one frame's real-time delta into the accumulator.
    ///
    /// Call once per frame, before the `while clock.step()` loop.
    pub fn begin_frame(&mut self, real_delta: f32) {
        let dt = if real_delta.is_finite() {
            real_delta.clamp(0.0, self.max_frame_delta)
        } else {
            0.0
        };
        if !self.paused {
            self.accumulator += dt * self.time_scale;
        }
        self.steps_this_frame = 0;
        self.update_alpha();
    }

    /// Advances one fixed step, returning `true` while a step is due.
    ///
    /// ```no_run
    /// # use noxel_core::time::GameClock;
    /// # let mut clock = GameClock::new(60.0);
    /// # clock.begin_frame(0.016);
    /// let mut steps = 0;
    /// while clock.step() {
    ///     steps += 1;
    ///     assert!(steps <= 8, "the substep cap must bound this loop");
    /// }
    /// ```
    pub fn step(&mut self) -> bool {
        if self.accumulator < self.fixed_dt {
            self.update_alpha();
            return false;
        }
        // Guard against the spiral of death: never run more than `max_substeps`
        // in one frame, and account for the time we refuse to simulate so the
        // engine can report that it is overloaded instead of silently lagging.
        if self.steps_this_frame >= self.max_substeps {
            self.dropped_time += (self.accumulator - self.fixed_dt) as f64;
            self.accumulator = self.fixed_dt;
            self.update_alpha();
            return false;
        }
        self.accumulator -= self.fixed_dt;
        self.elapsed += self.fixed_dt as f64;
        self.tick += 1;
        self.steps_this_frame += 1;
        self.update_alpha();
        true
    }

    /// The number of fixed steps already executed for the current frame.
    #[inline]
    #[must_use]
    pub fn substeps_this_frame(&self) -> u32 {
        self.steps_this_frame
    }

    fn update_alpha(&mut self) {
        self.alpha = if self.fixed_dt > 0.0 {
            (self.accumulator / self.fixed_dt).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Resets the clock to its initial state, keeping the configuration.
    pub fn reset(&mut self) {
        self.accumulator = 0.0;
        self.elapsed = 0.0;
        self.tick = 0;
        self.dropped_time = 0.0;
        self.alpha = 0.0;
        self.steps_this_frame = 0;
    }
}

/// A rolling window of frame times.
///
/// Deliberately allocation-free after construction: the ring buffer is fixed
/// size so this can be called from a render loop without touching the allocator.
#[derive(Clone, Debug)]
pub struct FrameTiming {
    samples: VecDeque<f32>,
    capacity: usize,
    frame_index: u64,
    last_delta: f32,
    total: f64,
    worst: f32,
}

impl FrameTiming {
    /// Creates a timing window holding `capacity` samples.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(2);
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            frame_index: 0,
            last_delta: 0.0,
            total: 0.0,
            worst: 0.0,
        }
    }

    /// Records one frame.
    pub fn push(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(delta_seconds);
        self.frame_index += 1;
        self.last_delta = delta_seconds;
        self.total += delta_seconds as f64;
        self.worst = self.worst.max(delta_seconds);
    }

    /// The most recent frame time.
    #[inline]
    #[must_use]
    pub fn last(&self) -> f32 {
        self.last_delta
    }

    /// Number of frames recorded.
    #[inline]
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Total wall-clock time observed.
    #[inline]
    #[must_use]
    pub fn total_time(&self) -> f64 {
        self.total
    }

    /// Shortest frame in the window.
    #[must_use]
    pub fn min(&self) -> f32 {
        self.samples.iter().copied().fold(f32::INFINITY, f32::min)
    }

    /// Longest frame in the window.
    #[must_use]
    pub fn max(&self) -> f32 {
        self.samples.iter().copied().fold(0.0, f32::max)
    }

    /// Mean frame time over the window.
    #[must_use]
    pub fn mean(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f32>() / self.samples.len() as f32
    }

    /// Smoothed instantaneous frames-per-second.
    ///
    /// Uses the median rather than the mean so a single 200 ms hitch does not
    /// make the counter flicker.
    #[must_use]
    pub fn fps(&self) -> f32 {
        let med = self.median();
        if med > 0.0 { 1.0 / med } else { 0.0 }
    }

    /// Median frame time in the window.
    #[must_use]
    pub fn median(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut v: Vec<f32> = self.samples.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        v[v.len() / 2]
    }

    /// The `q`th percentile frame time (`q` in `[0, 1]`).
    #[must_use]
    pub fn percentile(&self, q: f32) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut v: Vec<f32> = self.samples.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let idx = ((v.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
        v[idx]
    }

    /// The worst frame since construction (not windowed).
    #[inline]
    #[must_use]
    pub fn worst(&self) -> f32 {
        self.worst
    }

    /// A one-line human-readable summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "frame {} | {:.1} fps | last {:.2} ms | mean {:.2} ms | p99 {:.2} ms | max {:.2} ms",
            self.frame_index,
            self.fps(),
            self.last_delta * 1000.0,
            self.mean() * 1000.0,
            self.percentile(0.99) * 1000.0,
            self.max() * 1000.0
        )
    }

    /// Clears the window.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.frame_index = 0;
        self.total = 0.0;
        self.worst = 0.0;
        self.last_delta = 0.0;
    }
}

impl Default for FrameTiming {
    fn default() -> Self {
        Self::with_capacity(120)
    }
}

/// A simple wall-clock stopwatch.
#[derive(Clone, Debug)]
pub struct Stopwatch {
    start: Instant,
}

impl Stopwatch {
    /// Starts a stopwatch now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Seconds since the stopwatch started.
    #[must_use]
    pub fn elapsed_seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Milliseconds since the stopwatch started.
    #[must_use]
    pub fn elapsed_ms(&self) -> f64 {
        self.elapsed_seconds() * 1000.0
    }

    /// Restarts and returns the elapsed time of the previous interval.
    pub fn lap_ms(&mut self) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.start).as_secs_f64() * 1000.0;
        self.start = now;
        dt
    }
}

/// One timed region within a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeSample {
    /// The scope's name, as passed to [`Profiler::scope_begin`].
    pub name: &'static str,
    /// Accumulated time in this frame, in milliseconds.
    pub ms: f32,
    /// Number of times the scope was entered this frame.
    pub calls: u32,
    /// Nesting depth; 0 is a top-level scope.
    pub depth: u16,
    /// Parent scope index within the frame, or `u32::MAX` for a root scope.
    pub parent: u32,
}

impl ScopeSample {
    /// Mean cost per call in milliseconds.
    #[must_use]
    pub fn mean_ms(&self) -> f32 {
        if self.calls == 0 {
            0.0
        } else {
            self.ms / self.calls as f32
        }
    }
}

/// A completed frame's profile.
#[derive(Clone, Debug, Default)]
pub struct FrameProfile {
    /// Frame index.
    pub frame: u64,
    /// Total wall time attributed to *root* scopes, in milliseconds.
    pub total_ms: f32,
    /// Every scope sampled in the frame, in the order first entered.
    pub scopes: Vec<ScopeSample>,
}

impl FrameProfile {
    /// Looks up a scope by name.
    #[must_use]
    pub fn scope(&self, name: &str) -> Option<&ScopeSample> {
        self.scopes.iter().find(|s| s.name == name)
    }

    /// Runs the supplied function for every top-level scope, deepest-first
    /// indentation applied automatically.
    pub fn walk(&self, mut f: impl FnMut(&ScopeSample, u16)) {
        for s in &self.scopes {
            f(s, s.depth);
        }
    }

    /// Renders the profile as an indented table, ready to print.
    #[must_use]
    pub fn to_table(&self) -> String {
        let mut out = format!("frame {} — {:.2} ms total\n", self.frame, self.total_ms);
        for s in &self.scopes {
            let indent = "  ".repeat(s.depth as usize);
            out.push_str(&format!(
                "{indent}{:<24} {:>7.2} ms  x{:<5} {:>6.3} ms/call\n",
                s.name,
                s.ms,
                s.calls,
                s.mean_ms()
            ));
        }
        out
    }
}

/// A hierarchical CPU profiler.
///
/// Deliberately tiny: no sampling thread, no allocation per sample beyond the
/// first time a name is seen, and it can be compiled out by checking
/// [`Profiler::is_enabled`] at the call site.
///
/// ```
/// use noxel_core::time::Profiler;
///
/// let mut p = Profiler::new();
/// p.begin_frame(0.0);
/// let t = p.scope_begin("physics");
/// p.scope_end(t);
/// p.end_frame();
/// assert!(p.last_frame().scope("physics").is_some());
/// ```
#[derive(Debug)]
pub struct Profiler {
    enabled: bool,
    frame: u64,
    current: Vec<ScopeSample>,
    open: Vec<(u32, Instant)>,
    last: FrameProfile,
    /// Frame budget in milliseconds; scopes exceeding it are flagged.
    budget_ms: f32,
}

impl Profiler {
    /// Creates an enabled profiler with a 16.67 ms (60 fps) budget.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: true,
            frame: 0,
            current: Vec::with_capacity(32),
            open: Vec::with_capacity(16),
            last: FrameProfile::default(),
            budget_ms: 1000.0 / 60.0,
        }
    }

    /// Creates a profiler that records nothing. Use this in shipping builds to
    /// remove all overhead.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new()
        }
    }

    /// True when the profiler is recording.
    #[inline]
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enables or disables recording.
    #[inline]
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Sets the per-frame budget used by [`Profiler::over_budget`].
    #[inline]
    pub fn set_budget_ms(&mut self, ms: f32) {
        self.budget_ms = ms.max(0.0);
    }

    /// Begins a frame. `_real_delta` is accepted for call-site symmetry with
    /// [`GameClock::begin_frame`] and is not used for accounting.
    pub fn begin_frame(&mut self, _real_delta: f32) {
        if !self.enabled {
            return;
        }
        self.current.clear();
        self.open.clear();
    }

    /// Finishes a frame and rotates the sample buffers.
    pub fn end_frame(&mut self) {
        if !self.enabled {
            return;
        }
        // Any scope left open (an early return / `?`) is closed at the frame
        // boundary rather than leaking into the next frame.
        while let Some((idx, start)) = self.open.pop() {
            let dt = start.elapsed().as_secs_f32() * 1000.0;
            if let Some(s) = self.current.get_mut(idx as usize) {
                s.ms += dt;
                s.calls += 1;
            }
        }
        let total_ms = self
            .current
            .iter()
            .filter(|s| s.depth == 0)
            .map(|s| s.ms)
            .sum();
        self.last = FrameProfile {
            frame: self.frame,
            total_ms,
            scopes: self.current.clone(),
        };
        self.frame += 1;
    }

    /// Opens a named scope. Pair with [`Profiler::scope_end`].
    #[inline]
    pub fn scope_begin(&mut self, name: &'static str) -> u32 {
        if !self.enabled {
            return u32::MAX;
        }
        let depth = self.open.len() as u16;
        let parent = self.open.last().map_or(u32::MAX, |(i, _)| *i);
        // Reuse an existing sample with the same name+depth so repeated calls in
        // one frame accumulate instead of allocating a row per call.
        let idx = match self
            .current
            .iter()
            .position(|s| s.name == name && s.depth == depth)
        {
            Some(i) => i as u32,
            None => {
                self.current.push(ScopeSample {
                    name,
                    ms: 0.0,
                    calls: 0,
                    depth,
                    parent,
                });
                (self.current.len() - 1) as u32
            }
        };
        self.open.push((idx, Instant::now()));
        idx
    }

    /// Closes a scope opened by [`Profiler::scope_begin`].
    #[inline]
    pub fn scope_end(&mut self, token: u32) {
        if !self.enabled || token == u32::MAX {
            return;
        }
        // Close nested scopes that were not explicitly ended (defensive: keeps
        // the timer stack balanced).
        while let Some((idx, start)) = self.open.pop() {
            let dt = start.elapsed().as_secs_f32() * 1000.0;
            if let Some(s) = self.current.get_mut(idx as usize) {
                s.ms += dt;
                s.calls += 1;
            }
            if idx == token {
                break;
            }
        }
    }

    /// Times a closure as a named scope.
    #[inline]
    pub fn scope<R>(&mut self, name: &'static str, f: impl FnOnce() -> R) -> R {
        let t = self.scope_begin(name);
        let r = f();
        self.scope_end(t);
        r
    }

    /// The most recently completed frame.
    #[inline]
    #[must_use]
    pub fn last_frame(&self) -> &FrameProfile {
        &self.last
    }

    /// True when the last frame exceeded the budget.
    #[inline]
    #[must_use]
    pub fn over_budget(&self) -> bool {
        self.last.total_ms > self.budget_ms
    }
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_step_rate_matches_hz() {
        let mut c = GameClock::new(60.0);
        c.begin_frame(1.0);
        let mut steps = 0;
        while c.step() {
            steps += 1;
            if steps > 1000 {
                panic!("runaway");
            }
        }
        // Clamped to DEFAULT_MAX_FRAME_DELTA (0.25 s) -> 15 steps at 60 Hz.
        assert_eq!(steps, 15, "0.25s at 60Hz");
        assert_eq!(c.tick(), 15);
    }

    #[test]
    fn interpolation_alpha_is_in_unit_range() {
        let mut c = GameClock::new(60.0);
        c.begin_frame(1.0 / 120.0); // half a step
        while c.step() {}
        assert!(c.alpha() > 0.4 && c.alpha() < 0.6, "alpha {}", c.alpha());
    }

    #[test]
    fn pause_freezes_simulation() {
        let mut c = GameClock::new(60.0);
        c.set_paused(true);
        c.begin_frame(1.0);
        assert!(!c.step());
        assert_eq!(c.tick(), 0);
    }

    #[test]
    fn time_scale_scales_advance() {
        let mut c = GameClock::new(60.0);
        c.set_time_scale(0.0);
        c.begin_frame(1.0);
        assert!(!c.step());
        c.set_time_scale(2.0);
        c.begin_frame(0.1);
        let mut n = 0;
        while c.step() {
            n += 1;
        }
        assert_eq!(n, 12, "0.2 s of simulated time at 60 Hz");
    }

    #[test]
    fn substep_cap_bounds_the_loop() {
        let mut c = GameClock::with_fixed_dt(0.001);
        c.set_max_substeps(4);
        c.begin_frame(0.25);
        let mut n = 0;
        while c.step() {
            n += 1;
            assert!(n <= 8, "must not spin forever");
        }
        assert!(n <= 8);
    }

    #[test]
    fn reset_clears_state() {
        let mut c = GameClock::new(60.0);
        c.begin_frame(0.1);
        while c.step() {}
        c.reset();
        assert_eq!(c.tick(), 0);
        assert_eq!(c.elapsed(), 0.0);
    }

    #[test]
    fn frame_timing_statistics() {
        let mut t = FrameTiming::with_capacity(8);
        for i in 0..8 {
            t.push(0.016 + i as f32 * 0.001);
        }
        assert_eq!(t.frame_index(), 8);
        assert!(t.fps() > 40.0 && t.fps() < 70.0, "fps {}", t.fps());
        assert!(t.percentile(0.5) <= t.percentile(1.0));
        assert!(t.min() <= t.mean() && t.mean() <= t.max());
    }

    #[test]
    fn frame_timing_is_bounded() {
        let mut t = FrameTiming::with_capacity(4);
        for _ in 0..1000 {
            t.push(0.01);
        }
        assert!(t.frame_index() == 1000);
        assert!(t.total_time() > 9.0);
    }

    #[test]
    fn frame_timing_ignores_garbage() {
        let mut t = FrameTiming::with_capacity(4);
        t.push(f32::NAN);
        t.push(-1.0);
        assert_eq!(t.frame_index(), 0);
    }

    #[test]
    fn profiler_accumulates_repeated_scopes() {
        let mut p = Profiler::new();
        p.begin_frame(0.016);
        for _ in 0..3 {
            let t = p.scope_begin("npc");
            p.scope_end(t);
        }
        p.end_frame();
        let s = p.last_frame().scope("npc").expect("recorded");
        assert_eq!(s.calls, 3);
    }

    #[test]
    fn profiler_records_nesting() {
        let mut p = Profiler::new();
        p.begin_frame(0.016);
        let outer = p.scope_begin("outer");
        let inner = p.scope_begin("inner");
        p.scope_end(inner);
        p.scope_end(outer);
        p.end_frame();
        assert_eq!(p.last_frame().scope("outer").unwrap().depth, 0);
        assert_eq!(p.last_frame().scope("inner").unwrap().depth, 1);
    }

    #[test]
    fn disabled_profiler_records_nothing() {
        let mut p = Profiler::disabled();
        p.begin_frame(0.016);
        let t = p.scope_begin("x");
        p.scope_end(t);
        p.end_frame();
        assert!(p.last_frame().scope("x").is_none());
    }

    #[test]
    fn frame_profile_table_is_rendered() {
        let mut p = Profiler::new();
        p.begin_frame(0.016);
        let t = p.scope_begin("render");
        p.scope_end(t);
        p.end_frame();
        let table = p.last_frame().to_table();
        assert!(table.contains("render"), "{table}");
    }

    #[test]
    fn stopwatch_measures_time() {
        let mut sw = Stopwatch::start();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let ms = sw.lap_ms();
        assert!(ms >= 1.0, "{ms}");
    }
}
