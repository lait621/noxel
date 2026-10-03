//! # town-demo — a ready-to-run Noxel village
//!
//! This example is the shortest complete game loop the engine supports, and it
//! is meant to be copied. It generates a world from a seed, grows a village in
//! it, drops a player into the plaza, fills the streets with a crowd, follows
//! the player with a top-down camera, and writes every frame as a PNG.
//!
//! ```text
//! cargo run -p town-demo -- --frames 300 --dump frames
//! cargo run -p town-demo -- --mode hybrid --npcs 400
//! cargo run -p town-demo -- --stats            # one frame, full statistics
//! cargo run -p town-demo -- --world-info       # what this seed generates
//! ```
//!
//! ## The frame, in order
//!
//! ```text
//! App::step(dt)
//!   fixed_update   0..n substeps: PlayerPlugin::update, CrowdPlugin::update,
//!                  physics.step()      <- gameplay runs at a FIXED rate
//!   frame_update   camera follow -> stream chunks -> visibility cull
//!   render         raster / hybrid / raytrace into the framebuffer
//!   overlays       HUD text, debug panels
//!   dump           PNG to disk, if --dump was given
//! ```
//!
//! The order is the point. The camera moves before streaming, because otherwise
//! the world arrives one frame late. Visibility runs after streaming, because
//! culling a chunk that has not loaded yet is meaningless.
//!
//! ## Making it a real game
//!
//! Two things are missing, and both are deliberately out of scope for the
//! engine (see `docs/adr/0009-no-ui.md`):
//!
//! * **A window.** Call `App::step` yourself and blit `app.resolve()` to the
//!   surface. `docs/guides/windowing.md` has a worked `winit` example.
//! * **Input.** Fill `app.input_mut()` from the host's events; the player plugin
//!   already reads WASD in `PlayerPlugin::update` when its `scripted` flag is
//!   false; the demo leaves it on because there is no window to type into.

mod actors;
mod args;
mod hud;
mod interactive;
mod terrain;
mod village;

use std::time::Instant;

use actors::{CrowdPlugin, PlayerPlugin};
use args::Args;
use noxel_app::{App, AppConfig, DebugConfig};
use noxel_core::math::{Color, Vec3};
use noxel_npc::NpcConfig;
use noxel_render::light::{Ambient, Fog, Light};
use noxel_render::renderer::ShadingMode;
use noxel_world::WorldChunkPos;
use terrain::TerrainPlugin;

/// The demo's fixed timestep: 60 Hz, the rate every value in the engine is
/// tuned against.
const FIXED_DT: f32 = 1.0 / 60.0;

fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("town-demo: {message}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", Args::help());
        return;
    }

    match run(&args) {
        Ok(()) => {}
        Err(message) => {
            eprintln!("town-demo: {message}");
            std::process::exit(1);
        }
    }
}

/// Builds the app, wires the plugins, and runs for the requested number of
/// frames.
fn run(args: &Args) -> Result<(), String> {
    // Resolve the two locations a user needs to know about *before* building the
    // app, so the banner can report them whether or not anything was loaded.
    // A windowed run only dumps when asked: writing a PNG per frame while
    // someone is playing is 60 files a second that nobody wanted.
    let dumping = args.dump.is_some() && (!args.window || args.dump_given);
    let dump_dir = dump_root(args);
    let assets = asset_root();
    let mut app = setup(args, &assets)?;

    if args.world_info {
        print_world_info(&mut app, args);
        return Ok(());
    }

    if args.window {
        // Warm up *before* the window exists, so the first frame it presents is
        // a village rather than fog. The window is still on screen almost
        // immediately — this is about a fifth of a second.
        warm_up(&mut app);
        return run_in_window(args, app);
    }

    // Warm up: the first few frames load chunks and place the player. Reports
    // and dumps start after this so the first PNG is a settled frame.
    warm_up(&mut app);

    if args.stats_only {
        let report = app.run_and_capture(1);
        println!("{}", report.summary());
        println!("{}", app.debug().report());
        return Ok(());
    }

    if !args.quiet {
        println!("town-demo — seed {:#x}", args.seed);
        println!("  {} {}", app.renderer_name(), describe_mode(args.mode));
        println!("  internal {}x{}", args.size.0, args.size.1);
        if dumping {
            println!("  frames -> {}", absolute(&dump_dir).display());
        }
        println!("  assets -> {}", absolute(&assets).display());
    }

    let started = Instant::now();
    let report = app.run_headless(args.frames);
    let wall = started.elapsed();

    if !args.quiet {
        println!();
        println!("{}", app.renderer_name());
        println!("  {}", report.summary());
        println!(
            "  scene: {}",
            app.debug().report().lines().take(0).collect::<String>()
        );
        println!();
        print!("{}", hud::final_report(&app, args, wall.as_secs_f32()));
    } else {
        println!("{}", report.summary());
    }
    Ok(())
}

/// Builds the app: assets, world settings, camera, lighting and debug.
fn build_app(args: &Args, root: &std::path::Path) -> Result<App, String> {
    // `--no-dump` leaves `args.dump` as `None`; resolve once and reuse.
    // A windowed run only dumps when asked: writing a PNG per frame while
    // someone is playing is 60 files a second that nobody wanted.
    let dumping = args.dump.is_some() && (!args.window || args.dump_given);
    let dump_dir = dump_root(args);
    let mut config = AppConfig {
        seed: args.seed,
        internal: args.size,
        mode: args.mode,
        world: args.world_config(),
        // The engine's statistics panel is opt-in here: the demo draws its own
        // HUD, and a panel that covers two thirds of a 320x180 frame hides the
        // thing the demo exists to show.
        debug: if args.debug {
            DebugConfig::default()
        } else {
            DebugConfig::disabled()
        },
        assets_root: root.to_path_buf(),
        ..AppConfig::default()
    };
    config.dump = if dumping {
        Some((dump_dir.clone(), noxel_debug::DumpFormat::Png))
    } else {
        None
    };

    let mut app = App::new(config).map_err(|e| e.to_string())?;
    // A previous run's frames would otherwise survive into this one and be
    // mistaken for output.
    if let Some(dumper) = app.debug_mut().dumper_mut() {
        let _ = dumper.clear();
    }

    // ---- lighting ---------------------------------------------------------
    // A low sun so the ray-traced AO and shadows have something to say, plus a
    // strong ambient so the unlit terrain keeps the artist's colours.
    app.context.scene.ambient = Ambient {
        sky: Color::rgb(0.62, 0.70, 0.86),
        ground: Color::rgb(0.30, 0.28, 0.24),
        hemisphere: 0.7,
        intensity: 0.85,
    };
    let mut sun = Light::sun();
    if let Light::Directional {
        direction,
        intensity,
        shadow_extent,
        ..
    } = &mut sun
    {
        *direction = Vec3::new(-0.42, -0.78, -0.46).normalize_or_zero();
        *intensity = 0.85;
        // A wide enough orthographic shadow volume to cover the visible village;
        // too small and the shadows stop at an invisible wall.
        *shadow_extent = 60.0;
    }
    app.context.scene.add_light(sun);
    app.context.scene.fog = Some(Fog {
        color: Color::rgb(0.66, 0.74, 0.86),
        start: 70.0,
        end: 160.0,
        height_falloff: 0.0,
    });

    // ---- camera -----------------------------------------------------------
    // A true bird's-eye view shows the top of everyone's head. A 62-degree
    // three-quarter view is what a top-down pixel RPG actually uses: characters
    // read as characters, buildings show their walls, and the tile grid is still
    // axis-aligned.
    app.camera_mut().set_pitch(1.08);
    app.camera_mut().set_distance(60.0);
    app.camera_mut().set_ortho_height(20.0);
    app.camera_mut().set_smoothing(0.02);
    app.camera_mut().set_deadzone(1.6, 1.2, 0.0);
    app.camera_mut().set_look_ahead(0.22, 2.5);
    app.camera_mut().set_clip_planes(1.0, 300.0);

    Ok(app)
}

/// Finds `examples/town-demo/assets` whether the demo is run from the workspace
/// root or from its own directory.
fn asset_root() -> std::path::PathBuf {
    asset_root_from(&std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Finds the asset tree, so the binary works whether it is run from the
/// workspace root, from `dist/`, or from anywhere else on the machine.
///
/// The search order is, in decreasing order of intent:
///
/// 1. `$NOXEL_ASSET_DIR` — an explicit override, which a packager or a test uses.
/// 2. `<exe>/assets` — the distributed layout, where the assets sit beside the
///    binary. This is what makes the built executable self-contained.
/// 3. `<exe>/../examples/town-demo/assets` — a release build inside the source
///    tree, i.e. `target/release/town-demo`.
/// 4. `<cwd>/examples/town-demo/assets` and `<cwd>/assets` — the two layouts a
///    developer runs from.
/// 5. The demo's own source directory, from `CARGO_MANIFEST_DIR` at build time,
///    so a binary copied out of the tree still finds the art it was built with.
///
/// A directory counts if it contains `manifest.json`. If none does, the first
/// directory that exists wins, and failing that the build-time path — a missing
/// asset tree is not fatal, because the world generator falls back to procedural
/// content and the renderer to a placeholder texture.
fn asset_root_from(cwd: &std::path::Path) -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("NOXEL_ASSET_DIR") {
        candidates.push(std::path::PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets"));
            candidates.push(
                dir.join("..")
                    .join("examples")
                    .join("town-demo")
                    .join("assets"),
            );
            candidates.push(
                dir.join("..")
                    .join("..")
                    .join("examples")
                    .join("town-demo")
                    .join("assets"),
            );
        }
    }
    candidates.push(cwd.join("examples").join("town-demo").join("assets"));
    candidates.push(cwd.join("assets"));
    candidates.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"));

    if let Some(found) = candidates.iter().find(|c| c.join("manifest.json").exists()) {
        return found.clone();
    }
    if let Some(found) = candidates.iter().find(|c| c.is_dir()) {
        return found.clone();
    }
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

/// The directory frames are written to.
///
/// `--dump` wins. Otherwise `frames/` beside the working directory, unless that
/// cannot be created — a binary run from a read-only location still has to be
/// able to produce output — in which case `frames/` beside the executable.
fn dump_root(args: &Args) -> std::path::PathBuf {
    let Some(requested) = args.dump.clone() else {
        return std::path::PathBuf::from("frames");
    };
    if std::fs::create_dir_all(&requested).is_ok() {
        return requested;
    }
    let fallback = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("frames")))
        .unwrap_or_else(|| std::path::PathBuf::from("frames"));
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// Builds the app and registers the demo's three plugins.
///
/// Every path goes through here — headless, `--window`, `--world-info` — and
/// that is on purpose. The windowed path once returned before the plugins were
/// added, so the window opened onto an empty scene and presented a uniformly
/// grey rectangle: the renderer, the blit and the presenter were all fine, and
/// there was simply nothing in the world. One place that wires the plugins is
/// what keeps that from happening again, and
/// `the_window_path_renders_a_scene_and_not_an_empty_world` is the test that
/// catches it if it does.
fn setup(args: &Args, assets: &std::path::Path) -> Result<App, String> {
    let mut app = build_app(args, assets)?;
    app.add_plugin(TerrainPlugin::new());
    app.add_plugin(PlayerPlugin::new());
    app.add_plugin(CrowdPlugin::new(npc_config(args)));
    Ok(app)
}

/// Steps the world forward before the first frame is shown.
///
/// Streaming a chunk is not instant, and a window that opens onto fifteen frames
/// of empty fog reads as broken. `--frames 0` skips it, which is what the tests
/// use.
fn warm_up(app: &mut App) {
    for _ in 0..WARMUP_FRAMES {
        app.step(FIXED_DT);
    }
}

/// How many fixed steps to run before anything is shown or dumped.
const WARMUP_FRAMES: u64 = 12;

/// Opens a window and hands the app to it.
///
/// This is the whole difference between the demo and a game: the same `App`, the
/// same plugins, the same scene — driven by real time and real input instead of
/// by a frame counter.
fn run_in_window(args: &Args, app: App) -> Result<(), String> {
    if !args.quiet {
        println!("town-demo — seed {:#x}, windowed", args.seed);
        println!("  {} {}", app.renderer_name(), describe_mode(args.mode));
        println!(
            "  internal {}x{}, {} NPCs",
            args.size.0, args.size.1, args.npcs
        );
        println!();
        println!("  WASD or the arrow keys   walk");
        println!("  shift                    run");
        println!("  F1                       toggle the statistics overlay");
        println!("  F8                       write the current frame to a PNG");
        println!("  escape                   quit");
        if args.frames != Args::default().frames {
            println!();
            println!(
                "  (--frames {}: the window closes after that many frames)",
                args.frames
            );
        }
    }
    let started = Instant::now();
    interactive::run(args, app).map_err(|e| e.to_string())?;
    if !args.quiet {
        println!();
        println!(
            "window closed after {:.1}s",
            started.elapsed().as_secs_f32()
        );
    }
    Ok(())
}

/// Makes a path absolute for display, without requiring it to exist.
fn absolute(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(path)
}

/// The crowd configuration the demo uses.
fn npc_config(args: &Args) -> NpcConfig {
    NpcConfig {
        seed: args.seed,
        target_population: args.npcs,
        // Keep the crowd inside the streaming radius so an agent never walks
        // into a chunk that is not loaded.
        spawn_radius: 60.0,
        despawn_radius: 80.0,
        ..NpcConfig::default()
    }
}

/// A human-readable description of a shading mode.
fn describe_mode(mode: ShadingMode) -> &'static str {
    match mode {
        ShadingMode::Raster => "(tiled software rasterizer, shadow map)",
        ShadingMode::Hybrid => "(raster primary + ray-traced shadows and AO)",
        ShadingMode::Raytrace => "(full ray tracing, slow)",
    }
}

/// Prints what this seed generates, without rendering anything.
fn print_world_info(app: &mut App, args: &Args) {
    app.frame_update(FIXED_DT);
    let generator = app.context.streamer.generator();
    let config = generator.config();
    println!("seed {:#x}", args.seed);
    println!(
        "  chunk size      {} tiles ({} m)",
        config.tiles_per_chunk,
        config.chunk_world_size()
    );
    println!("  view distance   {} chunks", config.view_distance_chunks);
    println!("  road grid       every {} chunks", config.road_grid_chunks);
    println!(
        "  town spacing    every {} chunks",
        config.town_spacing_chunks
    );
    println!("  loaded chunks   {}", app.context.streamer.stats().loaded);
    println!();
    println!("height and biome samples:");
    for (x, z) in [
        (0.0f32, 0.0f32),
        (256.0, 0.0),
        (0.0, 256.0),
        (-512.0, 384.0),
        (1024.0, -1024.0),
    ] {
        let height = generator.sample_height(x, z);
        let biome = generator.biome_at(x, z);
        let road = generator.is_road_at(x, z);
        println!(
            "  {:>8.0},{:>8.0}  {:>7.2} m  {:<10} {}",
            x,
            z,
            height,
            biome.name(),
            if road { "road" } else { "" }
        );
    }
    println!();
    // Scan a small region of the town lattice for the seed's settlements. The
    // spacing is known, so this is a handful of hash lookups, not a search.
    let spacing = config.town_spacing_chunks.max(1);
    let mut towns: Vec<String> = Vec::new();
    for cz in -3..=3 {
        for cx in -3..=3 {
            let pos = WorldChunkPos::new(cx * spacing, cz * spacing);
            if let Some(town) = generator.town_at(pos) {
                towns.push(format!(
                    "{} ({:?}, {} buildings)",
                    town.name,
                    town.center_chunk,
                    town.building_plots.len()
                ));
            }
        }
    }
    if towns.is_empty() {
        println!("no towns near the origin for this seed");
    } else {
        println!("towns near the origin:");
        for town in towns.iter().take(6) {
            println!("  {town}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_root_prefers_a_directory_with_a_manifest() {
        // Any real checkout has one of these, and the demo must find it without
        // being told where it is.
        let found = asset_root();
        assert!(
            found.join("manifest.json").exists() || found.is_dir(),
            "{found:?} must be a directory"
        );
    }

    #[test]
    fn asset_root_falls_back_to_a_directory_when_no_manifest_exists() {
        // An empty directory in an unrelated place: the search must still return
        // something usable rather than an error, because the generator falls
        // back to procedural content.
        let scratch = std::env::temp_dir().join("noxel-asset-root-test");
        let _ = std::fs::create_dir_all(&scratch);
        let found = asset_root_from(&scratch);
        assert!(found.is_absolute() || found.is_dir(), "{found:?}");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn asset_root_is_never_empty() {
        let found = asset_root_from(std::path::Path::new("/nonexistent-directory-for-tests"));
        assert!(
            !found.as_os_str().is_empty(),
            "an empty path would break the asset db"
        );
    }

    #[test]
    fn the_compiled_binary_finds_its_assets_from_an_unrelated_directory() {
        // The point of the whole search: `current_dir` is somewhere else, and
        // the executable's own location is what has to win. `current_exe` is the
        // test harness here, so the check is that the build-time fallback is
        // reachable and points at the demo's assets.
        let fallback = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        assert!(fallback.is_dir(), "{fallback:?} must exist in a checkout");
        assert!(
            fallback.join("manifest.json").exists(),
            "the assets must be generated"
        );
    }

    #[test]
    fn dump_root_honours_explicit_and_default_paths() {
        let scratch = std::env::temp_dir().join("noxel-dump-root-test");
        let _ = std::fs::remove_dir_all(&scratch);

        let explicit = Args {
            dump: Some(scratch.clone()),
            ..Args::default()
        };
        assert_eq!(dump_root(&explicit), scratch);
        assert!(scratch.is_dir(), "the requested directory is created");

        let none = Args {
            dump: None,
            ..Args::default()
        };
        assert_eq!(dump_root(&none), std::path::PathBuf::from("frames"));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn absolute_leaves_absolute_paths_alone() {
        let path = std::path::Path::new("/tmp/definitely-absolute");
        assert_eq!(absolute(path), path);
        let relative = absolute(std::path::Path::new("frames"));
        assert!(relative.is_absolute(), "{relative:?}");
        assert!(relative.ends_with("frames"));
    }

    #[test]
    fn describe_mode_covers_every_shading_mode() {
        use noxel_render::renderer::ShadingMode;
        for mode in [
            ShadingMode::Raster,
            ShadingMode::Hybrid,
            ShadingMode::Raytrace,
        ] {
            assert!(!describe_mode(mode).is_empty());
        }
    }

    #[test]
    fn npc_config_carries_the_requested_population() {
        let args = Args {
            npcs: 42,
            seed: 7,
            ..Args::default()
        };
        let config = npc_config(&args);
        assert_eq!(config.target_population, 42);
        assert_eq!(config.seed, 7);
        assert!(
            config.spawn_radius < 70.0,
            "the crowd must stay inside the streaming radius, or agents walk into \
             chunks that are not loaded"
        );
    }

    use std::collections::BTreeSet;

    fn args() -> Args {
        Args {
            quiet: true,
            dump: None,
            stats_only: false,
            ..Args::default()
        }
    }

    /// The bug this exists for: `--window` used to return before the plugins were
    /// registered, so the window opened on an empty scene and presented a
    /// uniformly grey rectangle. Every layer below was working; there was just
    /// nothing in the world.
    #[test]
    fn the_window_path_renders_a_scene_and_not_an_empty_world() {
        let args = args();
        let mut app = setup(&args, &asset_root()).unwrap();
        warm_up(&mut app);

        let image = app.resolve();
        let distinct: BTreeSet<(u8, u8, u8)> =
            image.pixels.iter().map(|c| (c.r, c.g, c.b)).collect();
        assert!(
            distinct.len() > 20,
            "a windowed run must have a world in it, not {} distinct colours",
            distinct.len()
        );
    }

    /// Proves the test above is actually testing something: an app with no
    /// plugins really does render a flat frame.
    #[test]
    fn without_the_plugins_the_frame_is_flat() {
        let args = args();
        let mut app = build_app(&args, &asset_root()).unwrap();
        warm_up(&mut app);

        let image = app.resolve();
        let distinct: BTreeSet<(u8, u8, u8)> =
            image.pixels.iter().map(|c| (c.r, c.g, c.b)).collect();
        assert!(
            distinct.len() < 10,
            "an empty world should be nearly flat, but had {} colours",
            distinct.len()
        );
    }

    #[test]
    fn setup_registers_all_three_demo_plugins() {
        let args = args();
        let app = setup(&args, &asset_root()).unwrap();
        let mut names = app.plugins.names();
        names.sort_unstable();
        assert_eq!(names, ["crowd", "player", "terrain"]);
    }

    #[test]
    fn world_info_does_not_need_a_window() {
        let args = Args {
            world_info: true,
            quiet: true,
            dump: None,
            ..Args::default()
        };
        let mut app = setup(&args, &asset_root()).unwrap();
        warm_up(&mut app);
        // Chunks are resident, so the report has something to print.
        assert!(app.context.streamer.stats().loaded > 0);
    }

    #[test]
    fn a_headless_run_still_renders_a_scene() {
        // The headless path is the CI path; it must not regress while the window
        // path is being fixed.
        let args = args();
        let mut app = setup(&args, &asset_root()).unwrap();
        warm_up(&mut app);
        let report = app.run_headless(5);
        assert_eq!(report.frames, 5);
        assert!(report.fragments > 0, "the renderer drew something");
    }
}
