//! Noxel's audio layer: a small synthesiser, a procedural composer, and a
//! platform output behind an opt-in feature.
//!
//! # Why there is no audio file in this repository
//!
//! The soundtrack is **generated from numbers**. A [`Mood`](music::Mood) says
//! what key, tempo, register and texture the music should have, and a
//! [`Composer`](music::Composer) turns that into samples:
//!
//! ```no_run
//! use noxel_audio::music::{Composer, moods};
//!
//! let mut composer = Composer::new(48_000, 0x5EED, moods::SPRING);
//! let mut buffer = vec![0.0f32; 48_000 * 2]; // one second, stereo
//! composer.fill(&mut buffer);
//! ```
//!
//! That has three consequences worth the price of a synthesiser:
//!
//! * **It varies for free.** Weather and season change a dozen numbers and the
//!   music follows, with no second asset to author and no crossfade to write.
//! * **It is deterministic.** Same seed, same samples — so a test can assert on
//!   a rendered buffer and a performance can be reproduced.
//! * **It is reviewable.** An MP3 is not diffable. A mood is.
//!
//! # Layout
//!
//! | Module | What it holds |
//! |---|---|
//! | [`synth`] | oscillators, envelopes, filters, a voice, a noise bed |
//! | [`music`] | moods, the chord progression, the note chooser, the mix |
//! | [`output`] | the device stream, behind the `output` feature |
//!
//! # The dependency rule
//!
//! [`synth`] and [`music`] depend on nothing but `core` and `noxel-core`, and
//! they are where all the interesting behaviour lives. Only [`output`] needs a
//! platform crate, and only when the `output` feature is on — the same box
//! `noxel-window` keeps `winit` in. `docs/adr/0013-audio.md` records why there
//! is a second exception to `docs/adr/0002-no-dependencies.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod music;
pub mod synth;

#[cfg(feature = "output")]
pub mod output;

#[cfg(feature = "output")]
pub use output::{AudioError, AudioHandle};

/// The types a game reaches for on its first line of audio.
pub mod prelude {
    pub use crate::music::{Composer, Mood, moods};
    pub use crate::synth::{NoiseBed, OnePole, Rng, Voice, note_hz};
}
