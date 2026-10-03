//! Running the app: one frame at a time, or a whole headless session.

use noxel_asset::image::Image;
use noxel_core::time::Stopwatch;

use crate::app::App;

/// How long to run for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunMode {
    /// Run an exact number of frames as fast as possible.
    Headless {
        /// Frames to render.
        frames: u64,
    },
    /// Run for a wall-clock duration at a nominal frame rate.
    ///
    /// Used by a host that owns the real clock; the app never sleeps.
    Realtime {
        /// Seconds of simulated time.
        seconds: f32,
        /// The nominal frame rate the frames are simulated at.
        fps: f32,
    },
}

impl RunMode {
    /// The number of frames this mode will run.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        match *self {
            Self::Headless { frames } => frames,
            Self::Realtime { seconds, fps } => (seconds.max(0.0) * fps.max(1.0)).round() as u64,
        }
    }

    /// The frame delta this mode feeds the app.
    #[must_use]
    pub fn frame_dt(&self) -> f32 {
        match *self {
            Self::Headless { .. } => 1.0 / 60.0,
            Self::Realtime { fps, .. } => 1.0 / fps.max(1.0),
        }
    }
}

impl Default for RunMode {
    fn default() -> Self {
        Self::Headless { frames: 600 }
    }
}

/// What a headless run measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadlessReport {
    /// Frames rendered.
    pub frames: u64,
    /// Fixed update steps executed.
    pub steps: u64,
    /// Wall-clock milliseconds for the whole run.
    pub total_ms: f32,
    /// Mean frame time over the run, in milliseconds.
    pub mean_frame_ms: f32,
    /// 95th-percentile frame time.
    pub p95_frame_ms: f32,
    /// Worst frame time.
    pub worst_frame_ms: f32,
    /// Frames per second over the whole run.
    pub fps: f32,
    /// Fragments shaded in total.
    pub fragments: u64,
    /// Rays cast in total.
    pub rays: u64,
    /// The most instances drawn in any single frame.
    pub peak_instances: usize,
    /// Chunks resident at the end of the run.
    pub chunks: usize,
    /// Bytes of chunk memory at the end of the run.
    pub chunk_bytes: usize,
    /// Frames written to disk.
    pub dumped: u64,
    /// The last frame's image, when [`App::step`] was asked to keep it.
    pub last_image: Option<Image>,
}

impl HeadlessReport {
    /// A one-line summary, for a CI log or a test assertion.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} frames in {:.1}ms ({:.1} fps): mean {:.2}ms  p95 {:.2}ms  worst {:.2}ms | \
             {} fragments  {} rays | {} chunks ({:.1} MiB) | {} steps | {} dumped",
            self.frames,
            self.total_ms,
            self.fps,
            self.mean_frame_ms,
            self.p95_frame_ms,
            self.worst_frame_ms,
            self.fragments,
            self.rays,
            self.chunks,
            self.chunk_bytes as f32 / (1024.0 * 1024.0),
            self.steps,
            self.dumped
        )
    }

    /// True when every frame stayed inside the given budget.
    #[must_use]
    pub fn within_budget(&self, budget_ms: f32) -> bool {
        self.p95_frame_ms <= budget_ms
    }
}

impl App {
    /// Advances the app by one frame and renders it.
    ///
    /// Returns the wall-clock milliseconds the frame took.
    pub fn step(&mut self, frame_dt: f32) -> f32 {
        let watch = Stopwatch::start();
        self.context.debug.begin_frame(frame_dt);

        let steps = self.fixed_update(frame_dt);
        self.context.debug.record_counter("steps", steps as f32);
        self.frame_update(frame_dt);
        self.render();
        self.draw_overlays();

        self.context.frame += 1;
        self.context.elapsed += frame_dt;
        self.context.input.end_frame();

        let frame_ms = watch.elapsed_ms() as f32;
        self.context.debug.set_update_ms(frame_ms);
        // Dump *before* the statistics advance, so the first file is frame zero.
        if self.config.dump.is_some() {
            let _ = self.context.debug.dump_frame(&self.framebuffer);
        }
        self.context.debug.end_frame(frame_ms);
        frame_ms
    }

    /// Runs a whole session as fast as the CPU allows, with no window.
    ///
    /// This is the path CI uses: it renders every frame for real, so a golden
    /// image or a crash is caught without a display server.
    pub fn run_headless(&mut self, frames: u64) -> HeadlessReport {
        self.run_mode(RunMode::Headless { frames })
    }

    /// Runs a session in the given mode.
    pub fn run_mode(&mut self, mode: RunMode) -> HeadlessReport {
        let frames = mode.frame_count();
        let frame_dt = mode.frame_dt();
        let mut report = HeadlessReport::default();
        let watch = Stopwatch::start();

        for _ in 0..frames {
            let frame_ms = self.step(frame_dt);
            let stats = self.context.debug.stats();
            report.frames += 1;
            report.fragments += stats.current.fragments;
            report.rays += stats.current.rays;
            report.peak_instances = report.peak_instances.max(stats.current.instances);
            let _ = frame_ms;
        }
        report.dumped = self.context.debug.dumper().map_or(0, |d| d.dumped());

        let stats = self.context.debug.stats();
        report.total_ms = watch.elapsed_ms() as f32;
        report.mean_frame_ms = stats.frame_times.mean();
        report.p95_frame_ms = stats.frame_times.percentile(0.95);
        report.worst_frame_ms = stats.worst_frame_ms;
        report.fps = if report.total_ms > 0.0 {
            report.frames as f32 * 1000.0 / report.total_ms
        } else {
            0.0
        };
        report.steps = self.context.clock.tick();
        let stream = self.context.streamer.stats();
        report.chunks = stream.loaded;
        report.chunk_bytes = stream.memory_bytes;
        report
    }

    /// Renders a single frame and returns it as an sRGB image.
    ///
    /// Used by the golden-frame tests and by anything that wants a still without
    /// going through the file system.
    pub fn render_still(&mut self, frame_dt: f32) -> Image {
        self.step(frame_dt);
        self.resolve()
    }

    /// Runs until the app has rendered `frames`, dumping the last frame into
    /// `report.last_image`.
    pub fn run_and_capture(&mut self, frames: u64) -> HeadlessReport {
        let mut report = self.run_headless(frames);
        report.last_image = Some(self.resolve());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;

    fn app() -> App {
        App::new(AppConfig::headless()).unwrap()
    }

    #[test]
    fn headless_frame_count_is_exact() {
        assert_eq!(RunMode::Headless { frames: 12 }.frame_count(), 12);
    }

    #[test]
    fn realtime_frame_count_is_derived() {
        let mode = RunMode::Realtime {
            seconds: 1.0,
            fps: 60.0,
        };
        assert_eq!(mode.frame_count(), 60);
        assert!((mode.frame_dt() - 1.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn realtime_clamps_degenerate_input() {
        let mode = RunMode::Realtime {
            seconds: -5.0,
            fps: 0.0,
        };
        assert_eq!(mode.frame_count(), 0);
        assert!(mode.frame_dt() <= 1.0);
    }

    #[test]
    fn default_mode_is_a_ten_second_run() {
        assert_eq!(RunMode::default().frame_count(), 600);
    }

    #[test]
    fn step_advances_the_frame_counter() {
        let mut app = app();
        app.step(1.0 / 60.0);
        app.step(1.0 / 60.0);
        assert_eq!(app.context.frame, 2);
        assert!(app.context.elapsed > 0.0);
    }

    #[test]
    fn step_renders_something() {
        let mut app = app();
        app.step(1.0 / 60.0);
        assert!(app.framebuffer.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn run_headless_reports_every_field() {
        let mut app = app();
        let report = app.run_headless(5);
        assert_eq!(report.frames, 5);
        assert!(report.steps >= 5);
        assert!(report.total_ms > 0.0);
        assert!(report.mean_frame_ms > 0.0);
        assert!(report.fps > 0.0);
        assert!(report.worst_frame_ms >= report.mean_frame_ms);
    }

    #[test]
    fn report_summary_mentions_the_numbers() {
        let mut app = app();
        let report = app.run_headless(3);
        let text = report.summary();
        assert!(text.contains("3 frames"), "{text}");
        assert!(text.contains("fps"));
        assert!(text.contains("chunks"));
    }

    #[test]
    fn report_budget_check_works() {
        let report = HeadlessReport {
            p95_frame_ms: 10.0,
            ..Default::default()
        };
        assert!(report.within_budget(16.0));
        assert!(!report.within_budget(8.0));
    }

    #[test]
    fn zero_frames_is_a_valid_run() {
        let mut app = app();
        let report = app.run_headless(0);
        assert_eq!(report.frames, 0);
        assert_eq!(report.fps, 0.0);
    }

    #[test]
    fn realtime_mode_runs_the_expected_frames() {
        let mut app = app();
        let report = app.run_mode(RunMode::Realtime {
            seconds: 0.1,
            fps: 60.0,
        });
        assert_eq!(report.frames, 6);
    }

    #[test]
    fn render_still_returns_an_image() {
        let mut app = app();
        let image = app.render_still(1.0 / 60.0);
        assert_eq!((image.width, image.height), (160, 90));
    }

    #[test]
    fn run_and_capture_keeps_the_last_frame() {
        let mut app = app();
        let report = app.run_and_capture(2);
        assert!(report.last_image.is_some());
        assert_eq!(report.last_image.unwrap().width, 160);
    }

    #[test]
    fn dumping_counts_every_frame() {
        let dir = std::env::temp_dir().join("noxel-app-dump-run");
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::new(AppConfig::dumping(&dir)).unwrap();
        let report = app.run_headless(3);
        assert_eq!(report.dumped, 3);
        assert!(dir.join("frame_000000.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunk_counters_are_reported() {
        let mut app = app();
        let report = app.run_headless(2);
        assert!(
            report.chunks > 0,
            "the streamer must load the chunks around the origin"
        );
        assert!(report.chunk_bytes > 0);
    }
}
