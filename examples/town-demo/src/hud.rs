//! The demo's on-screen readout and its final report.
//!
//! This is not a UI framework (see `docs/adr/0009-no-ui.md`). It is the two
//! things a demo needs: a line of text in the corner so a screenshot is
//! self-explanatory, and a report at the end so a terminal run says what
//! happened.

use noxel_app::App;
use noxel_core::math::{Color8, Vec3};
use noxel_render::framebuffer::Framebuffer;
use noxel_render::overlay::Overlay;

use crate::args::Args;
use crate::terrain::TerrainPlugin;

/// Draws the demo's overlay: a title line, the clock, and the crowd count.
pub fn draw(app: &App, framebuffer: &mut Framebuffer) {
    let mut overlay = Overlay::new();
    // The overlay draws before the resolve, so the text lands under the tone
    // curve and keeps the exact colour that was asked for.
    overlay.set_depth_test(false);

    let hours = (app.context.elapsed / 60.0) % 24.0;
    let lines = [
        format!("TOWN DEMO  SEED {:#X}", app.config.seed),
        format!(
            "DAY {:02}:{:02}  FRAME {}",
            hours as u32,
            ((hours % 1.0) * 60.0) as u32,
            app.context.frame
        ),
        format!("{} INSTANCES", app.context.visible.items.len()),
        format!("{} CHUNKS", app.context.streamer.stats().loaded),
    ];
    let width = lines
        .iter()
        .map(|l| noxel_render::overlay::Font::text_width(l, 1))
        .max()
        .unwrap_or(0)
        + 6;
    let height = lines.len() as u32 * (noxel_render::overlay::Font::text_height(1) + 2) + 4;
    overlay.screen_rect(
        framebuffer,
        2,
        2,
        width,
        height,
        Color8::new(8, 10, 16, 190),
        true,
    );
    for (i, line) in lines.iter().enumerate() {
        let y = 4 + i as u32 * (noxel_render::overlay::Font::text_height(1) + 2);
        overlay.text(framebuffer, 5, y, line, Color8::new(236, 240, 248, 255));
    }
}

/// Draws a marker over every occluder currently fading, so a screenshot shows
/// *why* a roof is see-through.
pub fn draw_occluders(app: &App, framebuffer: &mut Framebuffer) {
    let overlay = Overlay::new();
    for (handle, instance) in app.scene().instances() {
        if !instance.flags.occluder {
            continue;
        }
        let alpha = app.context.visibility.fade_of(handle);
        if alpha > 0.99 {
            continue;
        }
        let bounds = instance.bounds();
        overlay.aabb(
            framebuffer,
            &app.context.view,
            &bounds,
            Color8::new(255, 96, 160, (alpha * 255.0) as u8),
        );
    }
}

/// Draws a marker at a world position.
pub fn draw_marker(app: &App, framebuffer: &mut Framebuffer, position: Vec3, color: Color8) {
    let overlay = Overlay::new();
    overlay.cross(framebuffer, &app.context.view, position, 0.6, color);
}

/// The end-of-run report.
#[must_use]
pub fn final_report(app: &App, args: &Args, wall_seconds: f32) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let stream = app.context.streamer.stats();
    let debug = app.debug();

    let _ = writeln!(out, "world");
    let _ = writeln!(out, "  seed            {:#x}", args.seed);
    let _ = writeln!(out, "  chunks resident {}", stream.loaded);
    let _ = writeln!(
        out,
        "  chunk memory    {:.1} MiB",
        stream.memory_bytes as f32 / (1024.0 * 1024.0)
    );
    let _ = writeln!(
        out,
        "  cache hits/miss {}/{}",
        stream.cache_hits, stream.cache_misses
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "frame");
    let _ = writeln!(
        out,
        "  mean {:.2} ms   p50 {:.2} ms   p95 {:.2} ms   worst {:.2} ms",
        debug.stats().frame_times.mean(),
        debug.stats().frame_times.percentile(0.5),
        debug.stats().frame_times.percentile(0.95),
        debug.stats().worst_frame_ms
    );
    let _ = writeln!(
        out,
        "  {:.1} fps mean, {:.1} fps p95",
        debug.stats().mean_fps(),
        debug.stats().p95_fps()
    );
    let _ = writeln!(out, "  wall clock {wall_seconds:.2} s");
    let _ = writeln!(out);

    let _ = writeln!(out, "budgets (last / allowance)");
    for budget in &debug.stats().budgets {
        let _ = writeln!(out, "  {:<12} {}", budget.name, budget.line());
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "counters");
    let mut names: Vec<&String> = debug.stats().counters.keys().collect();
    names.sort();
    for name in names {
        if let Some(counter) = debug.stats().counters.get(name) {
            let _ = writeln!(out, "  {:<14} {:>10.1}", counter.name, counter.value);
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "next steps");
    let _ = writeln!(
        out,
        "  * open the newest file in {}",
        args.dump
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "frames/".to_string())
    );
    let _ = writeln!(
        out,
        "  * `--mode hybrid` for ray-traced shadows and ambient occlusion"
    );
    let _ = writeln!(
        out,
        "  * `docs/02-getting-started.md` to attach a window and real input"
    );
    out
}

/// A one-line status for a log.
#[must_use]
pub fn status_line(app: &App) -> String {
    let stats = app.debug().stats();
    format!(
        "frame {} {:.2}ms {} instances {} chunks",
        app.context.frame,
        stats.frame_times.last(),
        app.context.visible.items.len(),
        app.context.streamer.stats().loaded
    )
}

/// The terrain summary the report prints.
#[must_use]
pub fn terrain_line(plugin: &TerrainPlugin) -> String {
    crate::terrain::describe(plugin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_app::AppConfig;

    fn app() -> App {
        App::new(AppConfig::headless()).unwrap()
    }

    #[test]
    fn draw_writes_something() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        let mut fb = Framebuffer::new(200, 100);
        draw(&app, &mut fb);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.1)
            .count();
        assert!(lit > 20, "{lit}");
    }

    #[test]
    fn occluder_boxes_are_skipped_when_nothing_fades() {
        let app = app();
        let mut fb = Framebuffer::new(64, 64);
        draw_occluders(&app, &mut fb);
        assert!(fb.color_slice().iter().all(|c| *c == 0.0));
    }

    #[test]
    fn markers_draw() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        let mut fb = Framebuffer::new(64, 64);
        draw_marker(&app, &mut fb, Vec3::ZERO, Color8::WHITE);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 0);
    }

    #[test]
    fn final_report_names_the_sections() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        app.debug_mut().end_frame(16.0);
        let text = final_report(&app, &Args::default(), 1.0);
        for section in ["world", "frame", "budgets", "counters", "next steps"] {
            assert!(text.contains(section), "{section} missing");
        }
    }

    #[test]
    fn status_line_is_one_line() {
        let app = app();
        let text = status_line(&app);
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("frame"));
    }

    #[test]
    fn terrain_line_describes_the_plugin() {
        let plugin = TerrainPlugin::new();
        assert!(terrain_line(&plugin).contains("terrain"));
    }
}
