//! Staged system scheduling.
//!
//! The scheduler is generic over a context type `C`, so the ECS crate does not
//! need to know about the renderer, physics or assets. `noxel-app` instantiates
//! it with its own `AppContext`.
//!
//! # Why stages
//!
//! Systems within a stage run in the order they were added; stages run in the
//! order they were declared. That gives a deterministic, readable frame:
//!
//! ```text
//! Input -> Simulation -> Physics -> Npc -> Transform -> Visibility -> Render -> Present
//! ```
//!
//! Anything that must not interleave (physics stepping vs. moving transforms)
//! is separated by a stage boundary rather than by hoping the insertion order
//! works out. See `docs/guides/frame-layout.md` for the canonical stage list the
//! demo uses.
//!
//! ```no_run
//! use noxel_ecs::Scheduler;
//!
//! struct Ctx { counter: u32 }
//!
//! let mut scheduler: Scheduler<Ctx> = Scheduler::new();
//! scheduler.add_stage("sim");
//! scheduler.add_system("sim", "tick", |c: &mut Ctx| c.counter += 1);
//! scheduler.add_system("sim", "double", |c: &mut Ctx| c.counter *= 2);
//!
//! let mut ctx = Ctx { counter: 1 };
//! scheduler.run(&mut ctx);
//! assert_eq!(ctx.counter, 4); // (1 + 1) * 2
//! ```
//!
//! Systems that touch *different* subsystems can also run in parallel through
//! [`noxel_core::jobs::JobPool`]; the scheduler deliberately does not do that
//! automatically, because implicit parallelism makes a frame far harder to
//! debug and buys little on the tile-shaped workloads Noxel actually has.

use noxel_core::time::Stopwatch;

/// One registered system.
pub struct SystemEntry<C> {
    /// Human-readable name, shown in the profiler and the debug overlay.
    pub name: &'static str,
    /// Set to `false` to skip the system without unregistering it.
    pub enabled: bool,
    /// How many times the system has run.
    pub runs: u64,
    /// Wall time of the most recent run, in milliseconds.
    pub last_ms: f32,
    /// Cumulative wall time, in milliseconds.
    pub total_ms: f64,
    /// The system body.
    run: Box<dyn FnMut(&mut C)>,
}

impl<C> SystemEntry<C> {
    /// Runs the system.
    #[inline]
    pub fn run(&mut self, ctx: &mut C) {
        (self.run)(ctx);
    }

    /// Average cost per run in milliseconds.
    #[must_use]
    pub fn mean_ms(&self) -> f32 {
        if self.runs == 0 {
            0.0
        } else {
            (self.total_ms / self.runs as f64) as f32
        }
    }
}

impl<C> core::fmt::Debug for SystemEntry<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SystemEntry")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("runs", &self.runs)
            .field("last_ms", &self.last_ms)
            .finish()
    }
}

/// A named group of systems that always run together, in order.
#[derive(Debug)]
pub struct Stage<C> {
    /// Stage name, e.g. `"physics"`.
    pub name: &'static str,
    /// Systems, in run order.
    pub systems: Vec<SystemEntry<C>>,
}

impl<C> Stage<C> {
    /// Number of registered systems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// True when the stage has no systems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Sum of the systems' last-run times, in milliseconds.
    #[must_use]
    pub fn last_ms(&self) -> f32 {
        self.systems
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.last_ms)
            .sum()
    }
}

/// A flat report of one system's cost, for the debug overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemStat {
    /// Stage the system belongs to.
    pub stage: &'static str,
    /// System name.
    pub system: &'static str,
    /// Times run.
    pub runs: u64,
    /// Milliseconds in the most recent frame.
    pub last_ms: f32,
    /// Mean milliseconds per run.
    pub mean_ms: f32,
    /// Whether the system is enabled.
    pub enabled: bool,
}

/// The ordered collection of stages and systems.
pub struct Scheduler<C> {
    stages: Vec<Stage<C>>,
    /// Total systems ever run, for the frame counter.
    frames: u64,
}

impl<C> Scheduler<C> {
    /// Creates an empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            frames: 0,
        }
    }

    /// Adds a stage.
    ///
    /// # Panics
    ///
    /// Panics if a stage with the same name already exists. Stage names are a
    /// compile-time-ish contract; a duplicate is always a bug, and failing at
    /// startup is better than silently running two stages with one name.
    pub fn add_stage(&mut self, name: &'static str) -> &mut Self {
        assert!(
            !self.stages.iter().any(|s| s.name == name),
            "duplicate scheduler stage `{name}`"
        );
        self.stages.push(Stage {
            name,
            systems: Vec::new(),
        });
        self
    }

    /// Registers a system in a stage.
    ///
    /// # Panics
    ///
    /// Panics if the stage does not exist. Register stages before systems; the
    /// app runtime does this once at startup.
    pub fn add_system(
        &mut self,
        stage: &'static str,
        name: &'static str,
        run: impl FnMut(&mut C) + 'static,
    ) {
        let index = self
            .stages
            .iter()
            .position(|s| s.name == stage)
            .unwrap_or_else(|| panic!("system `{name}` added to unknown stage `{stage}`"));
        self.stages[index].systems.push(SystemEntry {
            name,
            enabled: true,
            runs: 0,
            last_ms: 0.0,
            total_ms: 0.0,
            run: Box::new(run),
        });
    }

    /// Runs every enabled system in every stage, in order.
    pub fn run(&mut self, ctx: &mut C) {
        for stage in &mut self.stages {
            for system in &mut stage.systems {
                if !system.enabled {
                    continue;
                }
                let mut watch = Stopwatch::start();
                system.run(ctx);
                let ms = watch.lap_ms() as f32;
                system.last_ms = ms;
                system.total_ms += ms as f64;
                system.runs += 1;
            }
        }
        self.frames += 1;
    }

    /// Runs only the named stage. Useful for editor-style stepping and tests.
    pub fn run_stage(&mut self, stage: &str, ctx: &mut C) {
        let Some(s) = self.stages.iter_mut().find(|s| s.name == stage) else {
            return;
        };
        for system in &mut s.systems {
            if !system.enabled {
                continue;
            }
            let mut watch = Stopwatch::start();
            system.run(ctx);
            let ms = watch.lap_ms() as f32;
            system.last_ms = ms;
            system.total_ms += ms as f64;
            system.runs += 1;
        }
    }

    /// Number of completed full runs.
    #[inline]
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The stages.
    #[must_use]
    pub fn stages(&self) -> &[Stage<C>] {
        &self.stages
    }

    /// Total number of registered systems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stages.iter().map(|s| s.systems.len()).sum()
    }

    /// True when no systems are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Enables or disables one system by name. Returns `false` when not found.
    pub fn set_enabled(&mut self, system: &str, enabled: bool) -> bool {
        for stage in &mut self.stages {
            for s in &mut stage.systems {
                if s.name == system {
                    s.enabled = enabled;
                    return true;
                }
            }
        }
        false
    }

    /// Enables or disables every system in a stage.
    pub fn set_stage_enabled(&mut self, stage: &str, enabled: bool) -> bool {
        match self.stages.iter_mut().find(|s| s.name == stage) {
            Some(s) => {
                for sys in &mut s.systems {
                    sys.enabled = enabled;
                }
                true
            }
            None => false,
        }
    }

    /// Looks up a system's entry.
    #[must_use]
    pub fn system(&self, system: &str) -> Option<&SystemEntry<C>> {
        self.stages
            .iter()
            .flat_map(|s| s.systems.iter())
            .find(|s| s.name == system)
    }

    /// A flat cost report, heaviest first — what the debug overlay prints.
    #[must_use]
    pub fn stats(&self) -> Vec<SystemStat> {
        let mut out: Vec<SystemStat> = self
            .stages
            .iter()
            .flat_map(|stage| {
                stage.systems.iter().map(move |s| SystemStat {
                    stage: stage.name,
                    system: s.name,
                    runs: s.runs,
                    last_ms: s.last_ms,
                    mean_ms: s.mean_ms(),
                    enabled: s.enabled,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.last_ms
                .partial_cmp(&a.last_ms)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        out
    }

    /// The wall time of the most recent full run, in milliseconds: the sum of
    /// the per-system timings.
    #[must_use]
    pub fn last_frame_ms(&self) -> f32 {
        self.stages.iter().map(|s| s.last_ms()).sum()
    }

    /// Renders the cost report as an indented table.
    #[must_use]
    pub fn to_table(&self) -> String {
        let mut out =
            String::from("stage     system                    last ms   mean ms   runs\n");
        for stat in self.stats() {
            out.push_str(&format!(
                "{:<9} {:<25} {:>7.3}   {:>7.3}   {}{}\n",
                stat.stage,
                stat.system,
                stat.last_ms,
                stat.mean_ms,
                stat.runs,
                if stat.enabled { "" } else { "  (disabled)" }
            ));
        }
        out
    }

    /// Clears every timing counter but keeps the registrations.
    pub fn reset_stats(&mut self) {
        for stage in &mut self.stages {
            for s in &mut stage.systems {
                s.runs = 0;
                s.last_ms = 0.0;
                s.total_ms = 0.0;
            }
        }
        self.frames = 0;
    }
}

impl<C> Default for Scheduler<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> core::fmt::Debug for Scheduler<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Scheduler")
            .field(
                "stages",
                &self.stages.iter().map(|s| s.name).collect::<Vec<_>>(),
            )
            .field("systems", &self.len())
            .field("frames", &self.frames)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Debug)]
    struct Ctx {
        log: Vec<&'static str>,
        value: i32,
    }

    fn scheduler() -> Scheduler<Ctx> {
        let mut s: Scheduler<Ctx> = Scheduler::new();
        s.add_stage("a");
        s.add_stage("b");
        s.add_system("a", "one", |c: &mut Ctx| c.log.push("a1"));
        s.add_system("a", "two", |c: &mut Ctx| c.log.push("a2"));
        s.add_system("b", "three", |c: &mut Ctx| c.log.push("b1"));
        s
    }

    #[test]
    fn systems_run_in_declaration_order() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        assert_eq!(ctx.log, vec!["a1", "a2", "b1"]);
    }

    #[test]
    fn running_twice_repeats_the_frame() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        s.run(&mut ctx);
        assert_eq!(ctx.log.len(), 6);
        assert_eq!(s.frames(), 2);
    }

    #[test]
    fn context_is_mutated() {
        let mut s: Scheduler<Ctx> = Scheduler::new();
        s.add_stage("v");
        s.add_system("v", "inc", |c: &mut Ctx| c.value += 1);
        s.add_system("v", "double", |c: &mut Ctx| c.value *= 2);
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        assert_eq!(ctx.value, 2);
    }

    #[test]
    fn disabled_system_is_skipped() {
        let mut s = scheduler();
        assert!(s.set_enabled("two", false));
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        assert_eq!(ctx.log, vec!["a1", "b1"]);
    }

    #[test]
    fn disabling_a_whole_stage() {
        let mut s = scheduler();
        assert!(s.set_stage_enabled("a", false));
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        assert_eq!(ctx.log, vec!["b1"]);
    }

    #[test]
    fn run_single_stage() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run_stage("b", &mut ctx);
        assert_eq!(ctx.log, vec!["b1"]);
    }

    #[test]
    fn unknown_stage_lookups_are_false() {
        let mut s = scheduler();
        assert!(!s.set_enabled("nope", false));
        assert!(!s.set_stage_enabled("nope", false));
        assert!(s.system("nope").is_none());
    }

    #[test]
    fn stats_report_every_system() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        let stats = s.stats();
        assert_eq!(stats.len(), 3);
        assert!(stats.iter().all(|st| st.runs == 1));
        assert!(stats.iter().all(|st| st.enabled));
    }

    #[test]
    fn stats_are_sorted_heaviest_first() {
        let mut s: Scheduler<Ctx> = Scheduler::new();
        s.add_stage("s");
        s.add_system("s", "cheap", |_c: &mut Ctx| {});
        // A real sleep, not a spin: the timer resolution is microseconds and a
        // busy loop of this size gets constant-folded away in release builds.
        s.add_system("s", "expensive", |_c: &mut Ctx| {
            std::thread::sleep(std::time::Duration::from_millis(2));
        });
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        let stats = s.stats();
        assert_eq!(stats[0].system, "expensive", "{stats:?}");
        // And the ordering invariant always holds.
        assert!(
            stats.windows(2).all(|w| w[0].last_ms >= w[1].last_ms),
            "{stats:?}"
        );
    }

    #[test]
    fn table_renders() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        let table = s.to_table();
        assert!(table.contains("one") && table.contains("three"));
    }

    #[test]
    fn reset_stats_keeps_registrations() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        s.reset_stats();
        assert_eq!(s.len(), 3);
        assert_eq!(s.frames(), 0);
        assert!(s.stats().iter().all(|st| st.runs == 0));
    }

    #[test]
    fn len_counts_systems() {
        let s = scheduler();
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        let empty: Scheduler<Ctx> = Scheduler::new();
        assert!(empty.is_empty());
    }

    #[test]
    fn stages_report_their_own_cost() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        assert!(s.stages()[0].last_ms() >= 0.0);
        assert_eq!(s.stages()[0].len(), 2);
        assert!(!s.stages()[0].is_empty());
    }

    #[test]
    #[should_panic(expected = "duplicate scheduler stage")]
    fn duplicate_stage_panics() {
        let mut s: Scheduler<Ctx> = Scheduler::new();
        s.add_stage("x");
        s.add_stage("x");
    }

    #[test]
    #[should_panic(expected = "unknown stage")]
    fn unknown_stage_panics() {
        let mut s: Scheduler<Ctx> = Scheduler::new();
        s.add_system("missing", "sys", |_c: &mut Ctx| {});
    }

    #[test]
    fn frame_time_is_the_sum_of_systems() {
        let mut s = scheduler();
        let mut ctx = Ctx::default();
        s.run(&mut ctx);
        let sum: f32 = s.stats().iter().map(|x| x.last_ms).sum();
        assert!((s.last_frame_ms() - sum).abs() < 1e-3);
    }
}
