//! `noxel-gen` as a library: the primitives a Noxel asset generator is built
//! from, plus the generator itself.
//!
//! The binary is a thin front end over this. The library exists because a game
//! ships its own art — see `games/noxel-valley` in the Noxel Valley repository —
//! and that art wants the same palette, the same drawing helpers and the same
//! error type as the demo's, without forking this crate or copying three files
//! and letting them drift.
//!
//! Everything here is deterministic: generation is a pure function of its seed,
//! so two runs on one tree produce byte-identical files.
//!
//! ```text
//! noxel-gen generate [--out DIR] [--seed N] [--force]
//! noxel-gen verify   [--out DIR]
//! noxel-gen preview  [--out DIR] [--scale N]
//! noxel-gen list
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod assets;
pub mod buildings;
pub mod characters;
pub mod cli;
pub mod draw;
pub mod error;
pub mod palette;
pub mod prefabs;
pub mod preview;
pub mod props;
pub mod sprites;
pub mod terrain;
pub mod tilesets;
pub mod world;

#[cfg(test)]
mod tests;

/// Dispatches one command line.
///
/// The library's entry point, and the binary's entire body: everything the tool
/// does is reachable from here, which is what makes it testable without spawning
/// a process.
///
/// # Errors
/// Returns whatever the subcommand returns — a usage error, an IO error, or a
/// file that failed validation before it was written.
pub fn run(args: &[String]) -> error::Result<()> {
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
        cli::Command::List => {
            print!("{}", assets::listing(DEFAULT_SEED)?);
        }
        cli::Command::Help => {
            print!("{}", cli::USAGE);
        }
    }
    Ok(())
}

/// The default generation seed: `NOXEL` in ASCII.
///
/// It is small enough to survive the JSON round trip through an `f64` without
/// losing precision, which matters because it is written into `world/demo.json`.
pub const DEFAULT_SEED: u64 = 0x004E_4F58_454C;
