//! The application: configuration, the shared context, and the frame loop types.

use std::path::PathBuf;
use std::sync::Arc;

use noxel_asset::db::{AssetDb, AssetError};
use noxel_asset::format::{Prefab, TileSet};
use noxel_camera::TopDownCamera;
use noxel_core::math::{Aabb, Vec3};
use noxel_core::time::GameClock;
use noxel_debug::{DebugConfig, DebugSystem, DumpFormat};
use noxel_ecs::{Entity, World};
use noxel_physics::PhysicsWorld;
use noxel_render::CameraView;
use noxel_render::framebuffer::Framebuffer;
use noxel_render::raster::RasterRenderer;
use noxel_render::raytrace::RayTracer;
use noxel_render::renderer::{RenderSettings, Renderer, ShadingMode};
use noxel_render::scene::Scene;
use noxel_visibility::{VisibilityConfig, VisibilityInput, VisibilitySystem};
use noxel_world::WorldConfig;
use noxel_world::stream::WorldStreamer;

use crate::plugin::PluginRegistry;

/// Anything that can go wrong while starting or stepping an app.
#[derive(Debug)]
pub enum AppError {
    /// An asset could not be loaded.
    Asset(AssetError),
    /// A file could not be read or written.
    Io(std::io::Error),
    /// The configured render target has no area.
    EmptyViewport,
}

impl core::fmt::Display for AppError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Asset(e) => write!(f, "asset error: {e}"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::EmptyViewport => write!(f, "the internal render size must be at least 1x1"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<AssetError> for AppError {
    fn from(value: AssetError) -> Self {
        Self::Asset(value)
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Keyboard and mouse state.
///
/// The app never talks to a platform API, so the host fills this in and the game
/// reads it. Keeping it a plain data structure also means a test can drive a
/// whole play session with no window at all — which is how the demo's
/// golden-frame test works.
#[derive(Clone, Debug, Default)]
pub struct InputState {
    /// Keys currently held.
    held: Vec<u32>,
    /// Keys pressed since the last [`InputState::end_frame`].
    pressed: Vec<u32>,
    /// Keys released since the last [`InputState::end_frame`].
    released: Vec<u32>,
    /// Mouse position in internal pixels.
    pub mouse: (f32, f32),
    /// Mouse movement since the last frame.
    pub mouse_delta: (f32, f32),
    /// Scroll wheel movement since the last frame.
    pub scroll: f32,
    /// Whether each mouse button is held.
    pub mouse_buttons: [bool; 3],
}

impl InputState {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a key going down.
    pub fn press(&mut self, key: u32) {
        if !self.held.contains(&key) {
            self.held.push(key);
        }
        if !self.pressed.contains(&key) {
            self.pressed.push(key);
        }
    }

    /// Records a key coming up.
    pub fn release(&mut self, key: u32) {
        self.held.retain(|k| *k != key);
        if !self.released.contains(&key) {
            self.released.push(key);
        }
    }

    /// True while a key is held.
    #[must_use]
    pub fn is_held(&self, key: u32) -> bool {
        self.held.contains(&key)
    }

    /// True on the frame the key went down.
    #[must_use]
    pub fn was_pressed(&self, key: u32) -> bool {
        self.pressed.contains(&key)
    }

    /// True on the frame the key came up.
    #[must_use]
    pub fn was_released(&self, key: u32) -> bool {
        self.released.contains(&key)
    }

    /// The movement axis as a `(x, z)` pair, from the four arrow/WASD keys.
    #[must_use]
    pub fn movement_axis(&self, up: u32, down: u32, left: u32, right: u32) -> (f32, f32) {
        let x = f32::from(self.is_held(right)) - f32::from(self.is_held(left));
        let z = f32::from(self.is_held(down)) - f32::from(self.is_held(up));
        (x, z)
    }

    /// Clears the per-frame deltas. Call once at the end of a frame.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.mouse_delta = (0.0, 0.0);
        self.scroll = 0.0;
    }

    /// Releases every key, for a focus loss.
    pub fn release_all(&mut self) {
        self.released.extend(self.held.iter().copied());
        self.held.clear();
        self.mouse_buttons = [false; 3];
    }
}

/// The app's configuration.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// World seed.
    pub seed: u64,
    /// Internal render resolution. Pixel art is authored for a fixed size and
    /// upscaled, never rendered at the window size.
    pub internal: (u32, u32),
    /// Gameplay timestep, in seconds.
    pub fixed_dt: f32,
    /// Maximum fixed steps per frame, before time is dropped.
    pub max_substeps: u32,
    /// Which renderer to use.
    pub mode: ShadingMode,
    /// Visibility thresholds.
    pub visibility: VisibilityConfig,
    /// World generation settings.
    pub world: WorldConfig,
    /// Render settings.
    pub render: RenderSettings,
    /// Debug overlay and dumping.
    pub debug: DebugConfig,
    /// Where to write dumped frames, if anywhere.
    pub dump: Option<(PathBuf, DumpFormat)>,
    /// Root of the asset tree.
    pub assets_root: PathBuf,
    /// Stop automatically after this many frames. `0` means run forever.
    pub max_frames: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            seed: 0x4E4F_5845,
            internal: (320, 180),
            fixed_dt: 1.0 / 60.0,
            max_substeps: 8,
            mode: ShadingMode::Raster,
            visibility: VisibilityConfig::default(),
            world: WorldConfig::default(),
            render: RenderSettings::default(),
            debug: DebugConfig::default(),
            dump: None,
            assets_root: PathBuf::from("examples/town-demo/assets"),
            max_frames: 0,
        }
    }
}

impl AppConfig {
    /// A configuration with no dumping and no assets, for a test.
    #[must_use]
    pub fn headless() -> Self {
        Self {
            internal: (160, 90),
            debug: DebugConfig::minimal(),
            dump: None,
            max_frames: 0,
            ..Self::default()
        }
    }

    /// A configuration that writes every frame as a PNG.
    #[must_use]
    pub fn dumping(directory: impl Into<PathBuf>) -> Self {
        Self {
            dump: Some((directory.into(), DumpFormat::Png)),
            ..Self::default()
        }
    }

    /// Sets the internal resolution.
    #[must_use]
    pub fn with_internal(mut self, width: u32, height: u32) -> Self {
        self.internal = (width.max(1), height.max(1));
        self
    }

    /// Sets the renderer.
    #[must_use]
    pub fn with_mode(mut self, mode: ShadingMode) -> Self {
        self.mode = mode;
        self
    }

    /// The internal aspect ratio, `16/9` by default.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        if self.internal.1 == 0 {
            1.0
        } else {
            self.internal.0 as f32 / self.internal.1 as f32
        }
    }
}

/// Everything a plugin can reach.
///
/// A plugin gets `&mut App`, and `App` derefs to this context, so a plugin writes
/// `app.physics` rather than threading a dozen arguments through every system.
pub struct AppContext {
    /// The render scene: meshes, materials, instances.
    pub scene: Scene,
    /// The entity-component world.
    pub world: World,
    /// Rigid bodies, queries and the character controller.
    pub physics: PhysicsWorld,
    /// Chunk streaming around the camera.
    pub streamer: WorldStreamer,
    /// Culling, occlusion and fades.
    pub visibility: VisibilitySystem,
    /// Statistics, overlays and frame dumping.
    pub debug: DebugSystem,
    /// The camera rig.
    pub camera: TopDownCamera,
    /// The fixed-step clock.
    pub clock: GameClock,
    /// Input, filled by the host.
    pub input: InputState,
    /// Loaded assets.
    pub assets: AssetDb,
    /// Frames rendered since the app started.
    pub frame: u64,
    /// Total time since the app started, in seconds.
    pub elapsed: f32,
    /// The player entity, when there is one. The camera follows it and the
    /// occlusion system keeps it visible.
    pub player: Option<Entity>,
    /// The camera's view for the current frame.
    pub view: CameraView,
    /// The world position the camera must not lose sight of.
    pub focus: Vec3,
    /// The visible set produced by the last visibility pass.
    pub visible: noxel_render::renderer::VisibleSet,
    /// The tile set the world is built from.
    pub tile_set: Arc<TileSet>,
    /// The prefab library.
    pub prefabs: Vec<Arc<Prefab>>,
}

impl AppContext {
    /// The player's world position, or the camera focus when there is none.
    #[must_use]
    pub fn player_position(&self) -> Vec3 {
        self.focus
    }

    /// The world-space region the camera can currently see, as a ground-plane
    /// box with a generous vertical extent.
    #[must_use]
    pub fn view_bounds(&self) -> Aabb {
        let (min, max) = self.camera.visible_ground_rect(1.6);
        Aabb::new(
            Vec3::new(min.x, -200.0, min.y),
            Vec3::new(max.x, 200.0, max.y),
        )
    }

    /// The region the streamer must keep resident, for a debug readout.
    #[must_use]
    pub fn keep_bounds(&self) -> Aabb {
        self.view_bounds().expanded(8.0)
    }
}

/// The application.
pub struct App {
    /// The shared context.
    pub context: AppContext,
    /// The registered plugins.
    pub plugins: PluginRegistry,
    /// The configuration.
    pub config: AppConfig,
    /// The render target.
    pub framebuffer: Framebuffer,
    /// The software rasterizer, always built: it is the shadow-map source for
    /// the hybrid mode even when the ray tracer draws the image.
    pub raster: RasterRenderer,
    /// The ray tracer, used by the hybrid and full ray-traced modes.
    pub raytracer: RayTracer,
}

impl App {
    /// Builds an app, loading whatever assets the configured root contains.
    ///
    /// A missing or empty asset root is not an error: the world falls back to
    /// procedural content, which is what makes `App::new` usable in a test.
    ///
    /// # Errors
    /// Returns [`AppError::EmptyViewport`] when the internal size is zero, or an
    /// asset error when a manifest that *does* exist cannot be parsed.
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        if config.internal.0 == 0 || config.internal.1 == 0 {
            return Err(AppError::EmptyViewport);
        }
        let mut assets = AssetDb::new(&config.assets_root);
        let (tile_set, prefabs) = load_assets(&mut assets);

        let mut world_config = config.world.clone();
        world_config.seed = config.seed;
        let streamer = WorldStreamer::new(noxel_world::WorldGenerator::new(
            world_config,
            Arc::clone(&tile_set),
            prefabs.clone(),
        ));

        let mut camera = TopDownCamera::pixel_art(config.internal.0, config.internal.1);
        camera.set_projection(noxel_camera::ProjectionMode::Orthographic { height: 18.0 });

        let mut clock = GameClock::with_fixed_dt(config.fixed_dt);
        clock.set_max_substeps(config.max_substeps);

        let context = AppContext {
            scene: Scene::new(),
            world: World::new(),
            physics: PhysicsWorld::new(noxel_physics::PhysicsConfig::default()),
            streamer,
            visibility: VisibilitySystem::new(config.visibility.clone()),
            debug: {
                // The overlay configuration and the frame dumper are independent:
                // turning dumping on must not turn the panels back on.
                let mut debug = DebugSystem::new(config.debug.clone());
                if let Some((dir, format)) = &config.dump {
                    debug.set_dumper(Some(noxel_debug::FrameDumper::new(dir, *format)?));
                }
                debug
            },
            camera,
            clock,
            input: InputState::new(),
            assets,
            frame: 0,
            elapsed: 0.0,
            player: None,
            view: CameraView::default(),
            focus: Vec3::ZERO,
            visible: noxel_render::renderer::VisibleSet::default(),
            tile_set,
            prefabs,
        };

        Ok(Self {
            context,
            plugins: PluginRegistry::new(),
            framebuffer: Framebuffer::new(config.internal.0, config.internal.1),
            raster: RasterRenderer::new(config.internal.0, config.internal.1),
            raytracer: RayTracer::new(config.internal.0, config.internal.1),
            config,
        })
    }

    /// The shared context.
    #[must_use]
    pub fn context(&self) -> &AppContext {
        &self.context
    }

    /// The shared context, mutably.
    pub fn context_mut(&mut self) -> &mut AppContext {
        &mut self.context
    }

    /// The debug system.
    #[must_use]
    pub fn debug(&self) -> &DebugSystem {
        &self.context.debug
    }

    /// The debug system, mutably.
    pub fn debug_mut(&mut self) -> &mut DebugSystem {
        &mut self.context.debug
    }

    /// The scene.
    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.context.scene
    }

    /// The scene, mutably.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.context.scene
    }

    /// The physics world.
    #[must_use]
    pub fn physics(&self) -> &PhysicsWorld {
        &self.context.physics
    }

    /// The physics world, mutably.
    pub fn physics_mut(&mut self) -> &mut PhysicsWorld {
        &mut self.context.physics
    }

    /// The camera rig.
    #[must_use]
    pub fn camera(&self) -> &TopDownCamera {
        &self.context.camera
    }

    /// The camera rig, mutably.
    pub fn camera_mut(&mut self) -> &mut TopDownCamera {
        &mut self.context.camera
    }

    /// The input state.
    #[must_use]
    pub fn input(&self) -> &InputState {
        &self.context.input
    }

    /// The input state, mutably.
    pub fn input_mut(&mut self) -> &mut InputState {
        &mut self.context.input
    }

    /// The chunk streamer.
    #[must_use]
    pub fn streamer(&self) -> &WorldStreamer {
        &self.context.streamer
    }

    /// The chunk streamer, mutably.
    pub fn streamer_mut(&mut self) -> &mut WorldStreamer {
        &mut self.context.streamer
    }

    /// Adds a plugin and runs its `build` hook immediately.
    pub fn add_plugin(&mut self, plugin: impl crate::plugin::Plugin) -> usize {
        let index = self.plugins.push(Box::new(plugin));
        self.with_registry(|registry, app| registry.build_index(app, index));
        index
    }

    /// Runs `f` with the plugin registry detached from the app.
    ///
    /// A plugin hook takes `&mut App`, and the registry is a field of `App`, so
    /// calling a hook directly would be a double mutable borrow. Taking the
    /// registry out for the duration is the standard fix, and it costs one
    /// `Vec` move per hook call.
    fn with_registry<R>(&mut self, f: impl FnOnce(&mut PluginRegistry, &mut App) -> R) -> R {
        let mut registry = std::mem::take(&mut self.plugins);
        let result = f(&mut registry, self);
        self.plugins = registry;
        result
    }

    /// Runs the fixed-step update phase.
    ///
    /// Returns the number of substeps that ran. Time beyond
    /// `max_substeps * fixed_dt` is dropped rather than buffered: a frame that
    /// took a second must not turn into sixty physics steps.
    pub fn fixed_update(&mut self, frame_dt: f32) -> u32 {
        self.context.clock.begin_frame(frame_dt);
        // `GameClock::step` returns false once the substep budget is spent, so
        // this loop cannot spiral after a hitch.
        while self.context.clock.step() {
            let dt = self.context.clock.fixed_dt();
            self.with_registry(|plugins, app| plugins.update(app, dt));
            self.context.physics.step(dt);
        }
        self.context.clock.substeps_this_frame()
    }

    /// Runs the per-frame phase: frame hooks, camera, streaming and culling.
    pub fn frame_update(&mut self, frame_dt: f32) {
        self.with_registry(|plugins, app| plugins.frame(app, frame_dt));
        self.with_registry(|plugins, app| plugins.pre_cull(app, frame_dt));

        let aspect = self.config.aspect();
        let focus = self.context.focus;
        let velocity = Vec3::ZERO;
        self.context
            .camera
            .follow_with_velocity(focus, velocity, frame_dt);
        self.context.view = self.context.camera.view(aspect);

        let stats = self.context.streamer.update(focus);
        self.context
            .debug
            .record_counter("chunks", stats.loaded as f32);
        self.context
            .debug
            .record_counter("chunk gen", stats.generated_this_update as f32);
        self.context
            .debug
            .record_counter("chunk mem KB", (stats.memory_bytes / 1024) as f32);

        let input = VisibilityInput::new(&self.context.scene, &self.context.view, frame_dt, focus);
        self.context.visible = self.context.visibility.update(&input, &self.config.render);
        let counts = self.context.visible.counts;
        self.context.debug.record_culling(&counts);
    }

    /// Renders the frame into the framebuffer.
    pub fn render(&mut self) -> noxel_render::renderer::RenderStats {
        let mut settings = self.config.render.clone();
        settings.mode = self.config.mode;
        let stats = match self.config.mode {
            ShadingMode::Raytrace | ShadingMode::Hybrid => self.raytracer.render(
                &self.context.scene,
                &self.context.view,
                Some(&self.context.visible),
                &mut self.framebuffer,
                &settings,
            ),
            ShadingMode::Raster => self.raster.render(
                &self.context.scene,
                &self.context.view,
                Some(&self.context.visible),
                &mut self.framebuffer,
                &settings,
            ),
        };
        self.context.debug.record_render(&stats);
        stats
    }

    /// Draws plugin overlays and the debug panels.
    pub fn draw_overlays(&mut self) {
        let mut framebuffer = std::mem::take(&mut self.framebuffer);
        self.with_registry(|plugins, app| plugins.draw(app, &mut framebuffer));
        self.framebuffer = framebuffer;
        let view = self.context.view;
        self.context.debug.draw(&mut self.framebuffer, Some(&view));
    }

    /// Resolves the framebuffer to an sRGB image.
    ///
    /// The tone curve is derived from the mode the frame was actually rendered
    /// with, not from `config.render.mode`: a caller can switch modes at runtime
    /// (`AppConfig::with_mode`) without touching the render settings, and getting
    /// this wrong means a ray-traced frame (which carries HDR radiance above 1.0)
    /// resolves with no curve at all.
    #[must_use]
    pub fn resolve(&self) -> noxel_asset::image::Image {
        let mut settings = self.config.render.clone();
        settings.mode = self.config.mode;
        self.framebuffer.resolve(&settings.resolve())
    }

    /// A mutable reference to the internal renderer.
    pub fn renderer_mut(&mut self) -> &mut dyn Renderer {
        match self.config.mode {
            ShadingMode::Raster => &mut self.raster,
            _ => &mut self.raytracer,
        }
    }

    /// The renderer's name.
    #[must_use]
    pub fn renderer_name(&self) -> &'static str {
        match self.config.mode {
            ShadingMode::Raster => self.raster.name(),
            _ => self.raytracer.name(),
        }
    }

    /// Releases cached GPU-side or BVH-side data.
    pub fn release_cached(&mut self) {
        self.raster.release_cached();
        self.raytracer.release_cached();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.with_registry(|plugins, app| plugins.shutdown(app));
    }
}

impl core::fmt::Debug for App {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("App")
            .field("frame", &self.context.frame)
            .field("mode", &self.config.mode)
            .field("plugins", &self.plugins.names())
            .field("instances", &self.context.scene.instance_count())
            .finish()
    }
}

/// Loads the tile set and prefabs from an asset root.
///
/// A missing asset tree is **not** an error: the world generator falls back to
/// procedural content and the engine still runs. That is what makes
/// `App::new(AppConfig::headless())` usable in a test and what makes a fresh
/// checkout work before `noxel-gen` has run.
fn load_assets(assets: &mut AssetDb) -> (Arc<TileSet>, Vec<Arc<Prefab>>) {
    let (tile_set_paths, prefab_paths) = match assets.manifest("manifest.json") {
        Ok(manifest) => (manifest.tile_sets.clone(), manifest.prefabs.clone()),
        Err(_) => (vec!["tilesets/terrain.json".to_string()], Vec::new()),
    };
    // Every tile set in the manifest is **merged** into one, because they share
    // a single global tile-id space: the terrain set owns 0..16 and the building
    // set owns 16..28, and a prefab's voxel ids have to resolve through the same
    // lookup the world uses. A tile that names no texture of its own inherits its
    // set's.
    let mut merged = TileSet::default();
    for path in &tile_set_paths {
        let Ok(set) = assets.tile_set(path) else {
            continue;
        };
        if merged.tiles.is_empty() {
            merged.name = set.name.clone();
            merged.tile_size = set.tile_size;
        }
        for tile in &set.tiles {
            let mut tile = tile.clone();
            if tile.texture.is_empty() {
                tile.texture = set.texture.clone();
            }
            merged.tiles.push(tile);
        }
    }
    merged.tiles.sort_by_key(|tile| tile.id);
    merged.tiles.dedup_by_key(|tile| tile.id);
    let tile_set = Arc::new(merged);
    let prefabs = prefab_paths
        .iter()
        .filter_map(|path| assets.prefab(path).ok())
        .collect();
    (tile_set, prefabs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(AppConfig::headless()).unwrap()
    }

    #[test]
    fn empty_viewport_is_rejected() {
        // `with_internal` clamps, so the zero has to be written directly.
        let config = AppConfig {
            internal: (0, 90),
            ..AppConfig::headless()
        };
        assert!(matches!(App::new(config), Err(AppError::EmptyViewport)));
        let config = AppConfig {
            internal: (160, 0),
            ..AppConfig::headless()
        };
        assert!(matches!(App::new(config), Err(AppError::EmptyViewport)));
    }

    #[test]
    fn headless_app_starts() {
        let app = app();
        assert_eq!(app.context.frame, 0);
        assert_eq!(app.context.scene.instance_count(), 0);
        assert_eq!(app.framebuffer.width(), 160);
        assert_eq!(app.framebuffer.height(), 90);
    }

    #[test]
    fn config_aspect_is_computed() {
        assert!((AppConfig::headless().aspect() - 160.0 / 90.0).abs() < 1e-6);
        let zero = AppConfig {
            internal: (10, 0),
            ..AppConfig::headless()
        };
        assert_eq!(zero.aspect(), 1.0);
    }

    #[test]
    fn input_tracks_held_and_edge_states() {
        let mut input = InputState::new();
        input.press(65);
        assert!(input.is_held(65));
        assert!(input.was_pressed(65));
        assert!(!input.was_released(65));
        input.end_frame();
        assert!(input.is_held(65), "held survives the frame boundary");
        assert!(!input.was_pressed(65), "the edge does not");
        input.release(65);
        assert!(!input.is_held(65));
        assert!(input.was_released(65));
        // Arrow keys: up, down, left, right.
        input.press(3); // right
        input.press(1); // down
        assert_eq!(input.movement_axis(0, 1, 2, 3), (1.0, 1.0));
    }

    #[test]
    fn input_movement_axis_cancels_opposites() {
        let mut input = InputState::new();
        input.press(0);
        input.press(1);
        input.press(2);
        input.press(3);
        assert_eq!(input.movement_axis(0, 1, 2, 3), (0.0, 0.0));
    }

    #[test]
    fn input_release_all_clears_everything() {
        let mut input = InputState::new();
        input.press(10);
        input.mouse_buttons[0] = true;
        input.release_all();
        assert!(!input.is_held(10));
        assert!(input.was_released(10));
        assert_eq!(input.mouse_buttons, [false; 3]);
    }

    #[test]
    fn input_press_twice_does_not_duplicate() {
        let mut input = InputState::new();
        input.press(5);
        input.press(5);
        assert_eq!(input.held.len(), 1);
        assert_eq!(input.pressed.len(), 1);
    }

    #[test]
    fn fixed_update_runs_at_least_one_step() {
        let mut app = app();
        let steps = app.fixed_update(1.0 / 60.0);
        assert!(steps >= 1, "{steps}");
    }

    #[test]
    fn fixed_update_drops_excess_time() {
        let mut app = app();
        // A one-second hitch must not produce sixty physics steps.
        let steps = app.fixed_update(1.0);
        assert!(steps <= app.config.max_substeps, "{steps}");
    }

    #[test]
    fn frame_update_produces_a_view_and_a_visible_set() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        assert!(app.context.view.view_projection.is_finite());
        assert_eq!(app.context.visible.items.len(), 0);
    }

    #[test]
    fn render_produces_a_frame() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        let stats = app.render();
        assert_eq!(stats.mode, ShadingMode::Raster);
        assert!(app.framebuffer.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn hybrid_and_raytrace_modes_are_selectable() {
        for mode in [ShadingMode::Hybrid, ShadingMode::Raytrace] {
            let mut app = App::new(AppConfig::headless().with_mode(mode)).unwrap();
            app.frame_update(1.0 / 60.0);
            let stats = app.render();
            assert_eq!(stats.mode, mode);
        }
    }

    #[test]
    fn renderer_name_reflects_the_mode() {
        let app = app();
        assert_eq!(app.renderer_name(), "software-raster");
        let rt = App::new(AppConfig::headless().with_mode(ShadingMode::Raytrace)).unwrap();
        assert_eq!(rt.renderer_name(), "ray-tracer");
    }

    #[test]
    fn overlays_do_not_panic() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        app.render();
        app.draw_overlays();
    }

    #[test]
    fn resolve_produces_an_image() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        app.render();
        let image = app.resolve();
        assert_eq!((image.width, image.height), (160, 90));
    }

    #[test]
    fn plugins_receive_the_frame() {
        let mut app = app();
        app.add_plugin(crate::plugin::FrameCounter::default());
        assert_eq!(app.plugins.names(), vec!["frame-counter"]);
        app.fixed_update(1.0 / 60.0);
        app.frame_update(1.0 / 60.0);
        assert!(app.context.clock.tick() > 0);
    }

    #[test]
    fn release_cached_keeps_the_app_usable() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        app.render();
        app.release_cached();
        app.render();
    }

    #[test]
    fn debug_reports_the_frame() {
        let mut app = app();
        app.frame_update(1.0 / 60.0);
        app.render();
        app.debug_mut().end_frame(16.0);
        assert!(app.debug().report().contains("frames: 1"));
    }

    #[test]
    fn debug_impl_reports_state() {
        let app = app();
        let text = format!("{app:?}");
        assert!(text.contains("App"));
    }

    #[test]
    fn player_position_defaults_to_the_focus() {
        let mut app = app();
        app.context.focus = Vec3::new(4.0, 0.0, 9.0);
        assert!(
            app.context
                .player_position()
                .approx_eq(Vec3::new(4.0, 0.0, 9.0), 1e-6)
        );
    }

    #[test]
    fn view_bounds_are_a_sane_region() {
        let app = app();
        let bounds = app.context.view_bounds();
        assert!(bounds.min.x < bounds.max.x && bounds.min.z < bounds.max.z);
        assert!(app.context.keep_bounds().contains_point(Vec3::ZERO));
    }

    #[test]
    fn error_display_is_human_readable() {
        let e = AppError::EmptyViewport;
        assert!(format!("{e}").contains("1x1"));
        let io = AppError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(format!("{io}").contains("i/o"));
    }
}
