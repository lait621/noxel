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
//! noxel-gen list
//! ```
//!
//! The generator is also a **library**. `draw`, `palette` and `error` are the
//! primitives a game's own art generator builds on, so a game can ship its own
//! sprites without forking this crate or copying those files and letting them
//! drift. See `noxel-gen`'s library documentation.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match noxel_gen::run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noxel-gen: error: {error}");
            ExitCode::FAILURE
        }
    }
}
