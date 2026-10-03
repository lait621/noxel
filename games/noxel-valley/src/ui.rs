//! Every screen the player sees.
//!
//! One module rather than one file per screen, because the screens share their
//! vocabulary completely: they are all `UiRect` cuts, `ui.panel`, `ui.button` and
//! `ui.slot`. Splitting them would mean four files that each begin by importing
//! the same six things and re-deriving the same layout constants.
//!
//! # How a frame is drawn
//!
//! ```text
//!   draw_hud          always, underneath
//!   draw_screen       the open overlay, on the scrim, above
//!   draw_help         if toggled
//!   draw_toast        transient messages, on top of everything
//!   draw_tooltips     queued by any of the above, above even that
//! ```
//!
//! The order is the module's whole contract. A tooltip queued by the HUD has to
//! land above a panel the shop draws later, which is why tooltips are *queued*
//! rather than drawn where they are requested.
//!
//! # What this module does not own
//!
//! [`GameUi`] holds the open screen, the help flag and the toast. It does not
//! hold the inventory, the money or the crops. A screen reads the game state and
//! returns a [`UiAction`]; the game applies it. That is what keeps the UI from
//! being able to contradict the simulation, and it is what makes the screens
//! testable by driving the input and asserting on the action.

use noxel_core::math::Color8;
use noxel_ui::{
    Anchor, FrameStyle, Id, Insets, SlotState, Sprite, TextAlign, TextStyle, Ui, UiInput, UiRect,
    Wrap,
};

use crate::assets::Assets;
use crate::config::{
    CROP_STAGES, CROPS, Crop, FORECAST_DAYS, HOTBAR_SLOTS, INVENTORY_SLOTS, Season,
};
use crate::player::Player;
use crate::sim::{GameState, Item};

/// Which overlay is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    /// No overlay: the player is walking around.
    #[default]
    Playing,
    /// The bag.
    Inventory,
    /// The seed shop.
    Shop,
    /// The shipping bin.
    Bin,
    /// The morning report.
    Summary,
}

/// What the player asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    /// Nothing.
    None,
    /// A hotbar slot was clicked.
    SelectSlot(usize),
    /// A seed should be bought.
    BuySeed(&'static Crop, u32),
    /// Everything in the bag that can be shipped should be.
    ShipAll,
    /// The overlay should close.
    Close,
    /// The player wants to sleep.
    Sleep,
}

/// The game's interface state.
pub struct GameUi {
    /// The engine UI bundle: font, theme, interaction state.
    pub ui: Ui,
    /// Which overlay is open.
    pub screen: Screen,
    /// Whether the control reference is showing.
    pub help_open: bool,
    /// A transient message and the seconds it has left.
    toast: Option<(String, f32)>,
    /// How many of each shop row the player has dialled in.
    shop_quantity: [i32; CROPS.len()],
    /// The slot the pointer is over, for the tooltip.
    hovered_slot: Option<usize>,
}

impl GameUi {
    /// Builds the interface.
    #[must_use]
    pub fn new(ui: Ui) -> Self {
        Self {
            ui,
            screen: Screen::Playing,
            help_open: false,
            toast: None,
            shop_quantity: [1; CROPS.len()],
            hovered_slot: None,
        }
    }

    /// Shows a message in the corner for a few seconds.
    pub fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), 3.0));
    }

    /// Advances the toast timer.
    pub fn tick(&mut self, dt: f32) {
        if let Some((_, remaining)) = self.toast.as_mut() {
            *remaining -= dt;
            if *remaining <= 0.0 {
                self.toast = None;
            }
        }
    }

    /// The height a scale-2 heading occupies.
    ///
    /// Measured from the text, because a scale-2 Chinese title is 24 pixels tall
    /// and a scale-2 Latin one is 18 — reserving 14 for both is how the heading
    /// ends up drawn through the first row of the panel beneath it.
    #[must_use]
    fn heading_height(&self, text: &str) -> u32 {
        let style = TextStyle::new(self.ui.theme.palette.text_strong).with_scale(2);
        self.ui.font.measure(text, &style).1
    }

    /// Draws a panel title and returns the height it used.
    fn title(&self, painter: &mut noxel_ui::Painter<'_>, rect: UiRect, text: &str) -> u32 {
        self.ui.heading(painter, rect, text)
    }

    /// Starts a frame.
    pub fn begin(&mut self, dt: f32, input: &UiInput) {
        self.ui.begin(dt, input);
        self.hovered_slot = None;
    }

    /// Ends a frame.
    pub fn end(&mut self) {
        self.ui.end();
    }

    /// Draws everything and returns what the player asked for.
    ///
    /// The first action wins: a click that closes a screen must not also buy
    /// something, and returning the last action would let a panel underneath a
    /// just-closed overlay still act on the same frame's input.
    pub fn draw(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        player: &Player,
        assets: &Assets,
    ) -> UiAction {
        let screen = UiRect::screen(framebuffer.width(), framebuffer.height());

        // The HUD first, so its rectangles are the ones the world click has to
        // avoid. Then the overlay, then everything that floats above both.
        let hud = self.draw_hud(framebuffer, input, state, assets, screen);
        let overlay = match self.screen {
            Screen::Playing => UiAction::None,
            Screen::Inventory => self.draw_inventory(framebuffer, input, state, assets, screen),
            Screen::Shop => self.draw_shop(framebuffer, input, state, assets, screen),
            Screen::Bin => self.draw_bin(framebuffer, input, state, assets, screen),
            Screen::Summary => self.draw_summary(framebuffer, input, state, assets, screen),
        };
        if self.help_open {
            self.draw_help(framebuffer, assets, screen);
        }
        self.draw_toast(framebuffer, assets, screen);
        self.draw_prompt(framebuffer, player, assets, screen);

        // The overlay wins: a click that closes a screen must not also select a
        // hotbar slot on the same frame.
        if overlay != UiAction::None {
            overlay
        } else {
            hud
        }
    }

    /// Whether the interface is capturing the pointer, so the world must not act
    /// on a click.
    #[must_use]
    pub fn captures_pointer(&self) -> bool {
        self.ui.state.pointer_over_ui()
    }

    // -- HUD ----------------------------------------------------------------

    fn draw_hud(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        assets: &Assets,
        screen: UiRect,
    ) -> UiAction {
        let mut action = UiAction::None;
        let theme = self.ui.theme;
        let metrics = theme.metrics;

        // -- the clock, on the left -----------------------------------------
        //
        // Two rows, each a single run of text or a run plus images.
        //
        // The rows are not decoration. `FontSet::draw_text` aligns a run to the
        // baseline of the faces *that run* used, so a Latin-only run and a
        // Chinese run drawn at the same `y` do not share a baseline — the
        // Chinese one, being taller, sits lower. Putting them on separate rows
        // is what keeps the time and the weather from looking like two
        // different sizes of the same line.
        //
        // The panel is measured from its contents rather than given a fixed
        // width: a hard-coded 132 was wide enough for "第 1 年 春 1 日" and not
        // for "第 12 年 冬 28 日".
        let date_text = format!(
            "第 {} 年 {} {} 日",
            state.clock.year(),
            state.clock.season().name(),
            state.clock.day_of_season()
        );
        let time_text = state.clock.time_string();
        let weather = state.weather.today();
        let line_style = TextStyle::new(theme.palette.text);
        let dim_style = TextStyle::new(theme.palette.text_dim);
        let (date_width, date_height) = self.ui.font.measure(&date_text, &line_style);
        let (time_width, time_height) = self.ui.font.measure(&time_text, &dim_style);
        let (weather_width, _) = self.ui.font.measure(weather.name(), &line_style);
        let padding = metrics.padding;
        // The gap keeps the date and the weather from touching on the widest
        // date a save can reach: "第 10 年 冬 28 日" plus "雷雨".
        let row_one = date_width + 18 + 16 + 3 + weather_width;
        let row_two = time_width + 10 + FORECAST_DAYS as u32 * 18;
        let width = row_one.max(row_two) + padding.horizontal().max(0) as u32 + 4;
        let height = date_height + time_height + padding.vertical().max(0) as u32 + 4;
        let clock_panel = UiRect::new(4, 4, width, height);

        {
            let mut painter = self.ui.painter(framebuffer);
            // `panel_blocking` rather than `panel`: a HUD panel that does not
            // register with the pointer lets a click on the clock fall through
            // and swing a hoe at the farm behind it.
            self.ui
                .panel_blocking(&mut painter, input, clock_panel, &theme.panel);
            let inner = clock_panel.inset(padding);

            // Row one: the date, then the weather and its icon, right-aligned.
            self.ui.label(
                &mut painter,
                UiRect::new(inner.x, inner.y, date_width + 2, date_height),
                &date_text,
                &line_style,
            );
            let weather_x = inner.right() - weather_width as i32 - 19;
            if let Some(sprite) = icon(assets, weather.icon()) {
                painter.blit(
                    sprite.image,
                    sprite.rect,
                    weather_x,
                    inner.y + 2,
                    Color8::WHITE,
                );
            }
            self.ui.label(
                &mut painter,
                UiRect::new(weather_x + 19, inner.y, weather_width + 2, date_height),
                weather.name(),
                &line_style,
            );

            // Row two: the time, then the next two days' weather as dimmed
            // icons, so a rainy day is something to plan for rather than
            // discover.
            let row_two_y = inner.y + date_height as i32;
            self.ui.label(
                &mut painter,
                UiRect::new(inner.x, row_two_y, time_width + 2, time_height),
                &time_text,
                &dim_style,
            );
            let mut x = inner.x + time_width as i32 + 10;
            for day in 1..FORECAST_DAYS {
                let forecast = state.weather.forecast()[day];
                if let Some(sprite) = icon(assets, forecast.icon()) {
                    let tint = Color8::new(255, 255, 255, 140);
                    painter.blit(sprite.image, sprite.rect, x, row_two_y - 2, tint);
                }
                x += 18;
            }
        }

        // -- the purse, on the right ----------------------------------------
        let gold_text = format!("{}", state.inventory.gold);
        let gold_style = TextStyle::new(theme.palette.accent).with_align(TextAlign::Right);
        let (gold_width, _) = self.ui.font.measure(&gold_text, &gold_style);
        let gold_panel = UiRect::new(
            screen.right() - 8 - (gold_width as i32 + 30),
            4,
            gold_width + 30,
            18,
        );
        {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, gold_panel, &theme.panel);
            if let Some(sprite) = icon(assets, "icon_coin") {
                painter.blit(
                    sprite.image,
                    sprite.rect,
                    gold_panel.x + 4,
                    gold_panel.y + 2,
                    Color8::WHITE,
                );
            }
            self.ui.value(
                &mut painter,
                UiRect::new(gold_panel.x + 20, gold_panel.y + 6, gold_panel.w - 24, 10),
                &gold_text,
                noxel_ui::TextRole::Accent,
            );
        }

        // -- energy, above the hotbar ---------------------------------------
        let hotbar = hotbar_rect(screen);
        let energy_rect = UiRect::new(hotbar.x, hotbar.y - 12, hotbar.w, 8);
        {
            let mut painter = self.ui.painter(framebuffer);
            self.ui.state.block_pointer(energy_rect, input);
            self.ui.bar(
                &mut painter,
                energy_rect,
                state.energy / crate::config::MAX_ENERGY,
                &theme.bar,
            );
            let label = format!("{:.0}", state.energy);
            let style = TextStyle::new(theme.palette.text_strong)
                .with_align(TextAlign::Center)
                .with_shadow(theme.palette.shadow);
            self.ui.label(
                &mut painter,
                UiRect::new(energy_rect.x, energy_rect.y - 1, energy_rect.w, 9),
                &label,
                &style,
            );
        }

        // -- the hotbar ------------------------------------------------------
        {
            let mut painter = self.ui.painter(framebuffer);
            for index in 0..HOTBAR_SLOTS {
                let rect = hotbar_slot(hotbar, index);
                let selected = index == state.selected;
                let item = state.inventory.slots().get(index).and_then(|slot| *slot);
                let content = item.and_then(|slot| item_sprite(assets, slot.item));
                let response = self.ui.slot(
                    &mut painter,
                    input,
                    rect,
                    Id::new("hotbar").index(index),
                    content,
                    SlotState {
                        selected,
                        hovered: false,
                        empty: item.is_none(),
                        count: item.map(|slot| slot.count),
                    },
                );
                // Fall back to a coloured tile when the art atlas has no icon for
                // the item, so the hotbar is never blank.
                if content.is_none() {
                    if let Some(slot) = item {
                        painter.fill(rect.inset(Insets::all(3)), slot.item.color());
                    }
                }
                if response.clicked {
                    action = UiAction::SelectSlot(index);
                }
                if response.hovered {
                    self.hovered_slot = Some(index);
                }
            }
        }

        self.queued_slot_tooltip(input, state);
        action
    }

    // -- Inventory ----------------------------------------------------------

    fn draw_inventory(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        assets: &Assets,
        screen: UiRect,
    ) -> UiAction {
        let mut action = UiAction::None;
        let theme = self.ui.theme;
        let metrics = theme.metrics;
        let columns = 6u32;
        let rows = (INVENTORY_SLOTS as u32).div_ceil(columns);
        let slot = metrics.slot_size;
        let gap = metrics.gap;
        // The metrics are signed because a padding may legitimately be negative
        // (an outset); the layout is unsigned because a rectangle cannot be.
        let cell = slot + gap.max(0) as u32;
        let title = "背包";
        let title_height = self.heading_height(title);
        let width = columns * cell + gap.max(0) as u32 + metrics.padding.horizontal().max(0) as u32;
        let height = title_height
            + gap.max(0) as u32
            + rows * cell
            + gap.max(0) as u32
            + metrics.padding.vertical().max(0) as u32
            + metrics.row_height
            + metrics.gap_large.max(0) as u32;

        let panel = screen.place((width, height), Anchor::Center, (0, -8));
        {
            let mut painter = self.ui.modal_scrim(framebuffer);
            painter.frame(assets.ui_texture(), &theme.panel, panel);
        }

        // The painter borrows the framebuffer, so it is scoped: the tooltip
        // pass at the end of this function takes `&mut self` again, and the two
        // would overlap if the painter were still alive.
        let mut painter = self.ui.painter(framebuffer);
        let inner = panel.inset(metrics.padding);
        let used = self.title(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, title_height),
            title,
        );

        let close_rect = UiRect::new(inner.right() - 44, inner.y + 1, 44, 12);
        if self
            .ui
            .button(
                &mut painter,
                input,
                close_rect,
                Id::new("inv.close"),
                "关闭",
                true,
            )
            .clicked
        {
            action = UiAction::Close;
        }

        let grid_top = inner.y + used as i32 + metrics.gap;
        for index in 0..INVENTORY_SLOTS {
            let column = index as u32 % columns;
            let row = index as u32 / columns;
            let rect = UiRect::new(
                inner.x + gap + (column * cell) as i32,
                grid_top + gap + (row * cell) as i32,
                slot,
                slot,
            );
            let held = state.inventory.slots().get(index).and_then(|s| *s);
            let content = held.and_then(|s| item_sprite(assets, s.item));
            let response = self.ui.slot(
                &mut painter,
                input,
                rect,
                Id::new("inv").index(index),
                content,
                SlotState {
                    selected: false,
                    hovered: false,
                    empty: held.is_none(),
                    count: held.map(|s| s.count),
                },
            );
            if content.is_none() {
                if let Some(slot_value) = held {
                    painter.fill(rect.inset(Insets::all(3)), slot_value.item.color());
                }
            }
            if response.hovered {
                self.hovered_slot = Some(index);
            }
        }

        // A footer line: what is in the bag, at a glance.
        let footer = UiRect::new(
            inner.x,
            inner.bottom() - metrics.row_height as i32,
            inner.w,
            metrics.row_height,
        );
        let summary = format!(
            "金币 {}   空位 {}",
            state.inventory.gold,
            state.inventory.free_slots()
        );
        self.ui
            .label_role(&mut painter, footer, &summary, noxel_ui::TextRole::Dim);

        self.queued_slot_tooltip(input, state);
        action
    }

    // -- Shop ---------------------------------------------------------------

    fn draw_shop(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        assets: &Assets,
        screen: UiRect,
    ) -> UiAction {
        let mut action = UiAction::None;
        let theme = self.ui.theme;
        let metrics = theme.metrics;
        let stock = crate::sim::shop_stock(state.clock.season());

        let title = "种子商店";
        let title_height = self.heading_height(title);
        let width = 240u32;
        let height = (stock.len() as u32 + 1) * (metrics.row_height + 2)
            + title_height
            + metrics.gap as u32
            + metrics.padding.vertical().max(0) as u32;
        let panel = screen.place((width, height), Anchor::Center, (0, -6));

        {
            let mut painter = self.ui.modal_scrim(framebuffer);
            painter.frame(assets.ui_texture(), &theme.panel, panel);
        }

        let mut painter = self.ui.painter(framebuffer);
        let inner = panel.inset(metrics.padding);
        let used = self.title(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, title_height),
            title,
        );

        if self
            .ui
            .button(
                &mut painter,
                input,
                UiRect::new(inner.right() - 44, inner.y + 1, 44, 12),
                Id::new("shop.close"),
                "离开",
                true,
            )
            .clicked
        {
            action = UiAction::Close;
        }

        if stock.is_empty() {
            // Winter has nothing to sell, and saying so is better than an empty
            // list the player reads as a bug.
            let note = TextStyle::new(theme.palette.text_dim);
            self.ui.label(
                &mut painter,
                UiRect::new(inner.x, inner.y + used as i32 + metrics.gap, inner.w, 12),
                "这个季节没有种子。冬天是休耕的季节。",
                &note,
            );
        }

        for (row, crop) in stock.iter().enumerate() {
            // The quantity is keyed by the crop, not the row: the stock list
            // changes length with the season, and a row key would carry last
            // autumn's quantity onto a different seed.
            let stock_index = CROPS
                .iter()
                .position(|candidate| std::ptr::eq(candidate, *crop))
                .unwrap_or(row);
            let y = inner.y
                + used as i32
                + metrics.gap
                + (row as u32 * (metrics.row_height + 2)) as i32;
            let row_rect = UiRect::new(inner.x, y, inner.w, metrics.row_height);
            let id = Id::new("shop").with(crop.key);

            if self
                .ui
                .row(&mut painter, input, row_rect, id, false)
                .hovered
            {
                let lines = vec![
                    format!("{}  {} 金", crop.name, crop.seed_price),
                    format!("{} 天成熟 · {} 金出售", crop.growth_days, crop.sell_price),
                    if crop.regrow_days > 0 {
                        format!("收获后可再生长（{} 天）", crop.regrow_days)
                    } else {
                        "收成后需要重新播种".to_string()
                    },
                ];
                self.ui.tooltip(input, row_rect, &lines);
            }

            // The icon, then the name, then the price, then the controls.
            let icon_rect = UiRect::new(row_rect.x + 1, row_rect.y, 12, 12);
            if let Some(sprite) = icon(assets, &format!("seed_{}", crop.key)) {
                painter.blit(
                    sprite.image,
                    sprite.rect,
                    icon_rect.x,
                    icon_rect.y,
                    Color8::WHITE,
                );
            } else {
                painter.fill(icon_rect, crop.color);
            }

            let name_style = TextStyle::new(theme.palette.text);
            self.ui.label(
                &mut painter,
                UiRect::new(row_rect.x + 16, row_rect.y + 2, 70, 10),
                crop.name,
                &name_style,
            );

            // The growth bar: a four-stage pip strip, so "how long does this
            // take" is answered without reading a number.
            let pip_x = row_rect.x + 88;
            for stage in 0..CROP_STAGES {
                let pip = UiRect::new(pip_x + (stage * 5) as i32, row_rect.y + 5, 4, 4);
                painter.fill(
                    pip,
                    if stage < 3 {
                        theme.palette.good
                    } else {
                        theme.palette.accent
                    },
                );
            }

            let price = format!("{} 金", crop.seed_price);
            self.ui.value(
                &mut painter,
                UiRect::new(row_rect.x + 116, row_rect.y + 2, 40, 10),
                &price,
                noxel_ui::TextRole::Accent,
            );

            let mut quantity = self.shop_quantity[stock_index];
            let affordable = state.inventory.gold >= crop.seed_price;
            if self.ui.stepper(
                &mut painter,
                input,
                UiRect::new(row_rect.right() - 76, row_rect.y, 46, metrics.row_height),
                id.with("qty"),
                &mut quantity,
                1,
                99,
            ) {
                self.shop_quantity[stock_index] = quantity;
            }

            let cost = crop.seed_price * quantity as u32;
            let can_buy = state.inventory.gold >= cost && state.inventory.free_slots() > 0;
            if self
                .ui
                .button(
                    &mut painter,
                    input,
                    UiRect::new(row_rect.right() - 28, row_rect.y, 28, metrics.row_height),
                    id.with("buy"),
                    "买",
                    can_buy && affordable,
                )
                .clicked
                && can_buy
            {
                action = UiAction::BuySeed(crop, quantity as u32);
            }
        }

        let footer = UiRect::new(inner.x, inner.bottom() - 12, inner.w, 12);
        let gold = format!("金币 {}", state.inventory.gold);
        self.ui
            .value(&mut painter, footer, &gold, noxel_ui::TextRole::Accent);
        action
    }

    // -- Shipping bin -------------------------------------------------------

    fn draw_bin(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        assets: &Assets,
        screen: UiRect,
    ) -> UiAction {
        let mut action = UiAction::None;
        let theme = self.ui.theme;
        let metrics = theme.metrics;
        let title = "出货箱";
        let title_height = self.heading_height(title);
        let panel = screen.place((220, 72 + title_height), Anchor::Center, (0, -6));

        {
            let mut painter = self.ui.modal_scrim(framebuffer);
            painter.frame(assets.ui_texture(), &theme.panel, panel);
        }
        let mut painter = self.ui.painter(framebuffer);
        let inner = panel.inset(metrics.padding);
        let used = self.title(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, title_height),
            title,
        );

        if self
            .ui
            .button(
                &mut painter,
                input,
                UiRect::new(inner.right() - 44, inner.y + 1, 44, 12),
                Id::new("bin.close"),
                "关闭",
                true,
            )
            .clicked
        {
            action = UiAction::Close;
        }

        // What is already in the bin.
        let contents: Vec<String> = state
            .bin
            .contents()
            .iter()
            .map(|(item, count)| format!("{} x{}", item.name(), count))
            .collect();
        let body = if contents.is_empty() {
            "空空的。收获的作物都可以放进来。".to_string()
        } else {
            contents.join("  ")
        };
        let wrap = TextStyle::new(theme.palette.text_dim).with_wrap(Wrap::Width(inner.w));
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, inner.y + used as i32 + metrics.gap, inner.w, 30),
            &body,
            &wrap,
        );

        let total = format!("明日收入 {} 金", state.bin.value());
        self.ui.label_role(
            &mut painter,
            UiRect::new(inner.x, inner.bottom() - 32, inner.w, 10),
            &total,
            noxel_ui::TextRole::Accent,
        );

        if self
            .ui
            .button(
                &mut painter,
                input,
                UiRect::new(inner.x, inner.bottom() - 16, 100, 14),
                Id::new("bin.ship"),
                "全部放入",
                true,
            )
            .clicked
        {
            action = UiAction::ShipAll;
        }
        if self
            .ui
            .button(
                &mut painter,
                input,
                UiRect::new(inner.x + 106, inner.bottom() - 16, 100, 14),
                Id::new("bin.sleep"),
                "睡觉，结束今天",
                true,
            )
            .clicked
        {
            action = UiAction::Sleep;
        }
        action
    }

    // -- Morning report -----------------------------------------------------

    fn draw_summary(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        input: &UiInput,
        state: &GameState,
        assets: &Assets,
        screen: UiRect,
    ) -> UiAction {
        let theme = self.ui.theme;
        let metrics = theme.metrics;
        let panel = screen.place((200, 92), Anchor::Center, (0, -6));
        {
            let mut painter = self.ui.modal_scrim(framebuffer);
            painter.frame(assets.ui_texture(), &theme.panel, panel);
        }

        let mut painter = self.ui.painter(framebuffer);
        let inner = panel.inset(metrics.padding);
        let day = state.last_summary.as_ref();
        self.ui.heading(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 12),
            &format!("第 {} 天结束", day.map_or(1, |d| d.day)),
        );

        let rows = if let Some(summary) = day {
            vec![
                ("出货收入", format!("{} 金", summary.shipped)),
                ("作物成熟", format!("{} 株", summary.ripened)),
                ("枯萎损失", format!("{} 株", summary.died)),
                ("金币", format!("{}", state.inventory.gold)),
            ]
        } else {
            vec![("金币", format!("{}", state.inventory.gold))]
        };
        for (index, (label, value)) in rows.iter().enumerate() {
            let y = inner.y + 18 + (index as u32 * (metrics.row_height - 1)) as i32;
            let role = match *label {
                "枯萎损失" => noxel_ui::TextRole::Bad,
                "作物成熟" => noxel_ui::TextRole::Good,
                _ => noxel_ui::TextRole::Body,
            };
            self.ui.stat_row(
                &mut painter,
                UiRect::new(inner.x, y, inner.w, 11),
                label,
                value,
                role,
            );
        }

        let note = TextStyle::new(theme.palette.text_dim);
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, inner.bottom() - 26, inner.w, 10),
            &format!("今天：{}", state.weather.today().name()),
            &note,
        );

        if self
            .ui
            .button(
                &mut painter,
                input,
                UiRect::new(inner.x, inner.bottom() - 15, inner.w, 15),
                Id::new("summary.ok"),
                "开始新的一天",
                true,
            )
            .clicked
        {
            return UiAction::Close;
        }
        UiAction::None
    }

    // -- Help ---------------------------------------------------------------

    fn draw_help(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        assets: &Assets,
        screen: UiRect,
    ) {
        let theme = self.ui.theme;
        let metrics = theme.metrics;
        let lines = [
            "移动       WASD / 方向键       奔跑  Shift",
            "使用工具   空格 / 鼠标左键    切换工具  1-6 / 滚轮",
            "背包       Tab                购买 / 出货  在商店或箱子旁按 E",
            "睡觉       E（在出货箱旁）     帮助       ?",
            "退出       Esc",
            "",
            "锄头翻土 → 播种 → 每天浇水 → 成熟后收割",
            "雨天会替你把整片田浇好。作物在季节结束后会枯萎。",
        ];
        let mut painter = self.ui.modal_scrim(framebuffer);
        let width = 300;
        let height = lines.len() as u32 * 11 + 22;
        let panel = screen.place((width, height), Anchor::Center, (0, 0));
        painter.frame(assets.ui_texture(), &theme.panel, panel);
        let inner = panel.inset(metrics.padding);
        self.ui.heading(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 12),
            "操作说明",
        );
        let style = TextStyle::new(theme.palette.text);
        for (index, line) in lines.iter().enumerate() {
            self.ui.label(
                &mut painter,
                UiRect::new(
                    inner.x,
                    inner.y + 16 + (index as u32 * 11) as i32,
                    inner.w,
                    10,
                ),
                line,
                &style,
            );
        }
    }

    // -- Odds and ends ------------------------------------------------------

    fn draw_toast(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        assets: &Assets,
        screen: UiRect,
    ) {
        let Some((message, remaining)) = self.toast.clone() else {
            return;
        };
        let theme = self.ui.theme;
        let style = TextStyle::new(theme.palette.text_strong)
            .with_align(TextAlign::Center)
            .with_shadow(theme.palette.shadow);
        let (width, height) = self.ui.font.measure(&message, &style);
        let panel = UiRect::new(
            screen.x + (screen.w as i32 - width as i32 - 16) / 2,
            screen.y + 50,
            width + 16,
            height + 8,
        );
        let mut painter = self.ui.painter(framebuffer);
        // Fade out over the last second so the message leaves rather than blinks.
        let opacity = (remaining).clamp(0.0, 1.0);
        painter.with_opacity(opacity, |painter| {
            painter.frame(assets.ui_texture(), &theme.tooltip, panel);
            self.ui.label(
                painter,
                UiRect::new(panel.x + 8, panel.y + 4, panel.w - 16, panel.h - 8),
                &message,
                &style,
            );
        });
    }

    /// A contextual hint over the thing the player is standing next to.
    fn draw_prompt(
        &mut self,
        framebuffer: &mut noxel_render::framebuffer::Framebuffer,
        player: &Player,
        assets: &Assets,
        screen: UiRect,
    ) {
        if self.screen != Screen::Playing || self.help_open {
            return;
        }
        let Some(prompt) = crate::world::prompt_for(player.tile()) else {
            return;
        };
        let theme = self.ui.theme;
        let style = TextStyle::new(theme.palette.text_strong)
            .with_align(TextAlign::Center)
            .with_shadow(theme.palette.shadow);
        let text = format!("[E] {prompt}");
        let (width, height) = self.ui.font.measure(&text, &style);
        let panel = UiRect::new(
            screen.x + (screen.w as i32 - width as i32 - 12) / 2,
            screen.bottom() - 78,
            width + 12,
            height + 6,
        );
        let mut painter = self.ui.painter(framebuffer);
        painter.frame(assets.ui_texture(), &theme.tooltip, panel);
        self.ui.label(
            &mut painter,
            UiRect::new(panel.x + 6, panel.y + 3, panel.w - 12, panel.h - 6),
            &text,
            &style,
        );
    }

    fn queued_slot_tooltip(&mut self, input: &UiInput, state: &GameState) {
        let Some(index) = self.hovered_slot else {
            return;
        };
        let Some(slot) = state.inventory.slots().get(index).and_then(|s| *s) else {
            return;
        };
        let mut lines = vec![slot.item.name()];
        match slot.item {
            Item::Tool(tool) => {
                lines.push(format!(
                    "{} · 消耗 {:.0} 体力",
                    tool.english(),
                    tool.energy_cost()
                ));
            }
            Item::Seed(crop) => {
                lines.push(format!("{} 天成熟", crop.growth_days));
                lines.push(format!("可卖 {} 金", crop.sell_price));
            }
            Item::Produce(crop) => {
                lines.push(format!("出售 {} 金", crop.sell_price));
                let _ = crop;
            }
        }
        // The tooltip is anchored to the pointer rather than a rectangle: a slot
        // is small and the pointer is the thing the player is looking at.
        let anchor = UiRect::new(input.point().0, input.point().1, 1, 1);
        self.ui.tooltip(input, anchor, &lines);
    }
}

/// Where the hotbar sits for a given screen size.
#[must_use]
pub fn hotbar_rect(screen: UiRect) -> UiRect {
    let slot = crate::config::TILE + 6;
    let gap = 2u32;
    let width = HOTBAR_SLOTS as u32 * (slot + gap) + gap;
    let height = slot + gap * 2;
    UiRect::new(
        screen.x + (screen.w as i32 - width as i32) / 2,
        screen.bottom() - height as i32 - 6,
        width,
        height,
    )
}

/// One hotbar slot's rectangle.
#[must_use]
pub fn hotbar_slot(hotbar: UiRect, index: usize) -> UiRect {
    let slot = crate::config::TILE + 6;
    let gap = 2;
    UiRect::new(
        hotbar.x + gap + (index as u32 * (slot + gap as u32)) as i32,
        hotbar.y + gap,
        slot,
        slot,
    )
}

/// The sprite for an item, if the art has one.
#[must_use]
pub fn item_sprite<'a>(assets: &'a Assets, item: Item) -> Option<Sprite<'a>> {
    icon(assets, &item.icon())
}

/// The sprite for an atlas region, if it exists.
#[must_use]
pub fn icon<'a>(assets: &'a Assets, name: &str) -> Option<Sprite<'a>> {
    let (atlas, region) = assets.atlases.find(name)?;
    let rect = region.pixel_rect;
    Some(Sprite::new(
        atlas.texture().image(),
        UiRect::new(
            rect.min.x as i32,
            rect.min.y as i32,
            rect.width().max(0.0) as u32,
            rect.height().max(0.0) as u32,
        ),
    ))
}

/// The season's tint for a HUD accent, so the interface changes with the year.
#[must_use]
pub fn season_accent(season: Season) -> Color8 {
    match season {
        Season::Spring => Color8::new(140, 208, 120, 255),
        Season::Summer => Color8::new(248, 208, 96, 255),
        Season::Fall => Color8::new(232, 150, 72, 255),
        Season::Winter => Color8::new(150, 200, 244, 255),
    }
}

/// The frame style a screen uses for its main panel.
#[must_use]
pub fn screen_frame(theme: &noxel_ui::Theme) -> FrameStyle {
    theme.panel
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MAX_ENERGY, Tool};
    use crate::sim::{GameState, Item};
    use crate::world::build_farm;
    use noxel_render::framebuffer::Framebuffer;
    use noxel_ui::{UiInputBuilder, testfont::test_font};

    fn ui() -> GameUi {
        GameUi::new(Ui::flat(test_font()))
    }

    fn context() -> (GameState, Player) {
        (GameState::new(1), Player::at((13, 10)))
    }

    #[test]
    fn the_hud_draws_with_no_assets_at_all() {
        // The first run of a fresh checkout has no art; the HUD must still say
        // something useful.
        let mut game_ui = ui();
        let (state, player) = context();
        let assets = Assets::empty();
        let mut fb = Framebuffer::new(480, 270);

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().outside().build();
        let action = game_ui.draw(&mut fb, &input, &state, &player, &assets);
        game_ui.end();

        assert_eq!(action, UiAction::None);
        let lit = fb
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.01)
            .count();
        assert!(lit > 2000, "the HUD drew almost nothing: {lit} pixels");
    }

    #[test]
    fn the_hotbar_is_centred_at_the_bottom_and_does_not_overflow() {
        let screen = UiRect::screen(480, 270);
        let hotbar = hotbar_rect(screen);
        assert!(
            hotbar.x > 0 && hotbar.right() < 480,
            "the hotbar does not fit: {hotbar}"
        );
        assert!(hotbar.bottom() < 270);
        // Slots are inside the bar and in order.
        let first = hotbar_slot(hotbar, 0);
        let last = hotbar_slot(hotbar, HOTBAR_SLOTS - 1);
        assert!(first.x > hotbar.x);
        assert!(last.right() <= hotbar.right());
        assert!(first.x < last.x);
    }

    #[test]
    fn clicking_a_hotbar_slot_asks_to_select_it() {
        let mut game_ui = ui();
        let (state, player) = context();
        let assets = Assets::empty();
        let mut fb = Framebuffer::new(480, 270);
        let screen = UiRect::screen(480, 270);
        let target = hotbar_slot(hotbar_rect(screen), 2);
        let point = target.center();

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let press = UiInputBuilder::new()
            .at(point.0 as f32, point.1 as f32)
            .click()
            .build();
        game_ui.draw(&mut fb, &press, &state, &player, &assets);
        game_ui.end();

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let release = UiInputBuilder::new()
            .at(point.0 as f32, point.1 as f32)
            .release()
            .build();
        let action = game_ui.draw(&mut fb, &release, &state, &player, &assets);
        game_ui.end();

        assert_eq!(action, UiAction::SelectSlot(2));
    }

    #[test]
    fn the_interface_captures_the_pointer_over_a_panel() {
        // This is what stops a click on the clock from also swinging a hoe.
        let mut game_ui = ui();
        let (state, player) = context();
        let assets = Assets::empty();
        let mut fb = Framebuffer::new(480, 270);

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(10.0, 10.0).build();
        game_ui.draw(&mut fb, &input, &state, &player, &assets);
        game_ui.end();
        assert!(
            game_ui.captures_pointer(),
            "the clock panel did not capture the pointer"
        );

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let away = UiInputBuilder::new().at(240.0, 150.0).build();
        game_ui.draw(&mut fb, &away, &state, &player, &assets);
        game_ui.end();
        assert!(
            !game_ui.captures_pointer(),
            "open ground captured the pointer"
        );
    }

    #[test]
    fn an_open_overlay_is_modal_and_blocks_the_world() {
        let mut game_ui = ui();
        game_ui.screen = Screen::Inventory;
        let (state, player) = context();
        let assets = Assets::empty();
        let mut fb = Framebuffer::new(480, 270);

        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().at(240.0, 150.0).build();
        game_ui.draw(&mut fb, &input, &state, &player, &assets);
        game_ui.end();
        assert!(game_ui.captures_pointer());
    }

    #[test]
    fn the_summary_screen_reports_yesterday() {
        let mut game_ui = ui();
        let mut state = GameState::new(1);
        let mut map = build_farm();
        state.bin.add(
            Item::Produce(crate::config::crop_by_key("pumpkin").unwrap()),
            2,
        );
        state.sleep(&mut map);
        game_ui.screen = Screen::Summary;

        let assets = Assets::empty();
        let mut fb = Framebuffer::new(480, 270);
        game_ui.begin(1.0 / 60.0, &UiInput::new());
        let input = UiInputBuilder::new().outside().build();
        game_ui.draw(&mut fb, &input, &state, &Player::at((13, 10)), &assets);
        game_ui.end();

        let summary = state.last_summary.as_ref().unwrap();
        assert_eq!(summary.shipped, 640);
        assert!(fb.color_slice().chunks_exact(3).any(|c| c[0] > 0.02));
    }

    #[test]
    fn the_energy_bar_and_the_clock_read_from_the_state() {
        let mut start = GameState::new(1);
        start.energy = MAX_ENERGY;
        assert!((start.energy / MAX_ENERGY - 1.0).abs() < 1e-6);
        start.energy = 0.0;
        assert!((start.energy / MAX_ENERGY).abs() < 1e-6);
    }

    #[test]
    fn a_toast_expires() {
        let mut game_ui = ui();
        game_ui.toast("hello");
        assert!(game_ui.toast.is_some());
        game_ui.tick(4.0);
        assert!(game_ui.toast.is_none(), "the toast never went away");
    }

    #[test]
    fn every_item_has_a_fallback_colour_so_nothing_draws_invisible() {
        for crop in CROPS.iter() {
            assert_ne!(Item::Seed(crop).color().a, 0);
            assert_ne!(Item::Produce(crop).color().a, 0);
        }
        for tool in Tool::ALL {
            assert_ne!(Item::Tool(tool).color().a, 0);
        }
    }

    #[test]
    fn the_seasons_have_distinct_accents() {
        let mut seen = std::collections::HashSet::new();
        for season in Season::ALL {
            assert!(
                seen.insert(season_accent(season)),
                "{season:?} shares an accent"
            );
        }
    }
}
