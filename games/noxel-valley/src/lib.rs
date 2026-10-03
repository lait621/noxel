//! # Noxel Valley — the game's rules
//!
//! This library is the playable part of the farming game, split out from the
//! binary so it can be tested and reused. `src/main.rs` is the thin part: it
//! parses arguments, loads assets, wires the plugin and drives the loop.
//!
//! | Module | What it owns |
//! |---|---|
//! | [`action`] | what using a tool on a tile does — the game's core verb |
//! | [`config`] | every number and data table the game is tuned by |
//! | [`world`] | the tile grid, the farm layout, and the mesh built from it |
//! | [`player`] | movement, facing, the tool swing, screen-to-tile |
//! | [`sim`] | the clock, the weather, the bag and the economy |
//! | [`assets`] | finding and loading the atlases and the font |
//! | [`skin`] | building the UI theme from the atlas |
//! | [`ui`] | every screen the player sees |
//!
//! # The shape of a farming game
//!
//! ```text
//!   water a crop  ──►  the plant's `watered` flag
//!                            │
//!                        night falls
//!                            │
//!   rain waters everything ──┤
//!                            ▼
//!                    watered plants gain a day
//!                    out-of-season plants die
//!                    today's watering is cleared
//!                            │
//!   harvest          ◄── days >= growth_days
//!   sell at the bin  ──►  gold arrives the next morning
//!   buy seeds        ◄──  gold, at the shop
//! ```
//!
//! Everything above is a value in [`sim::GameState`] or a flag on a
//! [`world::Tile`]. Nothing reads a clock, a random number generator or a global;
//! the weather is a function of the save's seed and the farm layout is a function
//! of the tile's address. That is the engine's determinism rule
//! (`docs/adr/0006-deterministic-generation.md`) applied to gameplay, and it is
//! what makes "rain on day 3 waters the whole farm" a test rather than an
//! observation.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod action;
pub mod assets;
pub mod config;
pub mod player;
pub mod sim;
pub mod skin;
pub mod ui;
pub mod world;
