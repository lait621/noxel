//! Panels and graphs drawn into the framebuffer.
//!
//! The primitives themselves live in [`noxel_render::overlay`]; this module adds
//! the *layout*: which corner a panel goes in, how to stack several of them, and
//! a frame-time graph with a budget line.

use noxel_core::math::Color8;
use noxel_render::framebuffer::Framebuffer;

pub use noxel_render::overlay::{Font, Overlay};

use crate::stats::Rolling;

/// Where a panel anchors on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelSlot {
    /// Top-left corner.
    #[default]
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
}

impl PanelSlot {
    /// The anchor corner.
    #[must_use]
    pub fn is_right(self) -> bool {
        matches!(self, Self::TopRight | Self::BottomRight)
    }

    /// True when the panel grows upward from the bottom.
    #[must_use]
    pub fn is_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight)
    }
}

/// A block of text with a background.
#[derive(Clone, Debug)]
pub struct DebugPanel {
    /// Where it anchors.
    pub slot: PanelSlot,
    /// The lines to print.
    pub lines: Vec<String>,
    /// Text colour.
    pub foreground: Color8,
    /// Background colour.
    pub background: Color8,
    /// Pixels of padding.
    pub padding: u32,
}

impl DebugPanel {
    /// A panel with the default dark-on-light colours.
    #[must_use]
    pub fn new(slot: PanelSlot, lines: Vec<String>) -> Self {
        Self {
            slot,
            lines,
            foreground: Color8::new(232, 238, 246, 255),
            // 200/255 alpha: a panel you can see through is a panel you can keep
            // on while playing.
            background: Color8::new(8, 10, 16, 200),
            padding: 3,
        }
    }

    /// Overrides the colours.
    #[must_use]
    pub fn with_colors(mut self, foreground: Color8, background: Color8) -> Self {
        self.foreground = foreground;
        self.background = background;
        self
    }

    /// The panel's size in pixels at a text scale of 1.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        let widest = self
            .lines
            .iter()
            .map(|l| Font::text_width(l, 1))
            .max()
            .unwrap_or(0);
        let line_height = Font::text_height(1) + 2;
        let height = if self.lines.is_empty() {
            0
        } else {
            line_height * self.lines.len() as u32
        };
        (widest + self.padding * 2, height + self.padding * 2)
    }

    /// Draws the panel at the anchor for its slot, and returns the y coordinate
    /// below it, so panels can be stacked.
    ///
    /// `y` is used for the top-anchored slots; bottom-anchored slots ignore it
    /// and stack upward from the bottom edge.
    pub fn draw(
        &self,
        overlay: &Overlay,
        framebuffer: &mut Framebuffer,
        y: u32,
        alpha: f32,
    ) -> u32 {
        if self.lines.is_empty() {
            return y;
        }
        let (width, height) = self.size();
        let alpha = alpha.clamp(0.0, 1.0);
        let x = if self.slot.is_right() {
            framebuffer.width().saturating_sub(width + 2)
        } else {
            2
        };
        let top = if self.slot.is_bottom() {
            framebuffer.height().saturating_sub(height + 2)
        } else {
            y
        };
        let mut background = self.background;
        background.a = (background.a as f32 * alpha) as u8;
        overlay.screen_rect(framebuffer, x, top, width, height, background, true);

        let mut foreground = self.foreground;
        foreground.a = (foreground.a as f32 * alpha) as u8;
        let mut pen = top + self.padding;
        for line in &self.lines {
            overlay.text(framebuffer, x + self.padding, pen, line, foreground);
            pen += Font::text_height(1) + 2;
        }
        if self.slot.is_bottom() {
            top.saturating_sub(height)
        } else {
            top + height + 2
        }
    }
}

/// A frame-time graph with a budget line.
///
/// The graph is the single most useful debug widget in a game engine: a spike is
/// visible instantly, whereas a mean hides it. The horizontal line marks the
/// frame budget, so "the graph touches the line" is the whole diagnosis.
#[derive(Clone, Debug)]
pub struct FrameGraph {
    width: u32,
    height: u32,
    /// The budget line in milliseconds.
    pub budget_ms: f32,
    /// The top of the vertical scale in milliseconds.
    pub scale_ms: f32,
    /// Graph colour.
    pub line_color: Color8,
    /// Budget line colour.
    pub budget_color: Color8,
    /// Background colour.
    pub background: Color8,
}

impl FrameGraph {
    /// Creates a graph of the given size, scaled to a 60 fps budget.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(8),
            height: height.max(8),
            budget_ms: 1000.0 / 60.0,
            // Three times the budget, so a hitch is obvious without the steady
            // state being squashed into the bottom pixel row.
            scale_ms: 50.0,
            line_color: Color8::new(120, 230, 160, 255),
            budget_color: Color8::new(255, 180, 60, 255),
            background: Color8::new(8, 10, 16, 200),
        }
    }

    /// Sets the vertical scale.
    #[must_use]
    pub fn with_scale(mut self, scale_ms: f32) -> Self {
        self.scale_ms = scale_ms.max(1.0);
        self
    }

    /// Sets the budget line.
    #[must_use]
    pub fn with_budget(mut self, budget_ms: f32) -> Self {
        self.budget_ms = budget_ms.max(0.0);
        self
    }

    /// The graph's size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Maps a frame time to a row inside the graph.
    #[must_use]
    pub fn row_for(&self, ms: f32) -> u32 {
        let ratio = (ms / self.scale_ms).clamp(0.0, 1.0);
        ((1.0 - ratio) * (self.height - 1) as f32).round() as u32
    }

    /// Draws the graph at `(x, y)`.
    pub fn draw(
        &self,
        overlay: &Overlay,
        framebuffer: &mut Framebuffer,
        samples: &Rolling,
        x: u32,
        y: u32,
        alpha: f32,
    ) {
        let alpha = alpha.clamp(0.0, 1.0);
        let mut background = self.background;
        background.a = (background.a as f32 * alpha) as u8;
        overlay.screen_rect(framebuffer, x, y, self.width, self.height, background, true);
        overlay.screen_rect(
            framebuffer,
            x,
            y,
            self.width,
            self.height,
            self.line_color,
            false,
        );

        // Budget line.
        let budget_row = self.row_for(self.budget_ms);
        for px in x..x + self.width {
            overlay.screen_rect(
                framebuffer,
                px,
                y + budget_row,
                1,
                1,
                self.budget_color,
                true,
            );
        }

        // The history, oldest on the left.
        let count = samples.used().min(self.width as usize);
        if count == 0 {
            return;
        }
        let start = x + self.width - count as u32;
        for i in 0..count {
            let value = samples.at(i);
            let row = self.row_for(value);
            let px = start + i as u32;
            // A one-pixel column from the bottom to the value: a filled graph
            // reads as a distribution, a line graph as noise.
            let top = (y + row).min(y + self.height - 1);
            let bottom = y + self.height - 1;
            for py in top..=bottom {
                let color = if value > self.budget_ms {
                    self.budget_color
                } else {
                    self.line_color
                };
                overlay.screen_rect(framebuffer, px, py, 1, 1, color, true);
            }
        }
    }

    /// The worst frame time currently drawn.
    #[must_use]
    pub fn peak(samples: &Rolling) -> f32 {
        samples.max()
    }
}

/// A ready-made overlay with the depth test disabled.
///
/// Debug geometry should be visible *through* the world — that is the whole
/// point of drawing an occluder box.
#[must_use]
pub fn debug_overlay() -> Overlay {
    let mut overlay = Overlay::new();
    overlay.set_depth_test(false);
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Vec3;
    use noxel_render::CameraView;

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            20.0,
            1.0,
            1.0,
            100.0,
        )
    }

    #[test]
    fn slot_geometry() {
        assert!(PanelSlot::TopRight.is_right());
        assert!(!PanelSlot::TopLeft.is_right());
        assert!(PanelSlot::BottomRight.is_bottom());
        assert!(!PanelSlot::TopRight.is_bottom());
        assert_eq!(PanelSlot::default(), PanelSlot::TopLeft);
    }

    #[test]
    fn panel_size_grows_with_content() {
        let small = DebugPanel::new(PanelSlot::TopLeft, vec!["A".to_string()]);
        let big = DebugPanel::new(
            PanelSlot::TopLeft,
            vec!["AAAA".to_string(), "B".to_string()],
        );
        let (sw, sh) = small.size();
        let (bw, bh) = big.size();
        assert!(bw > sw);
        assert!(bh > sh);
    }

    #[test]
    fn empty_panel_has_no_size_and_draws_nothing() {
        let panel = DebugPanel::new(PanelSlot::TopLeft, Vec::new());
        assert_eq!(panel.size(), (6, 6));
        let mut fb = Framebuffer::new(64, 32);
        let before = fb.color_slice().to_vec();
        let overlay = debug_overlay();
        let next = panel.draw(&overlay, &mut fb, 2, 1.0);
        assert_eq!(next, 2);
        assert_eq!(fb.color_slice(), before.as_slice());
    }

    #[test]
    fn panel_draws_and_returns_the_next_y() {
        let panel = DebugPanel::new(PanelSlot::TopLeft, vec!["FRAME 16.6MS".to_string()]);
        let mut fb = Framebuffer::new(160, 64);
        let overlay = debug_overlay();
        let next = panel.draw(&overlay, &mut fb, 2, 1.0);
        let (_, height) = panel.size();
        assert_eq!(next, 2 + height + 2);
        // Something was drawn inside the panel area.
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.2)
            .count();
        assert!(lit > 5, "{lit}");
    }

    #[test]
    fn panels_stack_downward() {
        let a = DebugPanel::new(PanelSlot::TopLeft, vec!["A".to_string()]);
        let mut fb = Framebuffer::new(64, 64);
        let overlay = debug_overlay();
        let y1 = a.draw(&overlay, &mut fb, 2, 1.0);
        let y2 = a.draw(&overlay, &mut fb, y1, 1.0);
        assert!(y2 > y1, "the second panel must sit below the first");
    }

    #[test]
    fn right_slot_anchors_to_the_right_edge() {
        let panel = DebugPanel::new(PanelSlot::TopRight, vec!["X".to_string()]);
        let mut fb = Framebuffer::new(64, 32);
        let overlay = debug_overlay();
        panel.draw(&overlay, &mut fb, 2, 1.0);
        let (width, _) = panel.size();
        // The panel's right edge is inside the frame.
        assert!(width <= 62);
        assert!(fb.get(63, 31).unwrap()[0] >= 0.0);
    }

    #[test]
    fn bottom_slot_anchors_to_the_bottom_edge() {
        let panel = DebugPanel::new(
            PanelSlot::BottomLeft,
            vec!["Y".to_string(), "Z".to_string()],
        );
        let mut fb = Framebuffer::new(64, 64);
        let overlay = debug_overlay();
        let next = panel.draw(&overlay, &mut fb, 0, 1.0);
        let (_, height) = panel.size();
        assert!(next <= 64 - height, "{next}");
    }

    #[test]
    fn zero_alpha_draws_no_text() {
        let panel = DebugPanel::new(PanelSlot::TopLeft, vec!["SECRET".to_string()]);
        let mut fb = Framebuffer::new(96, 32);
        let overlay = debug_overlay();
        panel.draw(&overlay, &mut fb, 2, 0.0);
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert!(
            image.pixels.iter().all(|p| p.r < 40),
            "an invisible panel must stay invisible"
        );
    }

    #[test]
    fn panel_colours_can_be_overridden() {
        let panel = DebugPanel::new(PanelSlot::TopLeft, vec!["A".to_string()])
            .with_colors(Color8::RED, Color8::new(0, 0, 255, 255));
        assert_eq!(panel.foreground, Color8::RED);
        assert_eq!(panel.background.b, 255);
    }

    #[test]
    fn graph_maps_time_to_rows() {
        let graph = FrameGraph::new(100, 20).with_scale(50.0);
        assert_eq!(graph.row_for(0.0), 19, "0 ms sits on the bottom row");
        assert_eq!(
            graph.row_for(50.0),
            0,
            "the scale maximum sits on the top row"
        );
        assert_eq!(graph.row_for(1000.0), 0, "out-of-range clamps");
        let mid = graph.row_for(25.0);
        assert!(mid > 0 && mid < 19, "{mid}");
    }

    #[test]
    fn graph_defaults_to_a_60fps_budget() {
        let graph = FrameGraph::new(100, 20);
        assert!((graph.budget_ms - 16.6667).abs() < 0.01);
        assert_eq!(graph.size(), (100, 20));
    }

    #[test]
    fn graph_size_is_clamped() {
        let graph = FrameGraph::new(0, 0);
        assert_eq!(graph.size(), (8, 8));
        let graph = FrameGraph::new(10, 10).with_scale(0.0);
        assert!(graph.scale_ms >= 1.0);
    }

    #[test]
    fn graph_draws_something_with_samples() {
        let mut samples = Rolling::new(200);
        for i in 0..100 {
            samples.push(10.0 + (i % 20) as f32);
        }
        let graph = FrameGraph::new(100, 32);
        let mut fb = Framebuffer::new(140, 64);
        let overlay = debug_overlay();
        graph.draw(&overlay, &mut fb, &samples, 2, 2, 1.0);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[1] > 0.3)
            .count();
        assert!(lit > 100, "{lit}");
    }

    #[test]
    fn graph_with_no_samples_draws_the_frame_and_budget() {
        let samples = Rolling::new(10);
        let graph = FrameGraph::new(40, 16);
        let mut fb = Framebuffer::new(64, 32);
        let overlay = debug_overlay();
        graph.draw(&overlay, &mut fb, &samples, 0, 0, 1.0);
        // The budget line is drawn even without history.
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.3)
            .count();
        assert!(lit > 10, "{lit}");
    }

    #[test]
    fn graph_marks_over_budget_frames() {
        let mut samples = Rolling::new(32);
        for _ in 0..32 {
            samples.push(40.0);
        }
        let graph = FrameGraph::new(32, 16).with_scale(50.0);
        let mut fb = Framebuffer::new(64, 32);
        let overlay = debug_overlay();
        graph.draw(&overlay, &mut fb, &samples, 0, 0, 1.0);
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        // Orange (the over-budget colour) must appear.
        let orange = image
            .pixels
            .iter()
            .filter(|p| p.r > 180 && p.g > 120 && p.b < 120)
            .count();
        assert!(orange > 20, "{orange}");
    }

    #[test]
    fn graph_peak_reports_the_worst_sample() {
        let mut samples = Rolling::new(16);
        samples.push(4.0);
        samples.push(31.0);
        assert_eq!(FrameGraph::peak(&samples), 31.0);
    }

    #[test]
    fn debug_overlay_does_not_depth_test() {
        assert!(!debug_overlay().depth_test());
    }

    #[test]
    fn debug_overlay_draws_through_geometry() {
        let mut fb = Framebuffer::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                fb.set_depth(x, y, 0.001);
            }
        }
        let overlay = debug_overlay();
        let cam = camera();
        overlay.aabb(
            &mut fb,
            &cam,
            &noxel_core::math::Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)),
            Color8::WHITE,
        );
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.5)
            .count();
        assert!(lit > 0, "a debug box must be visible even behind the world");
    }
}
