//! `noxel-gen` — the deterministic pixel-art asset generator for Noxel.
//!
//! The tool draws every texture the demo ships — terrain, props, characters,
//! buildings — derives the tile sets, prefabs, sprite descriptions and world
//! configuration from the same constants, and writes them under one asset root.
//!
//! Two properties are load-bearing:
//!
//! * **Determinism.** Generation is a pure function of the seed. There is no
//!   clock, no system randomness and no iteration over a hash map anywhere in
//!   the pipeline, so two runs on the same tree produce byte-identical files.
//! * **Idempotence.** `generate` compares before it writes: a file whose bytes
//!   are unchanged is left alone, so a build never touches a timestamp. The
//!   `verify` subcommand makes the same comparison a gate.
//!
//! ```text
//! noxel-gen generate [--out DIR] [--seed N] [--force]
//! noxel-gen verify   [--out DIR]
//! noxel-gen preview  [--out DIR] [--scale N]
//! noxel-gen farm     --out DIR [--scale N]
//! noxel-gen list
//! ```

#![forbid(unsafe_code)]

mod assets;
mod buildings;
mod characters;
mod cli;
mod draw;
mod error;
mod farm;
mod farm_preview;
mod palette;
mod prefabs;
mod preview;
mod props;
mod sprites;
mod terrain;
mod tilesets;
mod world;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

/// The default generation seed: `NOXEL` in ASCII.
///
/// It is small enough to survive the JSON round trip through an `f64` without
/// losing precision, which matters because it is written into `world/demo.json`.
pub const DEFAULT_SEED: u64 = 0x004E_4F58_454C;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noxel-gen: error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatches one command line.
fn run(args: &[String]) -> error::Result<()> {
    match cli::parse(args)? {
        cli::Command::Generate { out, seed, force } => {
            let summary = assets::generate(&out, seed, force)?;
            println!("{}", summary.line(&out));
        }
        cli::Command::Verify { out } => {
            // The seed lives in the world the assets describe, so `verify` can
            // check a tree that was generated with `--seed N` without being
            // told about it again.
            let seed = assets::seed_from_disk(&out).unwrap_or(DEFAULT_SEED);
            let summary = assets::verify(&out, seed)?;
            println!(
                "noxel-gen: verified {} files, {} bytes (seed {seed}) in {}",
                summary.files,
                summary.bytes,
                out.display()
            );
        }
        cli::Command::Preview { out, scale } => {
            let (width, height) = preview::write(&out, scale)?;
            println!(
                "noxel-gen: preview {width}x{height} (scale {scale}) -> {}",
                out.join("preview.png").display()
            );
        }
        cli::Command::Farm { out, scale } => {
            let summary = farm::write(&out, scale)?;
            println!("{}", summary.line(&out));
        }
        cli::Command::List => {
            print!("{}", assets::listing(DEFAULT_SEED)?);
        }
        cli::Command::Help => {
            print!("{}", cli::USAGE);
        }
    }
    Ok(())
}
