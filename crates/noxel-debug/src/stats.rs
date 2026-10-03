//! Frame statistics, rolling history and the numbers behind the overlay.

use std::collections::HashMap;

/// A fixed-capacity rolling window of samples.
///
/// A frame budget is a distribution, not a number: "16.6 ms average" hides the
/// 30 ms hitch that players actually notice. Everything that matters keeps a
/// window so the overlay can show p95 and the worst frame, not just the mean.
#[derive(Clone, Debug)]
pub struct Rolling {
    samples: Vec<f32>,
    cursor: usize,
    filled: bool,
    sum: f32,
    max: f32,
    min: f32,
}

impl Rolling {
    /// Creates a window holding `capacity` samples.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: vec![0.0; capacity],
            cursor: 0,
            filled: false,
            sum: 0.0,
            max: 0.0,
            min: 0.0,
        }
    }

    /// Adds a sample.
    pub fn push(&mut self, value: f32) {
        let value = if value.is_finite() { value } else { 0.0 };
        if self.filled {
            self.sum -= self.samples[self.cursor];
        }
        self.samples[self.cursor] = value;
        self.sum += value;
        self.cursor = (self.cursor + 1) % self.samples.len();
        if self.cursor == 0 {
            self.filled = true;
        }
        self.recompute_extremes();
    }

    /// Recomputes min and max.
    ///
    /// `O(n)` on every push, which is fine at a window of a few hundred: it runs
    /// once per frame per metric, not once per entity.
    fn recompute_extremes(&mut self) {
        let n = self.len();
        if n == 0 {
            self.max = 0.0;
            self.min = 0.0;
            return;
        }
        let count = self.used();
        let mut max = f32::NEG_INFINITY;
        let mut min = f32::INFINITY;
        for v in &self.samples[..count] {
            max = max.max(*v);
            min = min.min(*v);
        }
        self.max = max;
        self.min = min;
    }

    /// How many samples have been recorded.
    #[must_use]
    pub fn used(&self) -> usize {
        if self.filled {
            self.samples.len()
        } else {
            self.cursor
        }
    }

    /// The window's capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    /// The number of samples in the window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.used()
    }

    /// True when nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.used() == 0
    }

    /// The mean.
    #[must_use]
    pub fn mean(&self) -> f32 {
        if self.used() == 0 {
            0.0
        } else {
            self.sum / self.used() as f32
        }
    }

    /// The largest sample in the window.
    #[must_use]
    pub fn max(&self) -> f32 {
        self.max
    }

    /// The smallest sample in the window.
    #[must_use]
    pub fn min(&self) -> f32 {
        self.min
    }

    /// The most recent sample.
    #[must_use]
    pub fn last(&self) -> f32 {
        if self.cursor == 0 && !self.filled {
            0.0
        } else {
            let index = (self.cursor + self.samples.len() - 1) % self.samples.len();
            self.samples[index]
        }
    }

    /// A percentile in `[0, 1]`, by nearest rank.
    #[must_use]
    pub fn percentile(&self, p: f32) -> f32 {
        let count = self.used();
        if count == 0 {
            return 0.0;
        }
        let mut sorted: Vec<f32> = self.samples[..count].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let index = ((p.clamp(0.0, 1.0) * (count - 1) as f32).round() as usize).min(count - 1);
        sorted[index]
    }

    /// The `index`-th oldest sample in the window, or `0.0` when out of range.
    ///
    /// Used by the frame-time graph, which draws history left to right.
    #[must_use]
    pub fn at(&self, index: usize) -> f32 {
        let count = self.used();
        if index >= count {
            return 0.0;
        }
        let len = self.samples.len();
        // Before the window has wrapped, the oldest sample is at slot 0; after,
        // it is at the cursor.
        let start = if count < len { 0 } else { self.cursor };
        self.samples[(start + index) % len]
    }

    /// Clears the window.
    pub fn clear(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
        self.filled = false;
        self.sum = 0.0;
        self.max = 0.0;
        self.min = 0.0;
    }
}

/// One frame's worth of measurements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Frame index, counted from the start of the run.
    pub frame: u64,
    /// Seconds spent in the whole frame.
    pub frame_ms: f32,
    /// Seconds spent updating.
    pub update_ms: f32,
    /// Seconds spent rendering.
    pub render_ms: f32,
    /// Seconds spent on the visibility pass.
    pub visibility_ms: f32,
    /// Seconds spent streaming chunks.
    pub stream_ms: f32,
    /// Seconds spent updating NPCs.
    pub npc_ms: f32,
    /// Seconds spent stepping physics.
    pub physics_ms: f32,
    /// Instances drawn.
    pub instances: usize,
    /// Fragments shaded by the rasterizer.
    pub fragments: u64,
    /// Primary rays cast.
    pub rays: u64,
}

impl FrameSample {
    /// The time attributed to nothing in particular, which is the frame time
    /// minus every measured section.
    #[must_use]
    pub fn unattributed_ms(&self) -> f32 {
        (self.frame_ms - self.update_ms - self.render_ms).max(0.0)
    }

    /// A one-line summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "frame {} {:.2}ms (up {:.2} ren {:.2}) {} inst {} frag {} rays",
            self.frame,
            self.frame_ms,
            self.update_ms,
            self.render_ms,
            self.instances,
            self.fragments,
            self.rays
        )
    }
}

/// A named counter that can be sampled over time.
#[derive(Clone, Debug)]
pub struct Counter {
    /// The display name.
    pub name: String,
    /// The current value.
    pub value: f32,
    /// The rolling history.
    pub history: Rolling,
    /// Display unit, for the overlay.
    pub unit: &'static str,
}

impl Counter {
    /// Creates a counter with a 240-frame window.
    #[must_use]
    pub fn new(name: impl Into<String>, unit: &'static str) -> Self {
        Self {
            name: name.into(),
            value: 0.0,
            history: Rolling::new(240),
            unit,
        }
    }

    /// Records a sample.
    pub fn set(&mut self, value: f32) {
        self.value = if value.is_finite() { value } else { 0.0 };
        self.history.push(self.value);
    }

    /// A formatted line for the overlay.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} {:.1}{}", self.name, self.value, self.unit)
    }
}

/// A time budget for one named section of the frame.
#[derive(Clone, Debug)]
pub struct Budget {
    /// The section name.
    pub name: String,
    /// The rolling spend in milliseconds.
    pub spent: Rolling,
    /// The allowance in milliseconds.
    pub allowance_ms: f32,
}

impl Budget {
    /// Creates a budget.
    #[must_use]
    pub fn new(name: impl Into<String>, allowance_ms: f32) -> Self {
        Self {
            name: name.into(),
            spent: Rolling::new(120),
            allowance_ms: allowance_ms.max(0.0),
        }
    }

    /// True when the section is over its allowance on the last frame.
    #[must_use]
    pub fn is_over(&self) -> bool {
        self.spent.last() > self.allowance_ms
    }

    /// How much of the allowance was used, `1.0` meaning exactly on budget.
    #[must_use]
    pub fn utilisation(&self) -> f32 {
        if self.allowance_ms <= 0.0 {
            0.0
        } else {
            self.spent.last() / self.allowance_ms
        }
    }

    /// A line like `physics 2.1/4.0ms ok`.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "{} {:.1}/{:.1}ms {}",
            self.name,
            self.spent.last(),
            self.allowance_ms,
            if self.is_over() { "OVER" } else { "ok" }
        )
    }
}

/// All the numbers a `noxel-debug` consumer can record.
#[derive(Clone, Debug)]
pub struct Stats {
    /// The most recent frame.
    pub current: FrameSample,
    /// Frame times, for the frame-time graph.
    pub frame_times: Rolling,
    /// Section budgets.
    pub budgets: Vec<Budget>,
    /// Free-form named counters.
    pub counters: HashMap<String, Counter>,
    /// Frames recorded since the start of the run.
    pub frames: u64,
    /// The longest frame seen since the start of the run.
    pub worst_frame_ms: f32,
    /// The frame index of the longest frame.
    pub worst_frame_index: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    /// Creates an empty statistics block with the standard budgets.
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: FrameSample::default(),
            frame_times: Rolling::new(240),
            budgets: vec![
                Budget::new("update", 4.0),
                Budget::new("physics", 4.0),
                Budget::new("visibility", 1.5),
                Budget::new("stream", 3.0),
                Budget::new("npc", 4.0),
                Budget::new("render", 6.0),
            ],
            counters: HashMap::new(),
            frames: 0,
            worst_frame_ms: 0.0,
            worst_frame_index: 0,
        }
    }

    /// Records a counter.
    pub fn record(&mut self, name: &str, value: f32) {
        self.counters
            .entry(name.to_string())
            .or_insert_with(|| Counter::new(name, ""))
            .set(value);
    }

    /// Records a counter with a display unit.
    pub fn record_unit(&mut self, name: &str, value: f32, unit: &'static str) {
        let counter = self
            .counters
            .entry(name.to_string())
            .or_insert_with(|| Counter::new(name, unit));
        counter.unit = unit;
        counter.set(value);
    }

    /// The current value of a counter, or `0`.
    #[must_use]
    pub fn counter(&self, name: &str) -> f32 {
        self.counters.get(name).map_or(0.0, |c| c.value)
    }

    /// A named section budget.
    #[must_use]
    pub fn budget(&self, name: &str) -> Option<&Budget> {
        self.budgets.iter().find(|b| b.name == name)
    }

    /// Records a finished frame.
    pub fn end_frame(&mut self, sample: FrameSample) {
        self.frames += 1;
        self.frame_times.push(sample.frame_ms);
        if sample.frame_ms > self.worst_frame_ms {
            self.worst_frame_ms = sample.frame_ms;
            self.worst_frame_index = sample.frame;
        }
        self.current = sample;
    }

    /// Records a section's spend into its budget, creating it on first use.
    pub fn spend(&mut self, section: &str, ms: f32, allowance_ms: f32) {
        if let Some(budget) = self.budgets.iter_mut().find(|b| b.name == section) {
            budget.spent.push(ms);
            if allowance_ms > 0.0 {
                budget.allowance_ms = allowance_ms;
            }
        } else {
            let mut budget = Budget::new(section, allowance_ms);
            budget.spent.push(ms);
            self.budgets.push(budget);
        }
    }

    /// Frames per second implied by the mean frame time.
    #[must_use]
    pub fn mean_fps(&self) -> f32 {
        let mean = self.frame_times.mean();
        if mean <= 0.0 { 0.0 } else { 1000.0 / mean }
    }

    /// Frames per second implied by the 95th-percentile frame time.
    ///
    /// This is the number that predicts whether the game *feels* smooth: the
    /// mean can be 60 while the p95 is 33.
    #[must_use]
    pub fn p95_fps(&self) -> f32 {
        let p95 = self.frame_times.percentile(0.95);
        if p95 <= 0.0 { 0.0 } else { 1000.0 / p95 }
    }

    /// Clears everything except the budget definitions.
    pub fn reset(&mut self) {
        self.frame_times.clear();
        for budget in &mut self.budgets {
            budget.spent.clear();
        }
        for counter in self.counters.values_mut() {
            counter.history.clear();
        }
        self.frames = 0;
        self.worst_frame_ms = 0.0;
        self.worst_frame_index = 0;
        self.current = FrameSample::default();
    }

    /// The lines the overlay prints.
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "{:.0} fps  {:.2}ms avg  {:.2}ms p95  {:.2}ms worst",
                self.mean_fps(),
                self.frame_times.mean(),
                self.frame_times.percentile(0.95),
                self.worst_frame_ms
            ),
            format!(
                "{} instances  {} fragments  {} rays",
                self.current.instances, self.current.fragments, self.current.rays
            ),
        ];
        for budget in &self.budgets {
            if budget.spent.last() > 0.0 {
                lines.push(budget.line());
            }
        }
        let mut names: Vec<&String> = self.counters.keys().collect();
        names.sort();
        for name in names {
            if let Some(counter) = self.counters.get(name) {
                lines.push(counter.line());
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_starts_empty() {
        let r = Rolling::new(8);
        assert!(r.is_empty());
        assert_eq!(r.mean(), 0.0);
        assert_eq!(r.percentile(0.5), 0.0);
        assert_eq!(r.last(), 0.0);
    }

    #[test]
    fn rolling_accumulates_the_mean() {
        let mut r = Rolling::new(8);
        for v in [1.0, 2.0, 3.0] {
            r.push(v);
        }
        assert_eq!(r.len(), 3);
        assert!((r.mean() - 2.0).abs() < 1e-6);
        assert_eq!(r.last(), 3.0);
        assert_eq!(r.max(), 3.0);
        assert_eq!(r.min(), 1.0);
    }

    #[test]
    fn rolling_wraps_and_forgets_the_oldest() {
        let mut r = Rolling::new(4);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0] {
            r.push(v);
        }
        assert_eq!(r.len(), 4);
        // Only 3, 4, 5, 6 remain.
        assert!((r.mean() - 4.5).abs() < 1e-6, "{}", r.mean());
        assert_eq!(r.min(), 3.0);
        assert_eq!(r.max(), 6.0);
        assert_eq!(r.last(), 6.0);
    }

    #[test]
    fn rolling_handles_non_finite_input() {
        let mut r = Rolling::new(4);
        r.push(f32::NAN);
        r.push(f32::INFINITY);
        r.push(2.0);
        assert!(r.mean().is_finite());
        assert_eq!(r.max(), 2.0);
    }

    #[test]
    fn percentile_is_nearest_rank_and_ordered() {
        let mut r = Rolling::new(101);
        for i in 0..=100 {
            r.push(i as f32);
        }
        assert_eq!(r.percentile(0.0), 0.0);
        assert_eq!(r.percentile(0.5), 50.0);
        assert_eq!(r.percentile(1.0), 100.0);
        assert!(r.percentile(0.95) >= r.percentile(0.5));
    }

    #[test]
    fn percentile_clamps_out_of_range() {
        let mut r = Rolling::new(4);
        r.push(1.0);
        assert_eq!(r.percentile(-1.0), 1.0);
        assert_eq!(r.percentile(5.0), 1.0);
    }

    #[test]
    fn rolling_at_reads_oldest_first() {
        let mut r = Rolling::new(4);
        r.push(1.0);
        r.push(2.0);
        r.push(3.0);
        assert_eq!(r.at(0), 1.0);
        assert_eq!(r.at(2), 3.0);
        assert_eq!(r.at(3), 0.0, "out of range reads zero");
        // After wrapping, the oldest sample is still index 0.
        r.push(4.0);
        r.push(5.0);
        assert_eq!(r.at(0), 2.0);
        assert_eq!(r.at(3), 5.0);
    }

    #[test]
    fn rolling_clear_resets() {
        let mut r = Rolling::new(4);
        r.push(9.0);
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.mean(), 0.0);
        assert_eq!(r.max(), 0.0);
    }

    #[test]
    fn rolling_capacity_is_at_least_one() {
        let r = Rolling::new(0);
        assert_eq!(r.capacity(), 1);
    }

    #[test]
    fn frame_sample_unattributed_time() {
        let s = FrameSample {
            frame_ms: 16.0,
            update_ms: 5.0,
            render_ms: 8.0,
            ..Default::default()
        };
        assert!((s.unattributed_ms() - 3.0).abs() < 1e-6);
        let over = FrameSample {
            frame_ms: 2.0,
            update_ms: 5.0,
            render_ms: 8.0,
            ..Default::default()
        };
        assert_eq!(over.unattributed_ms(), 0.0, "never negative");
    }

    #[test]
    fn frame_sample_summary_mentions_the_frame() {
        let s = FrameSample {
            frame: 42,
            ..Default::default()
        };
        assert!(s.summary().contains("frame 42"));
    }

    #[test]
    fn counters_record_and_format() {
        let mut c = Counter::new("npc", "");
        c.set(120.0);
        assert_eq!(c.value, 120.0);
        assert!(c.line().contains("120"));
        c.set(f32::NAN);
        assert_eq!(c.value, 0.0);
    }

    #[test]
    fn budgets_flag_overspend() {
        let mut b = Budget::new("physics", 4.0);
        b.spent.push(2.0);
        assert!(!b.is_over());
        assert!((b.utilisation() - 0.5).abs() < 1e-6);
        assert!(b.line().contains("ok"));
        b.spent.push(6.0);
        assert!(b.is_over());
        assert!(b.line().contains("OVER"));
    }

    #[test]
    fn zero_allowance_never_reports_utilisation() {
        let b = Budget::new("x", 0.0);
        assert_eq!(b.utilisation(), 0.0);
    }

    #[test]
    fn stats_have_the_standard_budgets() {
        let s = Stats::new();
        for name in ["update", "physics", "visibility", "stream", "npc", "render"] {
            assert!(s.budget(name).is_some(), "{name}");
        }
    }

    #[test]
    fn stats_end_frame_tracks_the_worst() {
        let mut s = Stats::new();
        s.end_frame(FrameSample {
            frame: 0,
            frame_ms: 10.0,
            ..Default::default()
        });
        s.end_frame(FrameSample {
            frame: 1,
            frame_ms: 30.0,
            ..Default::default()
        });
        s.end_frame(FrameSample {
            frame: 2,
            frame_ms: 12.0,
            ..Default::default()
        });
        assert_eq!(s.frames, 3);
        assert_eq!(s.worst_frame_ms, 30.0);
        assert_eq!(s.worst_frame_index, 1);
        assert_eq!(s.current.frame, 2);
    }

    #[test]
    fn stats_fps_is_derived_from_the_window() {
        let mut s = Stats::new();
        for i in 0..10 {
            s.end_frame(FrameSample {
                frame: i,
                frame_ms: 16.0,
                ..Default::default()
            });
        }
        assert!((s.mean_fps() - 62.5).abs() < 0.1, "{}", s.mean_fps());
        assert!(s.p95_fps() > 0.0);
    }

    #[test]
    fn stats_record_counters_by_name() {
        let mut s = Stats::new();
        s.record("npc", 42.0);
        assert_eq!(s.counter("npc"), 42.0);
        assert_eq!(s.counter("missing"), 0.0);
        s.record_unit("chunks", 12.0, " ch");
        assert!(s.counters.get("chunks").is_some_and(|c| c.unit == " ch"));
    }

    #[test]
    fn stats_spend_creates_and_updates_budgets() {
        let mut s = Stats::new();
        s.spend("physics", 2.0, 4.0);
        assert_eq!(s.budget("physics").unwrap().spent.last(), 2.0);
        s.spend("audio", 1.0, 2.0);
        assert!(s.budget("audio").is_some());
        assert_eq!(s.budget("audio").unwrap().allowance_ms, 2.0);
    }

    #[test]
    fn stats_summary_lists_the_headline_numbers() {
        let mut s = Stats::new();
        s.end_frame(FrameSample {
            frame: 0,
            frame_ms: 16.0,
            ..Default::default()
        });
        s.spend("physics", 1.0, 4.0);
        let lines = s.summary_lines();
        assert!(lines[0].contains("fps"), "{}", lines[0]);
        assert!(lines.iter().any(|l| l.contains("physics")));
    }

    #[test]
    fn stats_reset_keeps_the_budget_definitions() {
        let mut s = Stats::new();
        s.end_frame(FrameSample {
            frame: 0,
            frame_ms: 16.0,
            ..Default::default()
        });
        s.reset();
        assert_eq!(s.frames, 0);
        assert_eq!(s.worst_frame_ms, 0.0);
        assert!(s.budget("physics").is_some());
    }
}
