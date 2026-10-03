//! Command-line parsing.
//!
//! Hand-rolled: the crate has no third-party dependencies, and the surface is
//! five subcommands and four flags, so a parser is smaller than the argument
//! parsing crate it would replace.
//!
//! ```text
//! noxel-gen generate [--out DIR] [--seed N] [--force]
//! noxel-gen verify   [--out DIR]
//! noxel-gen preview  [--out DIR] [--scale N]
//! noxel-gen farm     --out DIR [--scale N]
//! noxel-gen list
//! ```

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The usage text printed by `--help` and on a parse error.
pub const USAGE: &str = "\
noxel-gen — deterministic pixel-art asset generator for Noxel

USAGE:
    noxel-gen generate [--out DIR] [--seed N] [--force]
    noxel-gen verify   [--out DIR]
    noxel-gen preview  [--out DIR] [--scale N]
    noxel-gen farm     --out DIR [--scale N]
    noxel-gen list

COMMANDS:
    generate    Write every asset, skipping files whose bytes are unchanged.
    verify      Regenerate in memory and compare against the files on disk.
    preview     Write a contact sheet of the textures to <out>/preview.png.
    farm        Write the farm atlas set to <out>/farm: terrain, crops, props,
                characters, ui, each a PNG plus its atlas JSON, plus a contact
                sheet. The farm set is additive: `generate` neither writes nor
                checks it.
    list        Print the manifest: every generated file, its kind and size.

OPTIONS:
    --out DIR   Asset root. Defaults to examples/town-demo/assets inside the
                workspace, found by walking up from the crate directory. `farm`
                has no default and always needs it, so that generating a farm
                never touches the demo's asset tree by accident.
    --seed N    Generation seed (default 0x4E4F58454C). Only changes the
                procedural detail scatter, never a file's identity.
    --force     generate: rewrite every file even when it is up to date.
    --scale N   preview and farm: integer upscale of the contact sheet
                (default 4 for preview, 3 for farm).
    -h, --help  Print this text.
";

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Write the assets to disk.
    Generate {
        /// Asset root.
        out: PathBuf,
        /// Generation seed.
        seed: u64,
        /// Rewrite files even when their bytes match.
        force: bool,
    },
    /// Check the files on disk against a fresh generation.
    Verify {
        /// Asset root.
        out: PathBuf,
    },
    /// Write a contact sheet of the textures.
    Preview {
        /// Asset root; the sheet is written next to the textures.
        out: PathBuf,
        /// Integer upscale factor.
        scale: u32,
    },
    /// Write the farm atlas set into `<out>/farm`.
    Farm {
        /// Asset root; the farm set goes in a `farm` directory under it.
        out: PathBuf,
        /// Integer upscale of the contact sheet.
        scale: u32,
    },
    /// Print the manifest of everything the generator writes.
    List,
    /// Print [`USAGE`].
    Help,
}

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command> {
    let mut args = args.iter();
    let Some(first) = args.next() else {
        return Err(Error::usage(format!("no subcommand given\n\n{USAGE}")));
    };

    if first == "-h" || first == "--help" || first == "help" {
        return Ok(Command::Help);
    }

    let mut out: Option<PathBuf> = None;
    let mut seed: Option<u64> = None;
    let mut scale: Option<u32> = None;
    let mut force = false;

    while let Some(arg) = args.next() {
        let (name, inline) = split_flag(arg);
        match name.as_str() {
            "--out" => out = Some(PathBuf::from(value(&name, inline, &mut args)?)),
            "--seed" => seed = Some(parse_u64(&name, &value(&name, inline, &mut args)?)?),
            "--scale" => scale = Some(parse_scale(&value(&name, inline, &mut args)?)?),
            "--force" => {
                if inline.is_some() {
                    return Err(Error::usage("`--force` takes no value"));
                }
                force = true;
            }
            "-h" | "--help" => return Ok(Command::Help),
            other => {
                return Err(Error::usage(format!("unknown option `{other}`\n\n{USAGE}")));
            }
        }
    }

    // `farm` is the one command that will not guess an asset root: its output
    // is a new tree, and the default root is the demo's shipped assets.
    let out_given = out.is_some();
    let out = match out {
        Some(path) => path,
        None => default_out_dir()?,
    };

    match first.as_str() {
        "generate" => {
            if scale.is_some() {
                return Err(Error::usage("`--scale` is only valid for `preview`"));
            }
            Ok(Command::Generate {
                out,
                seed: seed.unwrap_or(crate::DEFAULT_SEED),
                force,
            })
        }
        "verify" => {
            if scale.is_some() {
                return Err(Error::usage("`--scale` is only valid for `preview`"));
            }
            if force {
                return Err(Error::usage("`--force` is only valid for `generate`"));
            }
            Ok(Command::Verify { out })
        }
        "preview" => {
            if seed.is_some() {
                return Err(Error::usage("`--seed` is only valid for `generate`"));
            }
            if force {
                return Err(Error::usage("`--force` is only valid for `generate`"));
            }
            Ok(Command::Preview {
                out,
                scale: scale.unwrap_or(DEFAULT_PREVIEW_SCALE),
            })
        }
        "farm" => {
            if !out_given {
                return Err(Error::usage(format!(
                    "`farm` needs an explicit `--out DIR`\n\n{USAGE}"
                )));
            }
            if seed.is_some() {
                return Err(Error::usage("`--seed` is only valid for `generate`"));
            }
            if force {
                return Err(Error::usage("`--force` is only valid for `generate`"));
            }
            Ok(Command::Farm {
                out,
                scale: scale.unwrap_or(crate::farm::DEFAULT_PREVIEW_SCALE),
            })
        }
        "list" => {
            if seed.is_some() || scale.is_some() || force {
                return Err(Error::usage("`list` takes no options other than `--out`"));
            }
            Ok(Command::List)
        }
        other => Err(Error::usage(format!(
            "unknown subcommand `{other}`\n\n{USAGE}"
        ))),
    }
}

/// The default integer upscale used by `preview`.
pub const DEFAULT_PREVIEW_SCALE: u32 = 4;

/// The largest upscale `preview` accepts; beyond this the sheet is unwieldy.
pub const MAX_PREVIEW_SCALE: u32 = 32;

/// Splits `--key=value` into its parts.
fn split_flag(arg: &str) -> (String, Option<String>) {
    match arg.split_once('=') {
        Some((name, value)) => (name.to_string(), Some(value.to_string())),
        None => (arg.to_string(), None),
    }
}

/// The value of a flag, from `--key=value` or the following argument.
fn value(
    name: &str,
    inline: Option<String>,
    rest: &mut std::slice::Iter<'_, String>,
) -> Result<String> {
    match inline {
        Some(value) => Ok(value),
        None => rest
            .next()
            .cloned()
            .ok_or_else(|| Error::usage(format!("`{name}` needs a value"))),
    }
}

fn parse_u64(name: &str, text: &str) -> Result<u64> {
    text.parse::<u64>().map_err(|_| {
        Error::usage(format!(
            "`{name}` expects a non-negative integer, got `{text}`"
        ))
    })
}

fn parse_scale(text: &str) -> Result<u32> {
    let scale = text
        .parse::<u32>()
        .map_err(|_| Error::usage(format!("`--scale` expects an integer, got `{text}`")))?;
    if scale == 0 || scale > MAX_PREVIEW_SCALE {
        return Err(Error::usage(format!(
            "`--scale` must be between 1 and {MAX_PREVIEW_SCALE}, got {scale}"
        )));
    }
    Ok(scale)
}

/// The workspace root: the nearest ancestor of `start` whose `Cargo.toml`
/// declares `[workspace]`.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start.to_path_buf());
    while let Some(dir) = current {
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            if text.lines().any(|line| line.trim() == "[workspace]") {
                return Some(dir);
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// The workspace root, starting the walk at this crate's directory.
///
/// `CARGO_MANIFEST_DIR` is baked in at compile time, which is what makes
/// `cargo run -p noxel-gen` from any directory target the checkout's assets.
/// A binary copied to another machine falls back to walking up from the
/// current directory.
pub fn workspace_root() -> Option<PathBuf> {
    find_workspace_root(Path::new(env!("CARGO_MANIFEST_DIR"))).or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|dir| find_workspace_root(&dir))
    })
}

/// The default `--out`: `examples/town-demo/assets` under the workspace root.
pub fn default_out_dir() -> Result<PathBuf> {
    workspace_root()
        .map(|root| root.join("examples").join("town-demo").join("assets"))
        .ok_or_else(|| {
            Error::usage(
                "could not find the workspace root (no ancestor Cargo.toml with [workspace]); \
                 pass --out DIR",
            )
        })
}
