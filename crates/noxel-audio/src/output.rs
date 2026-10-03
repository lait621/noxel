//! Playing a [`Composer`](crate::music::Composer) on the system's audio device.
//!
//! Behind the `output` feature, off by default, for the same reason
//! `noxel-window` is: `cargo build` must download nothing and the tests must run
//! on a machine with no sound card. See `docs/adr/0013-audio.md`.
//!
//! # Why the stream owns the composer
//!
//! The audio callback runs on a real-time thread with a hard deadline. Anything
//! it touches must be already allocated, and it must never block on a lock a
//! game thread might hold. So the composer moves *into* the callback, and the
//! only things that cross the boundary are atomics: a volume, a mute, and the
//! mood. A mood is a `Copy` value of a dozen numbers, so even the mood change
//! avoids the allocator and the lock.
//!
//! If the game and the audio thread shared a `Mutex<Composer>`, a frame that
//! held it for a millisecond would produce a dropout — which is the single most
//! common way a game's audio stutters.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::music::{Composer, Mood};

/// Why audio could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The platform reports no output device at all.
    NoDevice,
    /// The device rejected the stream configuration.
    Config(String),
    /// The stream failed to build.
    Build(String),
    /// The stream failed after it started.
    Play(String),
}

impl core::fmt::Display for AudioError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no audio output device"),
            Self::Config(message) => write!(f, "the device rejected the format: {message}"),
            Self::Build(message) => write!(f, "could not build the audio stream: {message}"),
            Self::Play(message) => write!(f, "could not start the audio stream: {message}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// The controls a game holds while the audio thread plays.
///
/// Cheap to clone and safe to drop: dropping the last handle stops the music,
/// because the stream goes with it.
#[derive(Clone)]
pub struct AudioHandle {
    /// The stream is never read, and that is the point: it *is* the music. A
    /// `cpal::Stream` stops when it is dropped, so holding it here is what keeps
    /// the audio thread running for as long as the game holds a handle. The
    /// field is deliberately not `_stream` — the clone in `AudioHandle::clone`
    /// is what makes two handles share one stream rather than start two.
    #[allow(dead_code)]
    stream: Arc<cpal::Stream>,
    /// Volume as `f32` bits, so it can cross the thread boundary atomically.
    volume: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    /// The mood as a `&'static str` pointer and its length, for the same
    /// reason: a `Mood` is `Copy` and has no allocator inside it, but an enum
    /// does not fit in an atomic, so the composer polls this instead.
    mood_request: Arc<AtomicU32>,
    sample_rate: u32,
}

impl AudioHandle {
    /// Sets the master volume, `0..=1`.
    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The master volume.
    #[must_use]
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// Silences the music without stopping the stream.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Whether the music is muted.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Requests a mood.
    ///
    /// The change lands on the audio thread at the next block, so it is heard
    /// as a musical transition rather than as a cut.
    pub fn set_mood(&self, mood: Mood) {
        // The index into the engine's mood table travels as a small integer.
        self.mood_request.store(index_of(mood), Ordering::Relaxed);
    }

    /// The sample rate the device is running at.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// The moods a game can request by index.
///
/// A fixed table rather than passing a `Mood` through the atomic: `Mood` holds
/// a `&'static [i32]` and could be sent as a pointer, but a pointer's lifetime
/// cannot be proven at the atomic, and a table costs four lines and no risk.
const MOOD_TABLE: [Mood; 4] = [
    crate::music::moods::SPRING,
    crate::music::moods::SUMMER,
    crate::music::moods::FALL,
    crate::music::moods::WINTER,
];

/// The index of a mood in [`MOOD_TABLE`], or `0` for one that is not in it.
#[must_use]
pub fn index_of(mood: Mood) -> u32 {
    MOOD_TABLE
        .iter()
        .position(|m| m.name == mood.name)
        .unwrap_or(0) as u32
}

/// Starts playing `mood` on the default output device.
///
/// # Errors
/// Returns [`AudioError`] when there is no device, the device rejects the
/// format, or the stream will not start. A game that cannot play audio should
/// log this and carry on — a farming game with no sound is still a farming game.
pub fn start(seed: u32, mood: Mood) -> Result<AudioHandle, AudioError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.into();

    let volume = Arc::new(AtomicU32::new(0.7f32.to_bits()));
    let muted = Arc::new(AtomicBool::new(false));
    let mood_request = Arc::new(AtomicU32::new(index_of(mood)));

    let mut composer = Composer::new(sample_rate, seed, mood);
    let thread_volume = Arc::clone(&volume);
    let thread_muted = Arc::clone(&muted);
    let thread_mood = Arc::clone(&mood_request);

    // A scratch buffer, allocated once, so the callback never touches the
    // allocator. Two seconds is far longer than any device asks for.
    let mut scratch: Vec<f32> = Vec::with_capacity(sample_rate as usize * 2);

    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels;
                scratch.resize(frames * 2, 0.0);
                composer.fill(&mut scratch);

                let volume = if thread_muted.load(Ordering::Relaxed) {
                    0.0
                } else {
                    f32::from_bits(thread_volume.load(Ordering::Relaxed))
                };
                composer.set_volume(volume);

                let requested = thread_mood.load(Ordering::Relaxed) as usize;
                if let Some(next) = MOOD_TABLE.get(requested)
                    && next.name != composer.mood().name
                {
                    composer.set_mood(*next);
                }

                // Interleave into however many channels the device wants. A
                // mono device gets the average; a surround device gets the
                // front pair and silence, which is better than a centred mono
                // signal coming out of the rear speakers.
                for (index, frame) in out.chunks_mut(channels).enumerate() {
                    let left = scratch.get(index * 2).copied().unwrap_or(0.0);
                    let right = scratch.get(index * 2 + 1).copied().unwrap_or(0.0);
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        *sample = match (channels, channel) {
                            (1, _) => (left + right) * 0.5,
                            (_, 0) => left,
                            (_, 1) => right,
                            _ => 0.0,
                        };
                    }
                }
            },
            |error| eprintln!("noxel-audio: stream error: {error}"),
            None,
        )
        .map_err(|e| AudioError::Build(e.to_string()))?;

    stream.play().map_err(|e| AudioError::Play(e.to_string()))?;
    Ok(AudioHandle {
        stream: Arc::new(stream),
        volume,
        muted,
        mood_request,
        sample_rate,
    })
}

/// Whether an output device appears to be available, without opening one.
///
/// For a settings screen that wants to grey out a volume slider rather than
/// fail when the player moves it.
#[must_use]
pub fn is_available() -> bool {
    cpal::default_host().default_output_device().is_some()
}
