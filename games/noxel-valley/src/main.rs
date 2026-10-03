//! # Noxel Valley
//!
//! A playable top-down farming game, and the reference project for the engine's
//! UI layer.
//!
//! ```text
//! cargo run -p noxel-valley --features window -- --window
//! cargo run -p noxel-valley -- --frames 600 --dump frames
//! ```
//!
//! ## How a frame is put together
//!
//! The game is one `Plugin`. That is the engine's unit of composition, and it
//! buys the whole frame loop for free:
//!
//! ```text
//!   App::step(dt)
//!     fixed_update    ValleyPlugin::update
//!                       player movement, tool use, the clock, the day rollover
//!     frame_update    camera follows app.context.focus, which update() set
//!     render          one mesh per atlas, no culling needed at this size
//!     draw_overlays   ValleyPlugin::draw  <- the entire UI, still in linear space
//!     resolve         linear HDR -> sRGB, once
//! ```
//!
//! Drawing the UI in `draw` rather than after `resolve` is the important part. It
//! means the interface is composited in the same linear space as the world, one
//! tone curve applies to both, and a colour specified as `#FF0000` in the theme
//! resolves to exactly `#FF0000` on screen.
//!
//! ## Why the game owns its own input
//!
//! The window host produces `noxel_window::Input` with the cursor already mapped
//! into framebuffer pixels. The game copies it into a `UiInput` before stepping,
//! rather than pushing it through `App::input_mut()`, because the UI needs the
//! pointer and `InputState` is keyboard-only. One conversion, in one place.

use std::cell::RefCell;
use std::rc::Rc;

use noxel_app::{App, AppConfig, Plugin};
use noxel_core::math::{Color, Color8, Vec3};
use noxel_render::framebuffer::Framebuffer;
use noxel_render::material::{AlphaMode, Material};
use noxel_render::renderer::ShadingMode;
use noxel_render::scene::{InstanceFlags, InstanceHandle};
use noxel_ui::{Ui, UiInput};

use noxel_valley::assets::Assets;
use noxel_valley::config::{
    FARM_HEIGHT, FARM_WIDTH, INTERNAL_HEIGHT, INTERNAL_WIDTH, ORTHO_HEIGHT,
};
use noxel_valley::player::Player;
use noxel_valley::sim::{GameState, Item};
use noxel_valley::ui::{GameUi, Screen, UiAction};
use noxel_valley::world::FarmMap;
use noxel_valley::{assets, config, skin, world};

/// The farm, the player, the clock and the interface.
struct Valley {
    map: FarmMap,
    player: Player,
    state: GameState,
    game_ui: GameUi,
    /// The frame's input, in framebuffer pixels.
    input: UiInput,
    /// The map revision the meshes were built from.
    built_revision: u64,
    /// Multiplies the clock for `--fast`.
    fast: bool,
    /// The three materials, so the time of day can dim them all at once.
    ground_material: noxel_render::material::MaterialHandle,
    crop_material: noxel_render::material::MaterialHandle,
    prop_material: noxel_render::material::MaterialHandle,
    /// The single instance each mesh is drawn through.
    ground_instance: Option<InstanceHandle>,
    crop_instance: Option<InstanceHandle>,
    prop_instance: Option<InstanceHandle>,
    /// The character material and the player's current animation.
    character_material: noxel_render::material::MaterialHandle,
    player_instance: Option<InstanceHandle>,
    player_sprite: String,
    /// A villager wandering the plaza, so the farm is not deserted.
    villager: Villager,
    villager_instance: Option<InstanceHandle>,
    villager_sprite: String,
    /// Set when the player asks to sleep early.
    sleep_requested: bool,
    /// The most recent tool use, for the log and for tests.
    last_action: Option<String>,
}

impl Valley {
    /// Builds the farm, the player and the interface.
    fn new(app: &mut App, seed: u32, assets: &Assets, fast: bool) -> Self {
        let map = world::build_farm();
        let player = Player::at((13, 10));
        let ui = Ui::new(
            assets.font.clone().unwrap_or_else(default_font),
            skin::build(assets.atlases.ui.as_ref()),
            assets.ui_texture().clone(),
        );

        // One material per atlas, because a material carries one texture. The
        // ground is opaque and the sprites are cutouts: a cutout writes depth,
        // which is what lets the depth buffer sort a tree in front of the player
        // standing below it.
        // The texture handle is resolved *before* the scene is borrowed to add
        // the material: `add_material` takes `&mut scene`, and passing
        // `texture_handle(app, ..)` as an argument would borrow the app twice.
        let terrain_texture = texture_handle(app, assets, "terrain");
        let crop_texture = texture_handle(app, assets, "crops");
        let prop_texture = texture_handle(app, assets, "props");
        let ground_material = app.scene_mut().add_material(
            Material::sprite("valley.ground", terrain_texture).without_shadow_casting(),
        );
        let crop_material = app.scene_mut().add_material(
            Material::sprite("valley.crops", crop_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );
        let prop_material = app.scene_mut().add_material(
            Material::sprite("valley.props", prop_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );
        let character_texture = texture_handle(app, assets, "characters");
        let character_material = app.scene_mut().add_material(
            Material::sprite("valley.characters", character_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );

        setup_camera(app);
        app.scene_mut().clear_lights();
        app.scene_mut().background = Color::rgb(0.05, 0.06, 0.09);

        let mut valley = Self {
            map,
            player,
            state: GameState::new(seed),
            game_ui: GameUi::new(ui),
            input: UiInput::new(),
            built_revision: 0,
            fast,
            ground_material,
            crop_material,
            prop_material,
            ground_instance: None,
            crop_instance: None,
            prop_instance: None,
            character_material,
            player_instance: None,
            player_sprite: String::new(),
            villager: Villager::default(),
            villager_instance: None,
            villager_sprite: String::new(),
            sleep_requested: false,
            last_action: None,
        };
        valley.rebuild(app, assets);
        valley
    }

    /// Rebuilds the farm meshes if the map changed.
    fn rebuild(&mut self, app: &mut App, assets: &Assets) {
        if self.built_revision == self.map.revision() {
            return;
        }
        let (Some(terrain), Some(crops), Some(props)) = (
            assets.atlases.terrain.as_ref(),
            assets.atlases.crops.as_ref(),
            assets.atlases.props.as_ref(),
        ) else {
            // No art: the ground is a flat colour and there is nothing to build.
            self.built_revision = self.map.revision();
            return;
        };
        let meshes = world::build_meshes(&self.map, terrain, crops, props);

        for (mesh, material, slot) in [
            (meshes.ground, self.ground_material, 0u8),
            (meshes.crops, self.crop_material, 1),
            (meshes.props, self.prop_material, 2),
        ] {
            if mesh.is_empty() {
                continue;
            }
            let handle = app.scene_mut().add_mesh(mesh);
            let instance = app.scene_mut().spawn(
                format!("farm.{slot}"),
                handle,
                material,
                noxel_core::math::Transform::IDENTITY,
            );
            // Ground and crops are never occluders: nothing should fade because
            // the camera is behind a blade of grass.
            app.scene_mut().set_flags(
                instance,
                InstanceFlags {
                    occluder: false,
                    cast_shadow: false,
                    ..InstanceFlags::default()
                },
            );
            match slot {
                0 => {
                    self.release(app, self.ground_instance);
                    self.ground_instance = Some(instance);
                }
                1 => {
                    self.release(app, self.crop_instance);
                    self.crop_instance = Some(instance);
                }
                _ => {
                    self.release(app, self.prop_instance);
                    self.prop_instance = Some(instance);
                }
            }
        }
        app.scene_mut().update_all_bounds();
        self.built_revision = self.map.revision();
    }

    /// Rebuilds a character's mesh when its animation frame changes.
    ///
    /// Separate from [`Valley::rebuild`] because the farm changes a few times a
    /// second and the walk cycle changes six times a second: sharing one dirty
    /// flag would rebuild six thousand vertices to animate one sprite.
    fn refresh_characters(&mut self, app: &mut App, assets: &Assets) {
        let Some(atlas) = assets.atlases.characters.as_ref() else {
            return;
        };

        let player_sprite = self.player.sprite();
        if player_sprite != self.player_sprite {
            if let Some(instance) = self.player_instance.take() {
                app.scene_mut().remove_instance(instance);
            }
            let mesh = world::build_character_mesh(atlas, &player_sprite);
            if !mesh.is_empty() {
                let handle = app.scene_mut().add_mesh(mesh);
                let instance = app.scene_mut().spawn(
                    "player",
                    handle,
                    self.character_material,
                    noxel_core::math::Transform::IDENTITY,
                );
                app.scene_mut().set_flags(
                    instance,
                    InstanceFlags {
                        occluder: false,
                        cast_shadow: false,
                        ..InstanceFlags::default()
                    },
                );
                self.player_instance = Some(instance);
            }
            self.player_sprite = player_sprite;
        }

        let villager_sprite = self.villager.sprite();
        if villager_sprite != self.villager_sprite {
            if let Some(instance) = self.villager_instance.take() {
                app.scene_mut().remove_instance(instance);
            }
            let mesh = world::build_character_mesh(atlas, &villager_sprite);
            if !mesh.is_empty() {
                let handle = app.scene_mut().add_mesh(mesh);
                let instance = app.scene_mut().spawn(
                    "villager",
                    handle,
                    self.character_material,
                    noxel_core::math::Transform::IDENTITY,
                );
                app.scene_mut().set_flags(
                    instance,
                    InstanceFlags {
                        occluder: false,
                        cast_shadow: false,
                        ..InstanceFlags::default()
                    },
                );
                self.villager_instance = Some(instance);
            }
            self.villager_sprite = villager_sprite;
        }
    }

    /// Moves the character instances to where the characters are.
    ///
    /// The transform carries the ground position and the depth offset; the mesh
    /// is anchored at the sprite's feet, so the depth is taken at the feet. A
    /// sprite sorted by its head would let the player walk in front of a fence
    /// they are standing behind.
    fn place_characters(&mut self, app: &mut App) {
        let player_feet = self.player.position.y + 0.5;
        if let Some(instance) = self.player_instance {
            app.scene_mut().set_transform(
                instance,
                noxel_core::math::Transform::from_translation(Vec3::new(
                    self.player.position.x,
                    world::depth_of(player_feet),
                    player_feet,
                )),
            );
        }
        let villager_feet = self.villager.position.1 + 0.5;
        if let Some(instance) = self.villager_instance {
            app.scene_mut().set_transform(
                instance,
                noxel_core::math::Transform::from_translation(Vec3::new(
                    self.villager.position.0,
                    world::depth_of(villager_feet),
                    villager_feet,
                )),
            );
        }
    }

    fn release(&self, app: &mut App, instance: Option<InstanceHandle>) {
        if let Some(handle) = instance {
            app.scene_mut().remove_instance(handle);
        }
    }

    /// Applies the time of day and the weather to the three materials.
    ///
    /// One place, three materials. Everything in the world is unlit — pixel art
    /// has its lighting painted in — so "the sun went down" is a multiplier on
    /// the base colour and nothing else.
    fn update_lighting(&mut self, app: &mut App) {
        let hour = self.state.clock.hour();
        // Bright from mid-morning to late afternoon, falling off at both ends,
        // with a floor so the farm is never unplayably dark.
        let daylight = match hour {
            h if h < 7.0 => 0.55,
            h if h < 9.0 => 0.55 + (h - 7.0) / 2.0 * 0.4,
            h if h < 17.0 => 0.95,
            h if h < 20.0 => 0.95 - (h - 17.0) / 3.0 * 0.35,
            _ => 0.6,
        };
        let level = (daylight * self.state.weather.today().light()).clamp(0.35, 1.0);
        let season = self.state.clock.season().tint();
        let tint = Color8::new(
            (f32::from(season.r) * 1.0) as u8,
            (f32::from(season.g) * 1.0) as u8,
            (f32::from(season.b) * 1.0) as u8,
            255,
        );
        let apply =
            |app: &mut App, handle: noxel_render::material::MaterialHandle, tint: Color8| {
                if let Some(material) = app.scene_mut().material_mut(handle) {
                    material.base_color = Color::rgb(
                        level * f32::from(tint.r) / 255.0,
                        level * f32::from(tint.g) / 255.0,
                        level * f32::from(tint.b) / 255.0,
                    );
                }
            };
        // Only the ground takes the season's tint: a pumpkin should not turn
        // orange-er in autumn because the grass did.
        apply(app, self.ground_material, tint);
        apply(app, self.crop_material, Color8::WHITE);
        apply(app, self.prop_material, Color8::WHITE);
    }

    /// The fixed-step update.
    fn update(&mut self, app: &mut App, dt: f32, assets: &Assets) {
        self.handle_keys();

        let playing = self.game_ui.screen == Screen::Playing && !self.game_ui.help_open;
        if playing {
            let axis = movement_axis(&self.input);
            let running = self.input.shift;
            let travelled = self.player.update(&self.map, axis, running, dt);

            // Walking costs energy in proportion to distance, not to frames, so
            // a slow machine does not tire the player faster.
            let minutes = dt * config::GAME_MINUTES_PER_SECOND;
            if travelled > 1e-4 {
                self.state.drain_walking_energy(minutes);
            }

            if self.input.primary_pressed || self.input.key_pressed(KEY_SPACE) {
                self.use_tool();
            }
            if self.input.secondary_pressed {
                self.use_tool();
            }
            if self.input.key_pressed(KEY_E) {
                self.interact();
            }
        }

        // The clock, and the end of the day.
        let day_over = self.state.clock.advance(dt, self.fast);
        if day_over || self.sleep_requested {
            self.state.sleep(&mut self.map);
            self.game_ui.screen = Screen::Summary;
            self.game_ui
                .toast(format!("第 {} 天开始了", self.state.clock.day_of_season()));
        }

        self.villager.update(dt, &self.map);
        self.update_lighting(app);
        self.rebuild(app, assets);
        self.refresh_characters(app, assets);
        self.place_characters(app);
        app.context.focus = Vec3::new(self.player.position.x, 0.0, self.player.position.y);
    }

    /// Reads the keys that are not movement.
    fn handle_keys(&mut self) {
        if self.input.key_pressed(KEY_TAB) {
            self.game_ui.screen = match self.game_ui.screen {
                Screen::Inventory => Screen::Playing,
                _ => Screen::Inventory,
            };
        }
        if self.input.key_pressed(KEY_SLASH) || self.input.key_pressed(KEY_H) {
            self.game_ui.help_open = !self.game_ui.help_open;
        }
        if self.input.key_pressed(KEY_ESCAPE) {
            if self.game_ui.help_open {
                self.game_ui.help_open = false;
            } else if self.game_ui.screen != Screen::Playing {
                self.game_ui.screen = Screen::Playing;
            }
        }
        // Number keys select a hotbar slot, and the wheel cycles.
        for (index, key) in [KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6]
            .iter()
            .enumerate()
        {
            if self.input.key_pressed(*key) {
                self.state.selected = index;
            }
        }
        if self.input.scroll.abs() > 0.01 {
            let slots = config::HOTBAR_SLOTS as i32;
            let delta = if self.input.scroll > 0.0 { -1 } else { 1 };
            let next = (self.state.selected as i32 + delta).rem_euclid(slots);
            self.state.selected = next as usize;
        }
    }

    /// Acts on the tile the player is facing.
    fn use_tool(&mut self) {
        if self.state.is_exhausted() {
            self.game_ui.toast("太累了，去睡一觉吧");
            return;
        }
        let tool = self.state.current_tool();
        let (tx, ty) = self.player.aim_tile();
        let Some(tile) = self.map.get(tx, ty).copied() else {
            return;
        };

        let cost = tool.energy_cost();
        self.player.start_swing();

        match tool {
            config::Tool::Hoe => {
                // Hoeing a tile that already has a plant in it would uproot it,
                // which is never what the player meant.
                if tile.ground.is_hoeable()
                    && tile.plant.is_none()
                    && self.state.spend_energy(cost)
                    && let Some(tile) = self.map.get_mut(tx, ty)
                {
                    tile.ground = world::Ground::Tilled;
                    self.last_action = Some(format!("tilled {tx},{ty}"));
                }
            }
            config::Tool::Can => {
                let Some(tile) = self.map.get_mut(tx, ty) else {
                    return;
                };
                if tile.ground == world::Ground::Tilled
                    && !tile.watered
                    && self.state.spend_energy(cost)
                {
                    tile.watered = true;
                    if let Some(plant) = tile.plant.as_mut() {
                        plant.watered = true;
                    }
                    self.last_action = Some(format!("watered {tx},{ty}"));
                }
            }
            config::Tool::Scythe | config::Tool::Hand => {
                let harvested = self
                    .map
                    .get(tx, ty)
                    .and_then(|t| t.plant)
                    .filter(|p| p.is_ripe());
                if let Some(plant) = harvested {
                    let leftover = self.state.inventory.add(Item::Produce(plant.crop), 1);
                    if leftover > 0 {
                        self.game_ui.toast("背包满了");
                        return;
                    }
                    let _ = self.state.spend_energy(cost);
                    if let Some(tile) = self.map.get_mut(tx, ty) {
                        if plant.crop.regrow_days > 0 {
                            // A regrowing crop is cut back rather than dug up.
                            if let Some(p) = tile.plant.as_mut() {
                                p.days = plant
                                    .crop
                                    .growth_days
                                    .saturating_sub(plant.crop.regrow_days);
                            }
                        } else {
                            tile.plant = None;
                            tile.ground = world::Ground::Dirt;
                        }
                    }
                    self.game_ui.toast(format!("收获 {}", plant.crop.name));
                    self.last_action = Some(format!("harvested {tx},{ty}"));
                }
            }
            config::Tool::Axe | config::Tool::Pickaxe => {
                // Clears a weed, a rock or a small prop, which is what opens the
                // farm up in the first week.
                let clearable = tile.prop.filter(|name| {
                    matches!(
                        *name,
                        "weed"
                            | "rock_small"
                            | "rock_large"
                            | "bush"
                            | "log"
                            | "mushroom"
                            | "tree_stump"
                    )
                });
                if let Some(name) = clearable {
                    if self.state.spend_energy(cost) {
                        if let Some(tile) = self.map.get_mut(tx, ty) {
                            tile.prop = None;
                        }
                        self.last_action = Some(format!("cleared {name} at {tx},{ty}"));
                    }
                }
            }
        }

        // Planting: a seed in hand on tilled soil.
        let seeds = self.selected_seed();
        if let Some(crop) = seeds {
            if tile.ground == world::Ground::Tilled && tile.plant.is_none() {
                let item = Item::Seed(crop);
                if self.state.inventory.count_of(item) > 0 {
                    self.state.inventory.remove(item, 1);
                    if let Some(tile) = self.map.get_mut(tx, ty) {
                        tile.plant = Some(world::Plant::new(crop));
                    }
                    self.game_ui.toast(format!("种下 {}", crop.name));
                    self.last_action = Some(format!("planted {} at {tx},{ty}", crop.key));
                }
            }
        }
    }

    /// The crop whose seed is in the selected slot, if any.
    fn selected_seed(&self) -> Option<&'static config::Crop> {
        let slot = self
            .state
            .inventory
            .slots()
            .get(self.state.selected)?
            .as_ref()?;
        match slot.item {
            Item::Seed(crop) => Some(crop),
            _ => None,
        }
    }

    /// Opens the shop or the bin, or sleeps.
    fn interact(&mut self) {
        match world::prompt_for(self.player.tile()) {
            Some("进入种子商店") => self.game_ui.screen = Screen::Shop,
            Some("打开出货箱") => self.game_ui.screen = Screen::Bin,
            _ => self.sleep_requested = true,
        }
    }

    /// Draws the interface.
    fn draw(&mut self, app: &App, framebuffer: &mut Framebuffer, assets: &Assets) {
        let input = self.input.clone();
        self.game_ui.begin(app.config.fixed_dt, &input);
        // The action is applied here rather than in `update`, because this is
        // where the widgets run and the answer is only known now.
        let action = {
            let state = &self.state;
            let player = &self.player;
            self.game_ui
                .draw(framebuffer, &input, state, player, assets)
        };
        self.apply(action);
        self.game_ui.end();
    }

    fn apply(&mut self, action: UiAction) {
        match action {
            UiAction::None => {}
            UiAction::SelectSlot(index) => self.state.selected = index,
            UiAction::BuySeed(crop, quantity) => {
                let cost = crop.seed_price * quantity;
                if self.state.inventory.free_slots() == 0 {
                    self.game_ui.toast("背包满了");
                    return;
                }
                if self.state.inventory.spend(cost) {
                    let leftover = self.state.inventory.add(Item::Seed(crop), quantity);
                    if leftover > 0 {
                        // Hand back what did not fit rather than deleting it.
                        self.state.inventory.earn(crop.seed_price * leftover);
                        self.game_ui.toast("背包满了，部分已退款");
                    } else {
                        self.game_ui
                            .toast(format!("买了 {} 个{}种子", quantity, crop.name));
                    }
                } else {
                    self.game_ui.toast("金币不够");
                }
            }
            UiAction::ShipAll => {
                let mut moved = 0;
                for index in 0..self.state.inventory.slots().len() {
                    let Some(slot) = self.state.inventory.slots()[index] else {
                        continue;
                    };
                    if !matches!(slot.item, Item::Produce(_)) {
                        continue;
                    }
                    let count = slot.count;
                    if self.state.inventory.remove(slot.item, count) == count {
                        self.state.bin.add(slot.item, count);
                        moved += count;
                    }
                }
                if moved > 0 {
                    let value = self.state.bin.value();
                    self.game_ui
                        .toast(format!("放入 {moved} 件，价值 {value} 金"));
                } else {
                    self.game_ui.toast("没有可以出货的作物");
                }
            }
            UiAction::Close => self.game_ui.screen = Screen::Playing,
            UiAction::Sleep => self.sleep_requested = true,
        }
    }

    /// A one-line status for the window title and the log.
    fn status(&self) -> String {
        format!(
            "{} {} | {} | {} 金 | 体力 {:.0}",
            self.state.clock.season().name(),
            self.state.clock.day_of_season(),
            self.state.clock.time_string(),
            self.state.inventory.gold,
            self.state.energy
        )
    }
}

fn texture_handle(
    app: &mut App,
    assets: &Assets,
    which: &str,
) -> noxel_render::material::TextureHandle {
    let atlas = match which {
        "terrain" => assets.atlases.terrain.as_ref(),
        "crops" => assets.atlases.crops.as_ref(),
        "characters" => assets.atlases.characters.as_ref(),
        _ => assets.atlases.props.as_ref(),
    };
    match atlas {
        Some(atlas) => app.scene_mut().add_texture(atlas.texture().clone()),
        // A one-pixel white texture, so a missing atlas renders as flat colour
        // rather than failing to build.
        None => {
            let image = noxel_asset::image::Image::new(1, 1, Color8::WHITE);
            app.scene_mut()
                .add_texture(noxel_asset::texture::Texture::from_image(image))
        }
    }
}

/// One other person on the farm, walking a fixed route.
///
/// Deliberately not a crowd. `noxel-npc` exists for that and carries a flow-field
/// solver, a steering stack and a schedule system — all of which a single
/// villager pacing the plaza does not need. What this does need is to make the
/// farm look inhabited, and eight lines of waypoint following achieves that.
struct Villager {
    /// Waypoints in tiles, walked in order and then looped.
    route: [(f32, f32); 4],
    /// Which waypoint is being walked towards.
    target: usize,
    /// Current position in tiles.
    position: (f32, f32),
    /// Facing, for the sprite.
    facing: noxel_valley::player::Facing,
    /// Seconds spent walking, for the animation phase.
    phase: f32,
    /// Whether they are moving.
    moving: bool,
}

impl Default for Villager {
    fn default() -> Self {
        Self {
            // A short loop along the plaza in front of the farmhouse.
            route: [(13.5, 9.5), (24.5, 9.5), (24.5, 11.5), (13.5, 11.5)],
            target: 1,
            position: (13.5, 9.5),
            facing: noxel_valley::player::Facing::Right,
            phase: 0.0,
            moving: true,
        }
    }
}

impl Villager {
    /// Steps along the route.
    fn update(&mut self, dt: f32, map: &FarmMap) {
        const SPEED: f32 = 2.2;
        let goal = self.route[self.target];
        let dx = goal.0 - self.position.0;
        let dy = goal.1 - self.position.1;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < 0.05 {
            self.target = (self.target + 1) % self.route.len();
            return;
        }
        let step = (SPEED * dt).min(distance);
        let (nx, ny) = (dx / distance, dy / distance);
        self.facing = noxel_valley::player::Facing::from_vector(nx, ny);
        let next = (self.position.0 + nx * step, self.position.1 + ny * step);
        // If the route is blocked by scenery the villager simply waits rather
        // than pathing around it; the route is authored through open ground.
        if map.is_walkable(next.0.floor() as i32, next.1.floor() as i32) {
            self.position = next;
            self.moving = true;
            self.phase += dt;
        } else {
            self.moving = false;
        }
    }

    /// The atlas region for the current pose.
    fn sprite(&self) -> String {
        let frame = if self.moving {
            [1u32, 2, 3, 2][(self.phase * 5.0) as usize % 4]
        } else {
            0
        };
        format!("villager_{}_{}", self.facing.key(), frame)
    }
}

/// A camera that maps one world unit to exactly one tile of pixels.
fn setup_camera(app: &mut App) {
    let camera = app.camera_mut();
    camera.set_projection(noxel_camera::ProjectionMode::Orthographic {
        height: ORTHO_HEIGHT,
    });
    // Straight down. A tilted camera would foreshorten the ground and resample
    // every upright sprite by `cos(pitch)`, which is exactly the softness the
    // art is drawn to avoid.
    camera.set_pitch(core::f32::consts::FRAC_PI_2);
    camera.set_yaw_immediate(0.0);
    camera.lock_rotation();
    camera.set_smoothing(0.35);
    camera.set_deadzone(0.0, 0.0, 0.0);
    camera.set_look_ahead(0.0, 0.0);
    camera.set_pixel_perfect(noxel_camera::PixelPerfect::at(
        INTERNAL_WIDTH,
        INTERNAL_HEIGHT,
    ));
    camera.set_clip_planes(0.05, 400.0);
    camera.set_bounds(Some(noxel_core::math::Aabb::new(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(FARM_WIDTH as f32, 1.0, FARM_HEIGHT as f32),
    )));
}

/// The movement axis from WASD and the arrow keys.
fn movement_axis(input: &UiInput) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    for key in input.keys_held.iter().copied() {
        match key {
            KEY_W | KEY_UP => y -= 1.0,
            KEY_S | KEY_DOWN => y += 1.0,
            KEY_A | KEY_LEFT => x -= 1.0,
            KEY_D | KEY_RIGHT => x += 1.0,
            _ => {}
        }
    }
    (x, y)
}

/// A font for the case where the bake on disk is missing.
///
/// A blank font draws no text at all. That is worse than a fallback face and
/// better than a panic: the game still runs, the layout is correct, and the
/// startup banner says exactly which command regenerates the bake. A silent
/// blank UI with no explanation is the failure this avoids.
fn default_font() -> noxel_ui::FontSet {
    noxel_ui::FontSet::blank()
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

const KEY_ESCAPE: u32 = 27;
const KEY_SPACE: u32 = 32;
const KEY_TAB: u32 = 9;
const KEY_1: u32 = 49;
const KEY_2: u32 = 50;
const KEY_3: u32 = 51;
const KEY_4: u32 = 52;
const KEY_5: u32 = 53;
const KEY_6: u32 = 54;
const KEY_E: u32 = 69;
const KEY_H: u32 = 72;
const KEY_SLASH: u32 = 47;
const KEY_W: u32 = 87;
const KEY_A: u32 = 65;
const KEY_S: u32 = 83;
const KEY_D: u32 = 68;
const KEY_UP: u32 = 38;
const KEY_DOWN: u32 = 40;
const KEY_LEFT: u32 = 37;
const KEY_RIGHT: u32 = 39;

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// Adapter that lets the engine drive the game.
///
/// The state lives behind an `Rc<RefCell<_>>` so the host that owns the window
/// can read it for the title and the end-of-run report while the engine holds
/// the plugin. The alternative — the plugin owning the state outright — makes
/// every read from outside a downcast.
struct ValleyPlugin {
    valley: Rc<RefCell<Valley>>,
    assets: Rc<Assets>,
}

impl Plugin for ValleyPlugin {
    fn name(&self) -> &str {
        "noxel-valley"
    }

    fn update(&mut self, app: &mut App, dt: f32) {
        let mut valley = self.valley.borrow_mut();
        valley.update(app, dt, &self.assets);
    }

    fn draw(&mut self, app: &mut App, framebuffer: &mut Framebuffer) {
        let mut valley = self.valley.borrow_mut();
        valley.draw(app, framebuffer, &self.assets);
    }

    fn shutdown(&mut self, app: &mut App) {
        let valley = self.valley.borrow();
        app.debug_mut()
            .record_counter("gold", valley.state.inventory.gold as f32);
        app.debug_mut()
            .record_counter("crops", valley.map.ripe_count() as f32);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// A parsed command line.
#[derive(Clone, Debug)]
struct Args {
    window: bool,
    frames: u64,
    seed: u64,
    fast: bool,
    stats: bool,
    dump: Option<std::path::PathBuf>,
    assets: Option<std::path::PathBuf>,
    /// Open this screen on the first frame, for screenshots and for the
    /// documentation's figures.
    screen: Option<Screen>,
    help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            window: false,
            frames: 0,
            seed: 0x5EED,
            fast: false,
            stats: false,
            dump: None,
            assets: None,
            screen: None,
            help: false,
        }
    }
}

const USAGE: &str = "\
noxel-valley — a playable top-down farming game

USAGE:
    noxel-valley [--window] [--frames N] [--seed N] [--fast] [--stats] [--dump DIR]

OPTIONS:
    --window       Play in a window. Needs a build with `--features window`.
                   A build with window support and no other flag opens a window
                   anyway, so double-clicking the game just plays it.
    --frames N     Run N frames and stop. 0 (the default) runs forever.
    --seed N       World and weather seed (decimal, or 0x-prefixed).
    --fast         Run the clock 24x, so a season passes in minutes.
    --stats        Print one frame's statistics and exit.
    --dump DIR     Write each frame as a PNG into DIR.
    --assets DIR   Asset root. Found automatically by default.
    --screen NAME  Open a screen at startup: inventory, shop, bin, summary.
    -h, --help     Print this text.

CONTROLS:
    WASD / arrows   walk                 Shift   run
    Space / LMB     use the held tool    1-6     select a hotbar slot
    Tab             bag                  E       shop, shipping bin, or sleep
    ?               controls             Esc     close / quit
";

impl Args {
    /// Whether to open a window rather than render to PNGs.
    ///
    /// `--window` asks for one explicitly. Beyond that, a build with window
    /// support that was given no output flag is someone who double-clicked the
    /// game, so it plays: `./noxel-valley` opens a window, and
    /// `./noxel-valley --frames 300 --dump frames` still renders headlessly.
    /// Anything else makes "just run it" do something other than run it.
    fn wants_window(&self) -> bool {
        if self.window {
            return true;
        }
        if !cfg!(feature = "window") {
            return false;
        }
        self.frames == 0 && !self.stats && self.dump.is_none() && self.screen.is_none()
    }

    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--window" => parsed.window = true,
                "--fast" => parsed.fast = true,
                "--stats" => parsed.stats = true,
                "-h" | "--help" => parsed.help = true,
                "--frames" => {
                    let value = args.next().ok_or("--frames needs a number")?;
                    parsed.frames = value
                        .parse()
                        .map_err(|_| format!("bad --frames: {value}"))?;
                }
                "--seed" => {
                    let value = args.next().ok_or("--seed needs a number")?;
                    parsed.seed =
                        parse_number(&value).map_err(|_| format!("bad --seed: {value}"))?;
                }
                "--dump" => {
                    let value = args.next().ok_or("--dump needs a directory")?;
                    parsed.dump = Some(value.into());
                }
                "--assets" => {
                    let value = args.next().ok_or("--assets needs a directory")?;
                    parsed.assets = Some(value.into());
                }
                "--screen" => {
                    let value = args.next().ok_or("--screen needs a name")?;
                    parsed.screen = Some(match value.as_str() {
                        "inventory" | "bag" => Screen::Inventory,
                        "shop" => Screen::Shop,
                        "bin" | "shipping" => Screen::Bin,
                        "summary" => Screen::Summary,
                        other => return Err(format!("unknown screen {other:?}")),
                    });
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(parsed)
    }
}

fn parse_number(text: &str) -> Result<u64, core::num::ParseIntError> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    }
}

fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("noxel-valley: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return;
    }
    if let Err(message) = run(&args) {
        eprintln!("noxel-valley: {message}");
        std::process::exit(1);
    }
}

fn build_app(args: &Args, assets: &Assets) -> Result<App, String> {
    let mut config = AppConfig::headless();
    config.seed = args.seed;
    config.internal = (INTERNAL_WIDTH, INTERNAL_HEIGHT);
    config.mode = ShadingMode::Raster;
    config.assets_root = assets.root.clone().unwrap_or_default();
    // A UI drawn every frame into a framebuffer that also has a debug overlay is
    // two things writing the same pixels; every debug panel stays off.
    config.debug = noxel_app::DebugConfig::disabled();
    if let Some(directory) = &args.dump {
        config.dump = Some((directory.clone(), noxel_debug::dump::DumpFormat::Png));
    }
    App::new(config).map_err(|error| error.to_string())
}

fn run(args: &Args) -> Result<(), String> {
    let root = args.assets.clone().or_else(assets::find_root);
    let assets = Rc::new(Assets::load(root));

    match &assets.root {
        Some(path) => println!("noxel-valley: assets from {}", path.display()),
        None => println!("noxel-valley: no assets found; running with flat colours"),
    }
    if !assets.has_art() {
        println!("  (run `noxel-gen farm --out games/noxel-valley/assets` to generate the art)");
    }
    if !assets.has_font() {
        println!(
            "  (run `tools/fontgen/fontgen.py --out games/noxel-valley/assets/fonts` for the font)"
        );
    }

    let mut app = build_app(args, &assets)?;
    let valley = Rc::new(RefCell::new(Valley::new(
        &mut app,
        args.seed as u32,
        &assets,
        args.fast,
    )));
    // `--screen` exists so a screenshot of the shop does not need someone to
    // walk to the shop first — for the documentation's figures and for a
    // regression check on each overlay.
    if let Some(screen) = args.screen {
        valley.borrow_mut().game_ui.screen = screen;
    }
    app.add_plugin(ValleyPlugin {
        valley: Rc::clone(&valley),
        assets: Rc::clone(&assets),
    });

    // Warm up so the first presented frame is a farm rather than an empty scene.
    for _ in 0..2 {
        app.step(1.0 / 60.0);
    }

    if args.stats {
        let report = app.run_and_capture(1);
        println!("{}", report.summary());
        println!("{}", valley.borrow().status());
        return Ok(());
    }

    if args.wants_window() {
        return run_window(args, app, valley);
    }

    run_headless(args, app, valley)
}

/// Renders a fixed number of frames and writes each one as a PNG.
fn run_headless(args: &Args, mut app: App, valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    let frames = if args.frames == 0 { 600 } else { args.frames };
    let started = std::time::Instant::now();
    for _frame in 0..frames {
        // Drive the clock from the frame index so a headless run is exactly
        // reproducible: the same command produces the same PNGs on any machine.
        let dt = 1.0 / 60.0;
        app.step(dt);
        // Every frame is written when `--dump` is on: this is the mode a golden
        // image test drives, and a subset would make "which frame is this" a
        // question the file name already answers.
        if args.dump.is_some() {
            let _ = app.dump_current_frame();
        }
    }
    let elapsed = started.elapsed().as_secs_f32();
    let stats = app.debug().stats();
    println!(
        "noxel-valley: {frames} frames in {elapsed:.2}s ({:.1} fps), mean {:.2} ms, p95 {:.2} ms",
        frames as f32 / elapsed.max(1e-6),
        stats.frame_times.mean(),
        stats.frame_times.percentile(0.95)
    );
    println!("{}", valley.borrow().status());
    Ok(())
}

/// Opens a window and plays.
#[cfg(feature = "window")]
fn run_window(args: &Args, app: App, valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    use noxel_ui::UiInputBuilder;

    struct Game {
        app: App,
        valley: Rc<RefCell<Valley>>,
        input: UiInput,
        frames: u64,
        limit: u64,
    }

    impl noxel_window::Host for Game {
        fn step(&mut self, dt: f32, input: &noxel_window::Input) -> &Framebuffer {
            // One conversion, in one place: the window host has already mapped
            // the cursor into framebuffer pixels using the same presentation the
            // upscale uses, so a hit test here is in the space the UI draws in.
            self.input = UiInputBuilder::new()
                .at(input.cursor.0, input.cursor.1)
                .build();
            self.input.pointer_inside = input.cursor_inside;
            self.input.primary_down = input.mouse_buttons[0];
            self.input.primary_pressed = input.mouse_pressed[0];
            self.input.primary_released = input.mouse_released[0];
            self.input.secondary_pressed = input.mouse_pressed[2];
            self.input.scroll = input.scroll;
            self.input.shift = input.shift;
            self.input.control = input.control;
            self.input.alt = input.alt;
            self.input.keys_held = input.held().to_vec();
            self.input.keys_pressed = input.pressed().to_vec();

            self.valley.borrow_mut().input = self.input.clone();
            self.app.step(dt);
            self.frames += 1;
            self.app.framebuffer()
        }

        fn internal_size(&self) -> (u32, u32) {
            (INTERNAL_WIDTH, INTERNAL_HEIGHT)
        }

        fn should_quit(&self) -> bool {
            self.limit > 0 && self.frames >= self.limit
        }

        fn title_suffix(&self) -> String {
            self.valley.borrow().status()
        }
    }

    let config = noxel_window::WindowConfig::default()
        .with_title("星野农场 · Noxel Valley")
        .with_internal(INTERNAL_WIDTH, INTERNAL_HEIGHT);
    let game = Game {
        app,
        valley: Rc::clone(&valley),
        input: UiInput::new(),
        frames: 0,
        limit: args.frames,
    };
    noxel_window::run(config, game).map_err(|error| error.to_string())
}

#[cfg(not(feature = "window"))]
fn run_window(_args: &Args, _app: App, _valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    Err("this build has no window support; rebuild with `--features window`".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_ui::UiInputBuilder;
    use noxel_valley::config::TILE;

    #[test]
    fn the_seed_parser_accepts_decimal_and_hexadecimal() {
        assert_eq!(parse_number("1234").unwrap(), 1234);
        assert_eq!(parse_number("0x10").unwrap(), 16);
        assert_eq!(parse_number("0XFF").unwrap(), 255);
        assert!(parse_number("nonsense").is_err());
    }

    #[test]
    fn the_argument_parser_reads_every_flag() {
        let args = Args::parse(
            [
                "--window", "--frames", "30", "--seed", "0xAB", "--fast", "--stats",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .unwrap();
        assert!(args.window);
        assert_eq!(args.frames, 30);
        assert_eq!(args.seed, 0xAB);
        assert!(args.fast);
        assert!(args.stats);
    }

    #[test]
    fn an_output_flag_means_headless_and_nothing_means_play() {
        // The rule that makes a double-clicked bundle play the game instead of
        // rendering 600 PNGs into a directory nobody will look at.
        if !cfg!(feature = "window") {
            // Without the feature there is no window to open, and every run is
            // headless regardless.
            assert!(!Args::default().wants_window());
            return;
        }
        assert!(Args::default().wants_window(), "no arguments should play the game");
        assert!(Args { window: true, ..Args::default() }.wants_window());

        let headless = [
            Args { frames: 300, ..Args::default() },
            Args { stats: true, ..Args::default() },
            Args { dump: Some(std::path::PathBuf::from("frames")), ..Args::default() },
            Args { screen: Some(Screen::Shop), ..Args::default() },
        ];
        for args in headless {
            assert!(!args.wants_window(), "{args:?} should render, not play");
        }
    }

    #[test]
    fn the_screen_argument_names_every_overlay_and_rejects_the_rest() {
        for (name, expected) in [
            ("inventory", Screen::Inventory),
            ("shop", Screen::Shop),
            ("bin", Screen::Bin),
            ("summary", Screen::Summary),
        ] {
            let args = Args::parse(["--screen", name].iter().map(|s| s.to_string())).unwrap();
            assert_eq!(args.screen, Some(expected), "for {name}");
        }
        assert!(Args::parse(["--screen", "nonsense"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn an_unknown_flag_is_an_error_rather_than_being_ignored() {
        // Silently ignoring an argument is how a run ends up doing something
        // other than what was asked for.
        assert!(Args::parse(["--nonsense"].iter().map(|s| s.to_string())).is_err());
        assert!(Args::parse(["--frames"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn the_movement_axis_combines_wasd_and_the_arrows() {
        let mut input = UiInputBuilder::new().key(KEY_D).key(KEY_UP).build();
        assert_eq!(movement_axis(&input), (1.0, -1.0));
        input.keys_held = vec![KEY_A, KEY_D];
        assert_eq!(movement_axis(&input).0, 0.0, "opposite keys cancel");
        input.keys_held = vec![KEY_LEFT, KEY_RIGHT];
        assert_eq!(movement_axis(&input).0, 0.0);
    }

    #[test]
    fn the_camera_maps_one_world_unit_to_one_tile_of_pixels() {
        // The crispness contract, checked against the constant the camera is
        // actually configured with rather than against a literal.
        let pixels_per_unit = INTERNAL_HEIGHT as f32 / ORTHO_HEIGHT;
        assert!((pixels_per_unit - TILE as f32).abs() < 1e-4);
    }

    #[test]
    fn the_real_asset_tree_loads_and_the_farm_reaches_the_renderer() {
        // The test that would have caught "the world is empty": every unit test
        // ran against `Assets::empty()`, where an empty scene is correct. This
        // one loads what actually ships and asserts that the farm is in the
        // scene and survives culling.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        if !root.join("farm/terrain.png").exists() {
            // The art is generated, not committed to the source of a fresh
            // checkout of the engine; skip rather than fail there.
            eprintln!("skipping: run `noxel-gen farm --out games/noxel-valley/assets` first");
            return;
        }
        let assets = Assets::load(Some(root));
        assert!(assets.has_art(), "the farm atlases did not load");
        assert!(assets.atlases.terrain.is_some(), "terrain atlas missing");
        assert!(assets.atlases.crops.is_some(), "crop atlas missing");
        assert!(assets.atlases.props.is_some(), "prop atlas missing");
        assert!(
            assets.atlases.characters.is_some(),
            "character atlas missing"
        );

        let args = Args {
            seed: 3,
            ..Args::default()
        };
        let mut app = build_app(&args, &assets).expect("the app must build");
        let _valley = Valley::new(&mut app, 3, &assets, false);
        // Ground and props; the crop mesh is legitimately empty until something
        // is planted, which is why this is not `>= 3`.
        assert!(
            app.scene().instance_count() >= 2,
            "the farm meshes were not spawned: {} instances",
            app.scene().instance_count()
        );

        app.step(1.0 / 60.0);
        assert!(
            !app.context.visible.items.is_empty(),
            "every instance was culled, so the frame renders nothing"
        );
        // Culling accepting an instance is not the same as the rasterizer
        // drawing it: a mesh with the wrong winding passes every cull and then
        // has all of its triangles rejected as backfaces. Counting lit pixels is
        // the only check that covers both.
        let framebuffer = app.framebuffer();
        let lit = framebuffer
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.08)
            .count();
        assert!(
            lit > 10_000,
            "the farm passed culling but drew only {lit} of {} pixels",
            framebuffer.pixel_count()
        );
    }

    #[test]
    fn the_game_builds_and_draws_with_no_assets() {
        // The whole point of the flat fallback: `cargo test` on a fresh checkout
        // must be able to build a working game.
        let assets = Assets::empty();
        let args = Args {
            seed: 7,
            ..Args::default()
        };
        let mut app = build_app(&args, &assets).expect("the app must build");
        let valley = Valley::new(&mut app, 7, &assets, false);
        assert_eq!(valley.map.width(), FARM_WIDTH);
        assert_eq!(valley.map.height(), FARM_HEIGHT);
        assert!(valley.state.inventory.gold > 0);
    }
}
