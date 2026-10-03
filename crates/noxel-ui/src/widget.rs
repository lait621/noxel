//! The widgets.
//!
//! These are methods on [`Ui`] rather than free functions because a widget needs
//! three things at once — the theme, the font and the interaction state — and a
//! free function would take all three as arguments at every call site. The
//! painter stays a separate argument, so drawing a widget never borrows the `Ui`
//! and a caller can build a painter from anything.
//!
//! Every widget takes the rectangle it should occupy. There is no implicit
//! cursor: a layout is a few [`UiRect`] cuts at the top of a function, which is
//! easier to read at a glance than a chain of `.next_row()` calls, and it lets a
//! panel be laid out in any order.

use noxel_asset::image::Image;
use noxel_core::math::Color8;

use crate::context::{Id, Response, TextRole, Tooltip, Ui, text_color};
use crate::font::{TextAlign, TextStyle};
use crate::geom::{Anchor, Insets, UiRect};
use crate::painter::Painter;
use crate::theme::{FrameStyle, darken, lighten};

/// A region of a texture, ready to draw.
#[derive(Clone, Copy, Debug)]
pub struct Sprite<'a> {
    /// The texture the region lives in.
    pub image: &'a Image,
    /// The region, in texels.
    pub rect: UiRect,
}

impl<'a> Sprite<'a> {
    /// A sprite cut from `image`.
    #[must_use]
    pub const fn new(image: &'a Image, rect: UiRect) -> Self {
        Self { image, rect }
    }

    /// The region's size in texels.
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        (self.rect.w, self.rect.h)
    }
}

/// How a slot is currently being used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotState {
    /// The player has selected this slot.
    pub selected: bool,
    /// The pointer is over it.
    pub hovered: bool,
    /// It holds nothing and is drawn dimmed.
    pub empty: bool,
    /// Draw a count in the corner.
    pub count: Option<u32>,
}

impl Ui {
    /// Draws a container: a nine-slice frame, or a flat box when the style has
    /// no art.
    pub fn panel(&self, painter: &mut Painter<'_>, rect: UiRect, style: &FrameStyle) {
        painter.frame(&self.texture, style, rect);
    }

    /// Registers a panel as pointer-blocking without making it interactive.
    ///
    /// A HUD panel that swallowed clicks without this would let a click on the
    /// clock also swing a hoe in the world behind it.
    pub fn panel_blocking(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        style: &FrameStyle,
    ) {
        painter.frame(&self.texture, style, rect);
        self.state.block_pointer(rect, input);
    }

    /// Draws one line of text.
    pub fn label(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        text: &str,
        style: &TextStyle,
    ) -> u32 {
        self.font.draw_text(painter, rect, text, style)
    }

    /// Draws text in one of the theme's semantic roles.
    pub fn label_role(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        text: &str,
        role: TextRole,
    ) -> u32 {
        let style = TextStyle::new(text_color(&self.theme, role));
        self.font.draw_text(painter, rect, text, &style)
    }

    /// Draws a title: larger, emphasised, and shadowed so it stays readable over
    /// a bright world.
    pub fn heading(&self, painter: &mut Painter<'_>, rect: UiRect, text: &str) -> u32 {
        let style = TextStyle::new(self.theme.palette.text_strong)
            .with_scale(2)
            .with_shadow(self.theme.palette.shadow);
        self.font.draw_text(painter, rect, text, &style)
    }

    /// A right-aligned value, for a `label ......... value` row.
    pub fn value(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        text: &str,
        role: TextRole,
    ) -> u32 {
        let style = TextStyle::new(text_color(&self.theme, role)).with_align(TextAlign::Right);
        self.font.draw_text(painter, rect, text, &style)
    }

    /// A push button.
    ///
    /// Returns the response; `response.clicked` is the activation. A disabled
    /// button still reports hover so a tooltip can explain *why* it is disabled,
    /// which is the difference between a UI that feels broken and one that does
    /// not.
    pub fn button(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        label: &str,
        enabled: bool,
    ) -> Response {
        let mut response = self.state.interact(id, rect, input);
        if !enabled {
            // A disabled widget must not activate, but it may still hover.
            response.pressed = false;
            response.held = false;
            response.clicked = false;
        }

        let style = if !enabled {
            &self.theme.button.disabled
        } else if response.held {
            &self.theme.button.pressed
        } else if response.hovered {
            &self.theme.button.hover
        } else {
            &self.theme.button.idle
        };
        painter.frame(&self.texture, style, rect);

        let color = if !enabled {
            self.theme.button.text_disabled
        } else if response.hovered {
            self.theme.button.text_hover
        } else {
            self.theme.button.text
        };
        let inner = rect.inset(self.theme.button.padding);
        let text_style = TextStyle::new(color).with_align(TextAlign::Center);
        let (_, height) = self.font.measure(label, &text_style);
        let line = UiRect::new(
            inner.x,
            inner.y + (inner.h as i32 - height as i32) / 2,
            inner.w,
            height,
        );
        self.font.draw_text(painter, line, label, &text_style);
        response
    }

    /// A button with a sprite instead of a label.
    pub fn icon_button(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        icon: Sprite<'_>,
        enabled: bool,
    ) -> Response {
        let mut response = self.state.interact(id, rect, input);
        if !enabled {
            response.pressed = false;
            response.held = false;
            response.clicked = false;
        }
        let style = if !enabled {
            &self.theme.button.disabled
        } else if response.held {
            &self.theme.button.pressed
        } else if response.hovered {
            &self.theme.button.hover
        } else {
            &self.theme.button.idle
        };
        painter.frame(&self.texture, style, rect);
        let (w, h) = icon.size();
        let target = rect.place((w, h), Anchor::Center, (0, 0));
        let tint = if enabled {
            Color8::WHITE
        } else {
            self.theme.palette.text_dim
        };
        painter.blit(icon.image, icon.rect, target.x, target.y, tint);
        response
    }

    /// A progress or energy bar.
    ///
    /// The fill is quantised to whole pixels of the trough, so a bar at 10% of
    /// 40 pixels is 4 wide and not a 4.0-wide rectangle with a half-lit pixel at
    /// its end.
    pub fn bar(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        fraction: f32,
        style: &crate::theme::BarStyle,
    ) {
        painter.frame(&self.texture, &style.trough, rect);
        let inner = rect.inset(Insets::all(1));
        if inner.is_empty() {
            return;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        let filled = (fraction * inner.w as f32).round() as u32;
        if filled == 0 {
            return;
        }
        let fill_rect = UiRect::new(inner.x, inner.y, filled.min(inner.w), inner.h);
        let color = if fraction <= style.low_threshold {
            style.fill_low
        } else {
            style.fill
        };
        painter.fill(fill_rect, color);
        if inner.h > 2 && style.highlight.a != 0 {
            painter.fill(
                UiRect::new(fill_rect.x, fill_rect.y, fill_rect.w, 1),
                style.highlight,
            );
        }
    }

    /// An inventory slot: a square cell that may hold an item.
    ///
    /// `content` is the item's icon, or `None` for an empty slot.
    pub fn slot(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        content: Option<Sprite<'_>>,
        state: SlotState,
    ) -> Response {
        let response = self.state.interact(id, rect, input);
        let hovered = state.hovered || response.hovered;

        let mut style = self.theme.sunken;
        if state.selected {
            style.border = self.theme.palette.accent;
            style.border_width = 2;
        } else if hovered {
            style.border = lighten(self.theme.palette.border, 60);
        }
        painter.frame(&self.texture, &style, rect);

        if let Some(sprite) = content {
            let (w, h) = sprite.size();
            // Shrink to fit rather than overflow: an authored 16x16 icon in a
            // 20x20 slot is centred, and a 32x32 one is not allowed to spill.
            let target = rect.place((w.min(rect.w), h.min(rect.h)), Anchor::Center, (0, 0));
            painter.blit(sprite.image, sprite.rect, target.x, target.y, Color8::WHITE);
        }
        if let Some(count) = state.count.filter(|c| *c > 1) {
            let style = TextStyle::new(Color8::WHITE)
                .with_align(TextAlign::Right)
                .with_shadow(Color8::new(0, 0, 0, 220));
            let text_rect = UiRect::new(rect.x, rect.bottom() - 10, rect.w - 2, 9);
            self.font
                .draw_text(painter, text_rect, &count.to_string(), &style);
        }
        if hovered {
            self.state.block_pointer(rect, input);
        }
        response
    }

    /// Queues a tooltip anchored to a rectangle.
    ///
    /// Queued rather than drawn, because a tooltip must be on top of every panel
    /// and the widget that wants one is drawn long before the last panel is.
    pub fn tooltip(&mut self, input: &crate::input::UiInput, rect: UiRect, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let mut owned = Vec::with_capacity(lines.len());
        owned.push(String::new());
        owned.clear();
        for line in lines {
            owned.push(line.clone());
        }
        self.state.queue_tooltip(Tooltip {
            lines: owned,
            anchor: rect,
            pointer: input.point(),
        });
    }

    /// A horizontal rule.
    pub fn divider(&self, painter: &mut Painter<'_>, rect: UiRect) {
        painter.fill(rect, darken(self.theme.palette.border, 8));
        painter.fill(
            UiRect::new(rect.x, rect.y + 1, rect.w, 1),
            lighten(self.theme.palette.border, 30),
        );
    }

    /// A label on the left and a value on the right, on one row.
    pub fn stat_row(
        &self,
        painter: &mut Painter<'_>,
        rect: UiRect,
        label: &str,
        value: &str,
        role: TextRole,
    ) {
        let split = (rect.w as f32 * 0.55) as u32;
        let left = UiRect::new(rect.x, rect.y, split, rect.h);
        let right = UiRect::new(rect.x + split as i32, rect.y, rect.w - split, rect.h);
        self.label_role(painter, left, label, TextRole::Dim);
        self.value(painter, right, value, role);
    }

    /// A single row of a list: a background when hovered or selected.
    pub fn row(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        selected: bool,
    ) -> Response {
        let response = self.state.interact(id, rect, input);
        if selected {
            painter.fill(rect, self.theme.palette.accent);
        } else if response.hovered {
            painter.fill(rect, lighten(self.theme.palette.surface, 18));
        }
        response
    }

    /// A checkbox. Returns whether the value changed, and the new value.
    pub fn checkbox(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        label: &str,
        value: &mut bool,
    ) -> Response {
        let mut response = self.state.interact(id, rect, input);
        if response.clicked {
            *value = !*value;
            response.changed = true;
        }
        let box_size = rect.h.clamp(6, 10);
        let box_rect = UiRect::new(
            rect.x,
            rect.y + (rect.h as i32 - box_size as i32) / 2,
            box_size,
            box_size,
        );
        let style = if response.hovered {
            FrameStyle::flat(
                lighten(self.theme.palette.surface_raised, 16),
                self.theme.palette.accent,
            )
        } else {
            FrameStyle::flat(self.theme.palette.surface_raised, self.theme.palette.border)
        };
        painter.frame(&self.texture, &style, box_rect);
        if *value {
            // A hand-drawn tick rather than a glyph: a tick in the text font
            // would be a full-size character in a box a third its size.
            let inner = box_rect.inset(Insets::all(2));
            painter.line(
                inner.x,
                inner.y + inner.h as i32 / 2,
                inner.x + inner.w as i32 / 2,
                inner.bottom() - 1,
                self.theme.palette.accent,
            );
            painter.line(
                inner.x + inner.w as i32 / 2,
                inner.bottom() - 1,
                inner.right() - 1,
                inner.y,
                self.theme.palette.accent,
            );
        }
        let text_rect = UiRect::new(
            box_rect.right() + 4,
            rect.y,
            rect.w.saturating_sub(box_size + 4),
            rect.h,
        );
        let style = TextStyle::new(self.theme.palette.text);
        let (_, height) = self.font.measure(label, &style);
        self.font.draw_text(
            painter,
            UiRect::new(
                text_rect.x,
                text_rect.y + (text_rect.h as i32 - height as i32) / 2,
                text_rect.w,
                height,
            ),
            label,
            &style,
        );
        response
    }

    /// A `- value +` stepper, for a quantity.
    ///
    /// Returns whether the value changed. Holding a direction repeats, using the
    /// frame delta so the repeat rate is the same at 30 and 144 fps.
    // Eight arguments is one more than clippy's default. The alternative is a
    // `Stepper { .. }` builder for a widget with five parameters that are all
    // required and none of which are optional — which is more code at every call
    // site to satisfy a lint about a call site.
    #[allow(clippy::too_many_arguments)]
    pub fn stepper(
        &mut self,
        painter: &mut Painter<'_>,
        input: &crate::input::UiInput,
        rect: UiRect,
        id: Id,
        value: &mut i32,
        min: i32,
        max: i32,
    ) -> bool {
        let button_w = (rect.h).min(14);
        let mut layout = rect;
        let minus = layout.cut_left(button_w);
        let plus = layout.cut_right(button_w);
        let field = layout;

        let mut changed = false;
        let decrement = self.button(painter, input, minus, id.with("minus"), "-", *value > min);
        if decrement.clicked {
            *value = (*value - 1).max(min);
            changed = true;
        }
        let increment = self.button(painter, input, plus, id.with("plus"), "+", *value < max);
        if increment.clicked {
            *value = (*value + 1).min(max);
            changed = true;
        }

        painter.frame(&self.texture, &self.theme.sunken, field);
        let style = TextStyle::new(self.theme.palette.text).with_align(TextAlign::Center);
        let (_, height) = self.font.measure(&value.to_string(), &style);
        self.font.draw_text(
            painter,
            UiRect::new(
                field.x,
                field.y + (field.h as i32 - height as i32) / 2,
                field.w,
                height,
            ),
            &value.to_string(),
            &style,
        );
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::UiInputBuilder;
    use crate::testfont::test_font;

    fn ui() -> Ui {
        Ui::flat(test_font())
    }

    fn lit_pixels(fb: &noxel_render::framebuffer::Framebuffer) -> usize {
        fb.color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.02)
            .count()
    }

    #[test]
    fn a_button_click_reports_once_and_consumes_the_pointer() {
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(64, 32);
        let rect = UiRect::new(4, 4, 40, 14);
        let id = Id::new("test.button");

        let click = UiInputBuilder::new().at(10.0, 10.0).click().build();
        ui.begin(1.0 / 60.0, &click);
        {
            let mut painter = ui.painter(&mut fb);
            let response = ui.button(&mut painter, &click, rect, id, "Sell", true);
            assert!(response.pressed);
            assert!(!response.clicked);
        }
        assert!(
            ui.state.pointer_over_ui(),
            "a button must stop the click reaching the world"
        );

        let release = UiInputBuilder::new().at(10.0, 10.0).release().build();
        ui.begin(1.0 / 60.0, &release);
        {
            let mut painter = ui.painter(&mut fb);
            let response = ui.button(&mut painter, &release, rect, id, "Sell", true);
            assert!(response.clicked);
        }
    }

    #[test]
    fn a_disabled_button_never_activates_but_still_hovers() {
        // Hovering a disabled control is how a tooltip explains why it is
        // disabled; suppressing it makes the UI feel dead.
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(64, 32);
        let rect = UiRect::new(4, 4, 40, 14);
        let click = UiInputBuilder::new().at(10.0, 10.0).click().build();
        ui.begin(1.0 / 60.0, &click);
        let mut painter = ui.painter(&mut fb);
        let response = ui.button(&mut painter, &click, rect, Id::new("off"), "Sell", false);
        assert!(response.hovered);
        assert!(!response.pressed);
        assert!(!response.clicked);
    }

    #[test]
    fn a_button_draws_something() {
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(64, 32);
        let input = UiInputBuilder::new().at(-5.0, -5.0).outside().build();
        ui.begin(1.0 / 60.0, &input);
        {
            let mut painter = ui.painter(&mut fb);
            ui.button(
                &mut painter,
                &input,
                UiRect::new(2, 2, 50, 16),
                Id::new("b"),
                "OK",
                true,
            );
        }
        assert!(lit_pixels(&fb) > 100, "the button was not drawn");
    }

    #[test]
    fn a_bar_fills_in_whole_pixels_of_its_trough() {
        let ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(64, 16);
        let rect = UiRect::new(0, 0, 42, 7);
        {
            let mut painter = ui.painter(&mut fb);
            ui.bar(&mut painter, rect, 0.5, &ui.theme.bar);
        }
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        let inner_w = 40; // 42 minus a one-pixel border each side
        let expected = inner_w / 2;
        // The fill starts after the border and is exactly half the trough wide.
        assert_eq!(image.get(1 + expected - 1, 3), Some(ui.theme.bar.fill));
        assert_ne!(image.get(1 + expected, 3), Some(ui.theme.bar.fill));
    }

    #[test]
    fn an_empty_bar_draws_only_its_trough() {
        let ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(32, 16);
        {
            let mut painter = ui.painter(&mut fb);
            ui.bar(&mut painter, UiRect::new(0, 0, 30, 7), 0.0, &ui.theme.bar);
        }
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        // No pixel of the interior may be the fill colour. Asserting the
        // trough's own colour instead would be asserting a composite: the
        // framebuffer has no alpha channel, so a 240-alpha trough over black
        // resolves a shade darker than the authored byte, and a test pinned to
        // that byte would be measuring the blend, not the behaviour.
        let fill = ui.theme.bar.fill;
        for x in 1..29 {
            assert_ne!(
                image.get(x, 3),
                Some(fill),
                "an empty bar drew a fill at x={x}"
            );
        }
        // The trough itself was drawn: the interior is not the black background.
        assert_ne!(image.get(15, 3), Some(Color8::BLACK));
    }

    #[test]
    fn a_slot_centres_its_content_and_never_overflows() {
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(48, 48);
        let icon = Image::new(4, 4, Color8::new(255, 0, 0, 255));
        let rect = UiRect::new(4, 4, 20, 20);
        let input = UiInputBuilder::new().outside().build();
        ui.begin(1.0 / 60.0, &input);
        {
            let mut painter = ui.painter(&mut fb);
            ui.slot(
                &mut painter,
                &input,
                rect,
                Id::new("slot"),
                Some(Sprite::new(&icon, UiRect::new(0, 0, 4, 4))),
                SlotState::default(),
            );
        }
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        // 20 wide box, 4 wide icon: centred leaves 8 pixels of trough each side.
        assert_eq!(image.get(4 + 8, 4 + 8), Some(Color8::new(255, 0, 0, 255)));
        assert_ne!(
            image.get(5, 5),
            Some(Color8::new(255, 0, 0, 255)),
            "the icon must not fill the slot"
        );
    }

    #[test]
    fn a_checkbox_toggles_only_on_a_click() {
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(80, 20);
        let rect = UiRect::new(2, 2, 60, 14);
        let mut value = false;

        let hover = UiInputBuilder::new().at(6.0, 6.0).build();
        ui.begin(1.0 / 60.0, &hover);
        {
            let mut painter = ui.painter(&mut fb);
            let response = ui.checkbox(&mut painter, &hover, rect, Id::new("c"), "On", &mut value);
            assert!(response.hovered);
            assert!(!response.changed, "hovering must not toggle");
        }
        assert!(!value);

        let press = UiInputBuilder::new().at(6.0, 6.0).click().build();
        ui.begin(1.0 / 60.0, &press);
        {
            let mut painter = ui.painter(&mut fb);
            ui.checkbox(&mut painter, &press, rect, Id::new("c"), "On", &mut value);
        }
        let release = UiInputBuilder::new().at(6.0, 6.0).release().build();
        ui.begin(1.0 / 60.0, &release);
        {
            let mut painter = ui.painter(&mut fb);
            let response =
                ui.checkbox(&mut painter, &release, rect, Id::new("c"), "On", &mut value);
            assert!(response.changed);
        }
        assert!(value);
    }

    #[test]
    fn a_stepper_clamps_at_its_bounds() {
        let mut ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(80, 20);
        let rect = UiRect::new(0, 0, 60, 14);
        let mut value = 2;

        // The "+" side is the right third; a click there at the maximum is a
        // no-op rather than an overflow.
        let press = UiInputBuilder::new().at(52.0, 7.0).click().build();
        ui.begin(1.0 / 60.0, &press);
        {
            let mut painter = ui.painter(&mut fb);
            ui.stepper(&mut painter, &press, rect, Id::new("s"), &mut value, 0, 2);
        }
        let release = UiInputBuilder::new().at(52.0, 7.0).release().build();
        ui.begin(1.0 / 60.0, &release);
        {
            let mut painter = ui.painter(&mut fb);
            ui.stepper(&mut painter, &release, rect, Id::new("s"), &mut value, 0, 2);
        }
        assert_eq!(value, 2, "the stepper must clamp at its maximum");
    }

    #[test]
    fn a_divider_draws_two_toned_lines() {
        let ui = ui();
        let mut fb = noxel_render::framebuffer::Framebuffer::new(32, 8);
        {
            let mut painter = ui.painter(&mut fb);
            ui.divider(&mut painter, UiRect::new(0, 3, 32, 2));
        }
        let image = fb.resolve(&noxel_render::framebuffer::ResolveSettings::default());
        assert_eq!(image.get(16, 3), Some(darken(ui.theme.palette.border, 8)));
        assert_eq!(image.get(16, 4), Some(lighten(ui.theme.palette.border, 30)));
    }
}
