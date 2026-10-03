//! Command-line parsing.
//!
//! Hand-written rather than pulled from a crate: the demo has nine flags, the
//! engine has no dependencies, and a hand-written parser can produce a helpful
//! error with the exact flag that was wrong.

use std::path::PathBuf;

use noxel_render::renderer::ShadingMode;
use noxel_world::WorldConfig;

/// What the demo should do.
#[derive(Clone, Debug)]
pub struct Args {
    /// World seed.
    pub seed: u64,
    /// Frames to simulate and render.
    pub frames: u64,
    /// Internal render size.
    pub size: (u32, u32),
    /// Shading mode.
    pub mode: ShadingMode,
    /// Where to write frames, if anywhere.
    pub dump: Option<PathBuf>,
    /// Target NPC population.
    pub npcs: usize,
    /// Print the report and nothing else.
    pub quiet: bool,
    /// Print a one-frame statistics dump and exit.
    pub stats_only: bool,
    /// Print the help text and exit.
    pub help: bool,
    /// Print the seed and the world's statistics and exit.
    pub world_info: bool,
    /// Draw the full engine statistics panel over the frame.
    pub debug: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            seed: 0x4E4F_5845,
            frames: 600,
            size: (320, 180),
            mode: ShadingMode::Raster,
            dump: Some(PathBuf::from("frames")),
            npcs: 240,
            quiet: false,
            stats_only: false,
            help: false,
            world_info: false,
            debug: false,
        }
    }
}

impl Args {
    /// Parses the process arguments, ignoring the program name.
    ///
    /// # Errors
    /// Returns a message naming the offending flag.
    pub fn parse<I, S>(iter: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut args = Self::default();
        let mut iter = iter.into_iter().map(Into::into).peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--help" | "-h" => args.help = true,
                "--quiet" | "-q" => args.quiet = true,
                "--stats" => args.stats_only = true,
                "--world-info" => args.world_info = true,
                "--debug" => args.debug = true,
                "--no-dump" => args.dump = None,
                "--seed" => {
                    let value = iter.next().ok_or("--seed needs a number")?;
                    args.seed = parse_u64(&value, "--seed")?;
                }
                "--frames" => {
                    let value = iter.next().ok_or("--frames needs a number")?;
                    args.frames = parse_u64(&value, "--frames")?;
                }
                "--npcs" => {
                    let value = iter.next().ok_or("--npcs needs a number")?;
                    args.npcs = parse_u64(&value, "--npcs")? as usize;
                }
                "--size" => {
                    let value = iter.next().ok_or("--size needs WxH")?;
                    args.size = parse_size(&value)?;
                }
                "--mode" => {
                    let value = iter
                        .next()
                        .ok_or("--mode needs raster, hybrid or raytrace")?;
                    args.mode = parse_mode(&value)?;
                }
                "--dump" => {
                    let value = iter.next().ok_or("--dump needs a directory")?;
                    args.dump = Some(PathBuf::from(value));
                }
                other => return Err(format!("unknown flag `{other}` (try --help)")),
            }
        }
        Ok(args)
    }

    /// The world configuration the demo uses.
    #[must_use]
    pub fn world_config(&self) -> WorldConfig {
        WorldConfig {
            seed: self.seed,
            ..WorldConfig::default()
        }
    }

    /// The help text.
    #[must_use]
    pub fn help() -> String {
        "\
town-demo — a ready-to-run Noxel village

USAGE:
    town-demo [OPTIONS]                  # after ./scripts/build-dist.sh
    cargo run -p town-demo -- [OPTIONS]  # from the source tree

OPTIONS:
    --seed N          World seed (default 0x4E4F5845)
    --frames N        Frames to simulate (default 600)
    --size WxH        Internal resolution (default 320x180)
    --mode MODE       raster | hybrid | raytrace (default raster)
    --npcs N          Target crowd size (default 240)
    --dump DIR        Write every frame as a PNG (default `frames`)
    --no-dump         Render without writing anything
    --stats           Render one frame, print its statistics, exit
    --world-info      Print the world's shape for this seed, exit
    --debug           Draw the engine statistics panel over the frame
    --quiet           Suppress everything but the summary
    -h, --help        Print this text

EXAMPLES:
    town-demo --frames 300 --dump frames
    town-demo --mode hybrid --npcs 400
    town-demo --seed 12345 --world-info

The assets are found automatically, in this order: $NOXEL_ASSET_DIR, ./assets
beside the binary, ../examples/town-demo/assets, then ./examples/town-demo/assets
and ./assets relative to the working directory. A missing asset tree is not
fatal — the world falls back to procedural content.
"
        .to_string()
    }
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} expects a non-negative number, got `{value}`"))
}

fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let (w, h) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("--size expects WxH, got `{value}`"))?;
    let w = w
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("bad width in `{value}`"))?;
    let h = h
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("bad height in `{value}`"))?;
    if w == 0 || h == 0 {
        return Err(format!("--size must be at least 1x1, got `{value}`"));
    }
    Ok((w, h))
}

fn parse_mode(value: &str) -> Result<ShadingMode, String> {
    match value.to_ascii_lowercase().as_str() {
        "raster" | "rasterize" => Ok(ShadingMode::Raster),
        "hybrid" => Ok(ShadingMode::Hybrid),
        "raytrace" | "rt" => Ok(ShadingMode::Raytrace),
        other => Err(format!("unknown mode `{other}` (raster, hybrid, raytrace)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable() {
        let args = Args::default();
        assert_eq!(args.frames, 600);
        assert_eq!(args.size, (320, 180));
        assert_eq!(args.mode, ShadingMode::Raster);
        assert!(args.dump.is_some());
    }

    #[test]
    fn parses_every_flag() {
        let args = Args::parse([
            "--seed", "7", "--frames", "10", "--size", "160x90", "--mode", "hybrid", "--npcs",
            "50", "--dump", "/tmp/x",
        ])
        .unwrap();
        assert_eq!(args.seed, 7);
        assert_eq!(args.frames, 10);
        assert_eq!(args.size, (160, 90));
        assert_eq!(args.mode, ShadingMode::Hybrid);
        assert_eq!(args.npcs, 50);
        assert_eq!(args.dump.as_deref(), Some(std::path::Path::new("/tmp/x")));
    }

    #[test]
    fn no_dump_clears_the_directory() {
        let args = Args::parse(["--no-dump"]).unwrap();
        assert!(args.dump.is_none());
    }

    #[test]
    fn parses_boolean_flags() {
        let args = Args::parse(["-q", "--stats", "--world-info", "-h", "--debug"]).unwrap();
        assert!(args.quiet && args.stats_only && args.world_info && args.help && args.debug);
        assert!(
            !Args::default().debug,
            "the statistics panel is off by default"
        );
    }

    #[test]
    fn rejects_an_unknown_flag() {
        let err = Args::parse(["--nope"]).unwrap_err();
        assert!(err.contains("--nope"), "{err}");
        assert!(err.contains("--help"));
    }

    #[test]
    fn rejects_a_missing_value() {
        assert!(Args::parse(["--seed"]).is_err());
        assert!(Args::parse(["--mode"]).is_err());
        assert!(Args::parse(["--size"]).is_err());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Args::parse(["--seed", "abc"]).is_err());
        assert!(Args::parse(["--size", "160"]).is_err());
        assert!(Args::parse(["--size", "0x90"]).is_err());
        assert!(Args::parse(["--size", "axb"]).is_err());
        assert!(Args::parse(["--mode", "toon"]).is_err());
    }

    #[test]
    fn mode_aliases_work() {
        assert_eq!(
            Args::parse(["--mode", "rt"]).unwrap().mode,
            ShadingMode::Raytrace
        );
        assert_eq!(
            Args::parse(["--mode", "RASTER"]).unwrap().mode,
            ShadingMode::Raster
        );
        assert_eq!(
            Args::parse(["--mode", "rasterize"]).unwrap().mode,
            ShadingMode::Raster
        );
    }

    #[test]
    fn size_accepts_a_capital_x() {
        assert_eq!(Args::parse(["--size", "64X48"]).unwrap().size, (64, 48));
    }

    #[test]
    fn help_is_a_usage_block() {
        let text = Args::help();
        assert!(text.contains("USAGE"));
        for flag in ["--seed", "--frames", "--size", "--mode", "--npcs", "--dump"] {
            assert!(text.contains(flag), "{flag} missing from the help");
        }
    }

    #[test]
    fn world_config_carries_the_seed() {
        let args = Args::parse(["--seed", "99"]).unwrap();
        assert_eq!(args.world_config().seed, 99);
    }
}
