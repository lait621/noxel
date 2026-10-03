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
    let mut app = build_app(args)?;

    if args.world_info {
        print_world_info(&mut app, args);
        return Ok(());
    }

    let terrain_plugin = TerrainPlugin::new();
    let player_plugin = PlayerPlugin::new();
    let crowd_plugin = CrowdPlugin::new(npc_config(args));

    app.add_plugin(terrain_plugin);
    app.add_plugin(player_plugin);
    app.add_plugin(crowd_plugin);

    // Warm up: the first few frames load chunks and place the player. Reports
    // and dumps start after this so the first PNG is a settled frame.
    let warmup = 12u64;
    for _ in 0..warmup {
        app.step(FIXED_DT);
    }

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
        if let Some(dir) = &args.dump {
            println!("  frames -> {}", dir.display());
        }
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
fn build_app(args: &Args) -> Result<App, String> {
    let root = asset_root();
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
        assets_root: root,
        ..AppConfig::default()
    };
    if let Some(dir) = &args.dump {
        config.dump = Some((dir.clone(), noxel_debug::DumpFormat::Png));
    } else {
        config.dump = None;
    }

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
    let candidates = [
        std::path::PathBuf::from("examples/town-demo/assets"),
        std::path::PathBuf::from("assets"),
    ];
    for candidate in &candidates {
        if candidate.join("manifest.json").exists() {
            return candidate.clone();
        }
    }
    candidates[0].clone()
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
