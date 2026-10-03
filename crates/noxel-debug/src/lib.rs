//! # Noxel debug
//!
//! Measurements, overlays and headless frame dumping.
//!
//! Noxel deliberately has **no UI toolkit** (`docs/adr/0009-no-ui.md`). What it
//! has instead is the thing a developer actually needs while building an
//! engine: a way to see the numbers, and a way to turn a frame into a file that
//! can be diffed in CI.
//!
//! ```no_run
//! use noxel_debug::DebugSystem;
//!
//! let mut debug = DebugSystem::standard();
//! // Each frame:
//! debug.begin_frame(1.0 / 60.0);
//! // debug.record_render(&stats);
//! // debug.record_counter("npcs", 240.0);
//! // debug.draw(&mut framebuffer, Some(&view));
//! ```
//!
//! ## Three things, one crate
//!
//! | Module | Purpose |
//! |---|---|
//! | [`stats`] | rolling windows per metric, section budgets, percentiles |
//! | [`overlay`] | panels and graphs drawn into the framebuffer |
//! | [`dump`] | PNG/depth/raw frame output plus an image diff |
//!
//! ## Why percentiles
//!
//! A mean frame time is a lie. A build can average 60 fps while stuttering twice
//! a second, and the stutter is the only thing the player notices. Every rolling
//! window here reports p95 alongside the mean, and a budget line is marked
//! `OVER` the moment a section spends more than its allowance — which is the
//! signal that something regressed.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod dump;
pub mod overlay;
pub mod stats;

pub use dump::{DumpFormat, FrameDumper, ImageDiff};
pub use overlay::{DebugPanel, FrameGraph, PanelSlot};
pub use stats::{Budget, Counter, FrameSample, Rolling, Stats};

use noxel_render::CameraView;
use noxel_render::framebuffer::Framebuffer;
use noxel_render::renderer::{CullCounts, RenderStats};

/// The overlay's configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugConfig {
    /// Draw the statistics panel.
    pub stats_panel: bool,
    /// Draw the frame-time graph.
    pub frame_graph: bool,
    /// Draw the camera readout.
    pub camera_panel: bool,
    /// Draw the culling readout.
    pub culling_panel: bool,
    /// Draw the collision/occlusion visualisers when the caller supplies the
    /// data.
    pub wireframes: bool,
    /// Text scale.
    pub text_scale: u32,
    /// Panel opacity in `[0, 1]`.
    pub panel_alpha: f32,
    /// Record the per-frame section timings.
    pub record_sections: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            stats_panel: true,
            frame_graph: true,
            camera_panel: false,
            culling_panel: true,
            // Wireframes are opt-in: each one costs a query and they are noisy
            // enough that nobody wants them on by default.
            wireframes: false,
            text_scale: 1,
            panel_alpha: 1.0,
            record_sections: true,
        }
    }
}

impl DebugConfig {
    /// Everything on.
    #[must_use]
    pub fn verbose() -> Self {
        Self {
            camera_panel: true,
            wireframes: true,
            ..Self::default()
        }
    }

    /// Only the statistics panel.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            frame_graph: false,
            culling_panel: false,
            ..Self::default()
        }
    }

    /// Everything off.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            stats_panel: false,
            frame_graph: false,
            camera_panel: false,
            culling_panel: false,
            wireframes: false,
            text_scale: 1,
            panel_alpha: 1.0,
            record_sections: false,
        }
    }

    /// True when nothing at all is drawn.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        !self.stats_panel && !self.frame_graph && !self.camera_panel && !self.culling_panel
    }
}

/// A snapshot of one subsystem's numbers, for the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubsystemReport {
    /// Subsystem name.
    pub name: String,
    /// The lines it contributes.
    pub lines: Vec<String>,
}

impl SubsystemReport {
    /// Creates a report.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lines: Vec::new(),
        }
    }

    /// Adds a line.
    pub fn line(&mut self, text: impl Into<String>) {
        self.lines.push(text.into());
    }

    /// Renders as text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = format!("[{}]\n", self.name);
        for line in &self.lines {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// The debug system: statistics, overlay and optional frame dumping.
pub struct DebugSystem {
    config: DebugConfig,
    stats: Stats,
    dumper: Option<FrameDumper>,
    subsystems: Vec<SubsystemReport>,
    update_ms: f32,
}

impl DebugSystem {
    /// Creates a system with the given configuration.
    #[must_use]
    pub fn new(config: DebugConfig) -> Self {
        Self {
            config,
            stats: Stats::new(),
            dumper: None,
            subsystems: Vec::new(),
            update_ms: 0.0,
        }
    }

    /// The default configuration.
    #[must_use]
    pub fn standard() -> Self {
        Self::new(DebugConfig::default())
    }

    /// A system with frame dumping enabled.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the frames directory cannot be
    /// created.
    pub fn with_dump(
        directory: impl Into<std::path::PathBuf>,
        format: DumpFormat,
    ) -> std::io::Result<Self> {
        let mut system = Self::standard();
        system.dumper = Some(FrameDumper::new(directory, format)?);
        Ok(system)
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &DebugConfig {
        &self.config
    }

    /// Replaces the configuration.
    pub fn set_config(&mut self, config: DebugConfig) {
        self.config = config;
    }

    /// The statistics.
    #[must_use]
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Mutable access to the statistics.
    pub fn stats_mut(&mut self) -> &mut Stats {
        &mut self.stats
    }

    /// The frame dumper, if one is attached.
    #[must_use]
    pub fn dumper(&self) -> Option<&FrameDumper> {
        self.dumper.as_ref()
    }

    /// Attaches a frame dumper.
    pub fn set_dumper(&mut self, dumper: Option<FrameDumper>) {
        self.dumper = dumper;
    }

    /// True when the overlay draws nothing.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.config.is_silent()
    }

    /// Starts a frame.
    pub fn begin_frame(&mut self, _dt: f32) {
        self.update_ms = 0.0;
        self.subsystems.clear();
    }

    /// Records the render statistics.
    pub fn record_render(&mut self, stats: &RenderStats) {
        self.stats.current.instances = stats.instances;
        self.stats.current.fragments = stats.pixels_shaded as u64;
        self.stats.current.rays = stats.primary_rays as u64 + stats.secondary_rays as u64;
        self.stats.current.render_ms = stats.ms_total;
        if self.config.record_sections {
            self.stats.spend("render", stats.ms_total, 0.0);
        }
        self.record_counter("triangles", stats.triangles_drawn as f32);
        self.record_counter("fragments", stats.pixels_shaded as f32);
        if stats.primary_rays > 0 || stats.secondary_rays > 0 {
            self.record_counter("primary rays", stats.primary_rays as f32);
            self.record_counter("secondary rays", stats.secondary_rays as f32);
        }
    }

    /// Records the culling statistics.
    pub fn record_culling(&mut self, counts: &CullCounts) {
        self.record_counter("considered", counts.considered as f32);
        self.record_counter("drawn", counts.drawn as f32);
        self.record_counter("culled/frustum", counts.frustum as f32);
        self.record_counter("culled/distance", counts.distance as f32);
        self.record_counter("culled/size", counts.too_small as f32);
        self.record_counter("culled/occluded", counts.occluded as f32);
        self.record_counter("faded", counts.faded as f32);
    }

    /// Records a named section's cost.
    pub fn record_section(&mut self, name: &str, ms: f32) {
        if self.config.record_sections {
            self.stats.spend(name, ms, 0.0);
        }
    }

    /// Records a free-form counter.
    pub fn record_counter(&mut self, name: &str, value: f32) {
        self.stats.record(name, value);
    }

    /// Records a counter with a display unit.
    pub fn record_counter_unit(&mut self, name: &str, value: f32, unit: &'static str) {
        self.stats.record_unit(name, value, unit);
    }

    /// Ends the frame, folding the elapsed update time into the sample.
    pub fn end_frame(&mut self, frame_ms: f32) {
        let mut sample = self.stats.current.clone();
        sample.frame = self.stats.frames;
        sample.frame_ms = frame_ms;
        sample.update_ms = self.update_ms;
        self.stats.end_frame(sample);
    }

    /// Sets the measured update cost for the current frame.
    pub fn set_update_ms(&mut self, ms: f32) {
        self.update_ms = ms;
    }

    /// Adds a subsystem's lines to the report.
    pub fn record_subsystem(&mut self, report: SubsystemReport) {
        self.subsystems.push(report);
    }

    /// A short report of the current frame, for a log line.
    #[must_use]
    pub fn summary(&self) -> String {
        self.stats.current.summary()
    }

    /// The full report, including every recorded subsystem.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Noxel debug report (noxel-debug {})\n",
            env!("CARGO_PKG_VERSION")
        ));
        out.push_str(&format!("frames: {}\n", self.stats.frames));
        out.push_str(&format!(
            "frame: mean {:.2}ms  p50 {:.2}ms  p95 {:.2}ms  worst {:.2}ms (frame {})\n",
            self.stats.frame_times.mean(),
            self.stats.frame_times.percentile(0.5),
            self.stats.frame_times.percentile(0.95),
            self.stats.worst_frame_ms,
            self.stats.worst_frame_index
        ));
        out.push_str(&format!(
            "fps: mean {:.1}  p95 {:.1}\n",
            self.stats.mean_fps(),
            self.stats.p95_fps()
        ));
        out.push_str("\nbudgets\n");
        for budget in &self.stats.budgets {
            out.push_str(&format!(
                "  {:<12} last {:.3}ms  mean {:.3}ms  p95 {:.3}ms  allowance {:.1}ms\n",
                budget.name,
                budget.spent.last(),
                budget.spent.mean(),
                budget.spent.percentile(0.95),
                budget.allowance_ms
            ));
        }
        out.push_str("\ncounters\n");
        let mut names: Vec<&String> = self.stats.counters.keys().collect();
        names.sort();
        for name in names {
            if let Some(counter) = self.stats.counters.get(name) {
                out.push_str(&format!(
                    "  {:<20} {:>10.1}  min {:>8.1}  max {:>8.1}\n",
                    counter.name,
                    counter.value,
                    counter.history.min(),
                    counter.history.max()
                ));
            }
        }
        for subsystem in &self.subsystems {
            out.push('\n');
            out.push_str(&subsystem.to_text());
        }
        out
    }

    /// Draws every enabled panel into the framebuffer.
    pub fn draw(&mut self, framebuffer: &mut Framebuffer, camera: Option<&CameraView>) {
        if self.is_silent() {
            return;
        }
        let mut overlay = overlay::Overlay::new();
        overlay.set_text_scale(self.config.text_scale);
        let mut next_y = 2u32;

        if self.config.stats_panel {
            let mut lines = self.stats.summary_lines();
            if self.config.camera_panel {
                if let Some(camera) = camera {
                    lines.push(camera_line(camera));
                }
            }
            if !self.config.culling_panel {
                lines.retain(|l| !l.contains("culled/"));
            }
            let panel = DebugPanel::new(PanelSlot::TopLeft, lines);
            next_y = panel.draw(&overlay, framebuffer, next_y, self.config.panel_alpha);
        }
        if self.config.frame_graph {
            let graph = FrameGraph::new(framebuffer.width().min(200), framebuffer.height().min(40));
            graph.draw(
                &overlay,
                framebuffer,
                &self.stats.frame_times,
                2,
                next_y,
                self.config.panel_alpha,
            );
        }
    }

    /// Draws the occlusion visualiser: the focus ray and every blocking volume.
    pub fn draw_occlusion(
        &self,
        framebuffer: &mut Framebuffer,
        camera: &CameraView,
        focus: noxel_core::math::Vec3,
        blockers: &[noxel_core::math::Aabb],
    ) {
        if !self.config.wireframes {
            return;
        }
        let overlay = overlay::Overlay::new();
        overlay.line(
            framebuffer,
            camera,
            camera.position,
            focus,
            noxel_core::math::Color8::new(255, 220, 0, 255),
        );
        for bounds in blockers {
            overlay.aabb(
                framebuffer,
                camera,
                bounds,
                noxel_core::math::Color8::new(255, 0, 128, 255),
            );
        }
    }

    /// Draws a set of boxes, for the collision visualiser.
    pub fn draw_boxes(
        &self,
        framebuffer: &mut Framebuffer,
        camera: &CameraView,
        boxes: &[noxel_core::math::Aabb],
    ) {
        if !self.config.wireframes {
            return;
        }
        let overlay = overlay::Overlay::new();
        for bounds in boxes {
            overlay.aabb(
                framebuffer,
                camera,
                bounds,
                noxel_core::math::Color8::new(0, 255, 220, 255),
            );
        }
    }

    /// Draws a marker at a world position.
    pub fn draw_marker(
        &self,
        framebuffer: &mut Framebuffer,
        camera: &CameraView,
        position: noxel_core::math::Vec3,
        size: f32,
        color: noxel_core::math::Color8,
    ) {
        if !self.config.wireframes {
            return;
        }
        overlay::Overlay::new().cross(framebuffer, camera, position, size, color);
    }

    /// Writes the current frame to the dumper, when one is attached.
    ///
    /// The frame number is the debug system's own count, which increments at the
    /// *end* of a frame, so the first dump is `frame_000000.png`.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the frame cannot be written.
    pub fn dump_frame(
        &mut self,
        framebuffer: &Framebuffer,
    ) -> std::io::Result<Option<std::path::PathBuf>> {
        let frame = self.stats.frames;
        self.dump_frame_at(framebuffer, frame)
    }

    /// Writes a frame under an explicit number.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the frame cannot be written.
    pub fn dump_frame_at(
        &mut self,
        framebuffer: &Framebuffer,
        frame: u64,
    ) -> std::io::Result<Option<std::path::PathBuf>> {
        match &mut self.dumper {
            Some(dumper) => Ok(Some(dumper.dump(framebuffer, frame)?)),
            None => Ok(None),
        }
    }

    /// True when a frame dumper is attached.
    #[must_use]
    pub fn is_dumping(&self) -> bool {
        self.dumper.is_some()
    }
}

impl Default for DebugSystem {
    fn default() -> Self {
        Self::standard()
    }
}

impl core::fmt::Debug for DebugSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DebugSystem")
            .field("frames", &self.stats.frames)
            .field("dumping", &self.dumper.is_some())
            .field("silent", &self.is_silent())
            .finish()
    }
}

/// A one-line camera readout.
///
/// The yaw is recovered from the view direction rather than stored: a
/// [`CameraView`] is the projection, not the rig, so the heading has to be
/// derived. `-.atan2` matches the engine's convention that a yaw of zero looks
/// along `-Z`.
fn camera_line(camera: &CameraView) -> String {
    let yaw = (-camera.forward.x).atan2(-camera.forward.z);
    format!(
        "camera {:.1},{:.1},{:.1} yaw {:.0}deg {}",
        camera.position.x,
        camera.position.y,
        camera.position.z,
        yaw.to_degrees(),
        if camera.is_orthographic() {
            "ortho"
        } else {
            "persp"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> CameraView {
        CameraView::orthographic(
            noxel_core::math::Vec3::new(0.0, 20.0, 0.0),
            noxel_core::math::Vec3::ZERO,
            noxel_core::math::Vec3::Z,
            20.0,
            1.0,
            1.0,
            100.0,
        )
    }

    #[test]
    fn default_config_shows_the_panels() {
        let c = DebugConfig::default();
        assert!(c.stats_panel && c.frame_graph && c.culling_panel);
        assert!(
            !c.wireframes,
            "wireframes are opt-in: they cost a query each"
        );
        assert!(!c.is_silent());
    }

    #[test]
    fn disabled_config_is_silent() {
        assert!(DebugConfig::disabled().is_silent());
        assert!(!DebugConfig::verbose().is_silent());
        assert!(!DebugConfig::minimal().is_silent());
    }

    #[test]
    fn system_starts_empty() {
        let system = DebugSystem::standard();
        assert_eq!(system.stats().frames, 0);
        assert!(system.dumper().is_none());
    }

    #[test]
    fn recording_a_render_frame_populates_counters() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        let stats = RenderStats {
            instances: 12,
            triangles_drawn: 30,
            pixels_shaded: 4000,
            primary_rays: 60,
            secondary_rays: 120,
            ms_total: 3.5,
            ..Default::default()
        };
        system.record_render(&stats);
        system.end_frame(16.0);
        assert_eq!(system.stats().current.instances, 12);
        assert_eq!(system.stats().current.fragments, 4000);
        assert_eq!(system.stats().current.rays, 180);
        assert_eq!(system.stats().counter("triangles"), 30.0);
        assert_eq!(system.stats().frames, 1);
    }

    #[test]
    fn recording_culling_populates_every_bucket() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        let counts = CullCounts {
            considered: 100,
            drawn: 40,
            frustum: 30,
            distance: 20,
            too_small: 5,
            occluded: 3,
            faded: 2,
            ..Default::default()
        };
        system.record_culling(&counts);
        assert_eq!(system.stats().counter("considered"), 100.0);
        assert_eq!(system.stats().counter("culled/frustum"), 30.0);
        assert_eq!(system.stats().counter("culled/occluded"), 3.0);
        assert_eq!(system.stats().counter("faded"), 2.0);
    }

    #[test]
    fn sections_are_budgeted() {
        let mut system = DebugSystem::standard();
        system.record_section("physics", 2.5);
        let budget = system.stats().budget("physics").unwrap();
        assert_eq!(budget.spent.last(), 2.5);
        assert!(!budget.is_over());
    }

    #[test]
    fn sections_can_be_suppressed() {
        let mut system = DebugSystem::new(DebugConfig {
            record_sections: false,
            ..DebugConfig::default()
        });
        system.record_section("physics", 9.0);
        assert_eq!(system.stats().budget("physics").unwrap().spent.last(), 0.0);
    }

    #[test]
    fn counters_support_units() {
        let mut system = DebugSystem::standard();
        system.record_counter_unit("chunks", 49.0, " chunks");
        assert!(
            system
                .stats()
                .counters
                .get("chunks")
                .is_some_and(|c| c.unit == " chunks")
        );
    }

    #[test]
    fn report_contains_every_section() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        system.record_section("physics", 2.0);
        system.record_counter("npcs", 120.0);
        system.record_subsystem(SubsystemReport::new("world"));
        system.end_frame(16.0);
        let report = system.report();
        assert!(report.contains("frames: 1"), "{report}");
        assert!(report.contains("budgets"));
        assert!(report.contains("physics"));
        assert!(report.contains("npcs"));
        assert!(report.contains("[world]"));
    }

    #[test]
    fn subsystem_report_formats_its_lines() {
        let mut r = SubsystemReport::new("npc");
        r.line("active 240");
        r.line("tier 0 12");
        let text = r.to_text();
        assert!(text.starts_with("[npc]"));
        assert!(text.contains("  active 240"));
    }

    #[test]
    fn draw_does_nothing_when_silent() {
        let mut system = DebugSystem::new(DebugConfig::disabled());
        let mut fb = Framebuffer::new(64, 32);
        let before = fb.color_slice().to_vec();
        system.draw(&mut fb, Some(&camera()));
        assert_eq!(fb.color_slice(), before.as_slice());
    }

    #[test]
    fn draw_writes_the_statistics_panel() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        system.end_frame(16.0);
        let mut fb = Framebuffer::new(200, 120);
        system.draw(&mut fb, Some(&camera()));
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.1)
            .count();
        assert!(lit > 50, "the panel must draw something: {lit}");
    }

    #[test]
    fn wireframes_are_skipped_unless_enabled() {
        let box_bounds = noxel_core::math::Aabb::new(
            noxel_core::math::Vec3::splat(-1.0),
            noxel_core::math::Vec3::splat(1.0),
        );
        let system = DebugSystem::standard();
        let mut fb = Framebuffer::new(64, 64);
        let before = fb.color_slice().to_vec();
        system.draw_boxes(&mut fb, &camera(), &[box_bounds]);
        assert_eq!(fb.color_slice(), before.as_slice());

        let verbose = DebugSystem::new(DebugConfig::verbose());
        verbose.draw_boxes(&mut fb, &camera(), &[box_bounds]);
        assert_ne!(fb.color_slice(), before.as_slice());
    }

    #[test]
    fn occlusion_view_is_drawn_when_enabled() {
        let system = DebugSystem::new(DebugConfig::verbose());
        let mut fb = Framebuffer::new(64, 64);
        system.draw_occlusion(
            &mut fb,
            &camera(),
            noxel_core::math::Vec3::ZERO,
            &[noxel_core::math::Aabb::new(
                noxel_core::math::Vec3::splat(-1.0),
                noxel_core::math::Vec3::splat(1.0),
            )],
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.1 || c[1] > 0.1)
            .count();
        assert!(lit > 0);
    }

    #[test]
    fn markers_are_drawn_when_enabled() {
        let system = DebugSystem::new(DebugConfig::verbose());
        let mut fb = Framebuffer::new(64, 64);
        system.draw_marker(
            &mut fb,
            &camera(),
            noxel_core::math::Vec3::ZERO,
            1.0,
            noxel_core::math::Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 0);
    }

    #[test]
    fn dump_frame_without_a_dumper_is_a_no_op() {
        let mut system = DebugSystem::standard();
        let fb = Framebuffer::new(4, 4);
        assert!(system.dump_frame(&fb).unwrap().is_none());
        assert!(!system.is_dumping());
    }

    #[test]
    fn dump_frame_at_uses_the_given_number() {
        let dir = std::env::temp_dir().join("noxel-debug-explicit-frame");
        let _ = std::fs::remove_dir_all(&dir);
        let mut system = DebugSystem::with_dump(&dir, DumpFormat::Png).unwrap();
        let fb = Framebuffer::new(4, 4);
        let path = system.dump_frame_at(&fb, 42).unwrap().unwrap();
        assert!(path.ends_with("frame_000042.png"), "{}", path.display());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dump_frame_writes_when_a_dumper_is_attached() {
        let dir = std::env::temp_dir().join("noxel-debug-system-dump");
        let _ = std::fs::remove_dir_all(&dir);
        let mut system = DebugSystem::with_dump(&dir, DumpFormat::Png).unwrap();
        let fb = Framebuffer::new(4, 4);
        let path = system.dump_frame(&fb).unwrap().unwrap();
        assert!(path.exists());
        assert_eq!(system.dumper().unwrap().dumped(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_is_short_but_informative() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        system.end_frame(16.0);
        assert!(system.summary().contains("frame"), "{}", system.summary());
    }

    #[test]
    fn config_can_be_replaced() {
        let mut system = DebugSystem::standard();
        system.set_config(DebugConfig::disabled());
        assert!(system.is_silent());
    }

    #[test]
    fn debug_impl_reports_state() {
        let system = DebugSystem::standard();
        let text = format!("{system:?}");
        assert!(text.contains("DebugSystem"));
    }

    #[test]
    fn camera_line_reports_orientation() {
        let line = camera_line(&camera());
        assert!(line.contains("ortho"), "{line}");
        assert!(line.contains("yaw"));
    }

    #[test]
    fn update_time_is_recorded_in_the_sample() {
        let mut system = DebugSystem::standard();
        system.begin_frame(1.0 / 60.0);
        system.set_update_ms(4.5);
        system.end_frame(16.0);
        assert!((system.stats().current.update_ms - 4.5).abs() < 1e-6);
    }
}
