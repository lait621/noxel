//! The synthesiser: oscillators, envelopes, filters, and a voice.
//!
//! Everything here is **pure arithmetic over `f32`**, with no device, no thread
//! and no allocation after construction. That is deliberate on three counts:
//!
//! * A game's audio callback must not allocate, so the building blocks must not
//!   either.
//! * A synthesiser that needs a sound card to run cannot be tested. These can:
//!   `render` a buffer and assert on it.
//! * The engine has no audio files and no decoder. Music is generated from
//!   numbers, which means it is diffable, deterministic, and free of the asset
//!   pipeline entirely.
//!
//! # Anti-aliasing
//!
//! A naive sawtooth or square at a high pitch aliases badly — the harmonics
//! above Nyquist fold back down as inharmonic tones, which is the gritty sound
//! people mean by "cheap synth". Two things avoid it here:
//!
//! * The voices are built from **sines**, which have no harmonics to fold.
//! * Where a brighter tone is wanted, its partials are limited to those below
//!   Nyquist, and a one-pole low-pass rolls off the rest.
//!
//! That keeps the module small and the result clean, at the cost of not having
//! a proper band-limited oscillator. For a gentle soundtrack that is the right
//! trade; a synthesiser with a filter envelope and a resonant ladder would be
//! several times the size for a sound this music does not use.

use core::f32::consts::TAU;

/// A deterministic pseudo-random source.
///
/// The engine's determinism rule ([ADR 0006](../../../docs/adr/0006-deterministic-generation.md))
/// applies to audio too: the same seed must give the same music, or a recorded
/// performance cannot be reproduced and a test cannot assert on a buffer.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    /// A generator from a seed. Zero is remapped, because xorshift is stuck
    /// there.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// The next 32-bit value.
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// A value in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A value in `[-1, 1)`.
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// The next value in a range.
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.next_f32() * (high - low)
    }

    /// Picks one of a slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u32() as usize) % items.len()]
    }
}

/// Converts a MIDI note number to a frequency in Hz.
#[must_use]
pub fn note_hz(note: f32) -> f32 {
    440.0 * exp2((note - 69.0) / 12.0)
}

/// `2^x`, which `std` has as `f32::exp2`.
#[must_use]
pub fn exp2(x: f32) -> f32 {
    x.exp2()
}

/// A one-pole low-pass filter.
///
/// One pole rather than a biquad: the music wants a gentle tilt, not a
/// resonant sweep, and a one-pole cannot self-oscillate however badly it is
/// driven.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    state: f32,
}

impl OnePole {
    /// Filters one sample.
    ///
    /// `cutoff` is in Hz and `sample_rate` in Hz. A cutoff above Nyquist passes
    /// everything, which is what a caller who has not thought about it expects.
    pub fn process(&mut self, input: f32, cutoff: f32, sample_rate: f32) -> f32 {
        let nyquist = sample_rate * 0.5;
        if cutoff >= nyquist {
            self.state = input;
            return input;
        }
        // The standard one-pole coefficient. The clamp keeps a cutoff near zero
        // from producing a coefficient that never moves the state at all.
        let x = (-TAU * cutoff.max(1.0) / sample_rate).exp();
        let coefficient = (1.0 - x).clamp(0.0, 1.0);
        self.state += coefficient * (input - self.state);
        self.state
    }

    /// Forgets its history, for a voice restart.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// A percussive amplitude envelope: a short attack, then an exponential decay.
///
/// Exponential rather than linear because a linear decay of a plucked tone
/// sounds like it is being faded out by a hand, and an exponential one sounds
/// like the string running out of energy — which is what it is doing.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    level: f32,
    attack: f32,
    decay: f32,
    phase: f32,
    releasing: bool,
}

impl Envelope {
    /// An envelope with no level.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            level: 0.0,
            attack: 0.0,
            decay: 1.0,
            phase: 0.0,
            releasing: false,
        }
    }

    /// Restarts the envelope with an attack and a decay, both in seconds.
    pub fn trigger(&mut self, attack: f32, decay: f32) {
        self.level = 0.0;
        self.attack = attack.max(1e-4);
        self.decay = decay.max(1e-3);
        self.phase = 0.0;
        self.releasing = false;
    }

    /// Begins the release, for a note the composer wants to cut short.
    pub fn release(&mut self, decay: f32) {
        if !self.releasing {
            self.decay = decay.max(1e-3);
            self.phase = 0.0;
            self.releasing = true;
        }
    }

    /// Advances one sample and returns the new level.
    pub fn step(&mut self, sample_rate: f32, dt: f32) -> f32 {
        self.phase += dt;
        if !self.releasing && self.phase < self.attack {
            self.level = (self.phase / self.attack).clamp(0.0, 1.0);
        } else {
            // `exp(-t)`, computed from an absolute time so the decay does not
            // depend on how the frame was broken up. A decay that multiplies
            // per sample would change with the buffer size, which is exactly
            // the class of bug that makes a synth sound different on two
            // machines.
            let t = self.phase - if self.releasing { 0.0 } else { self.attack };
            self.level = (-t / (self.decay * 0.35)).exp();
        }
        let _ = sample_rate;
        self.level
    }

    /// Whether the envelope has fallen far enough to stop the voice.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.level < 1e-4
    }

    /// The current level.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }
}

/// One sounding note.
///
/// A small additive stack — a fundamental and two partials — through a low-pass
/// and an envelope. The partial mix is what gives a tone its character: mostly
/// fundamental is a flute, a strong second is a bell, and a weak third is a
/// marimba, which is the register this soundtrack lives in.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    phase: f32,
    frequency: f32,
    /// Levels of the fundamental, second and third partials.
    partials: [f32; 3],
    /// Partial frequency multipliers.
    ratios: [f32; 3],
    envelope: Envelope,
    filter: OnePole,
    cutoff: f32,
    /// Level the note was struck at, before the envelope.
    velocity: f32,
    /// Samples of tremolo, for a voice that should breathe.
    vibrato: f32,
    pan: f32,
    active: bool,
}

impl Voice {
    /// A silent voice.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            phase: 0.0,
            frequency: 440.0,
            partials: [1.0, 0.0, 0.0],
            ratios: [1.0, 2.0, 3.0],
            envelope: Envelope::idle(),
            filter: OnePole { state: 0.0 },
            cutoff: 6000.0,
            velocity: 1.0,
            vibrato: 0.0,
            pan: 0.0,
            active: false,
        }
    }

    /// Starts a note.
    ///
    /// `timbre` is `[fundamental, second, third]` partial levels. The sample
    /// rate is needed to scale the envelope and the filter.
    #[allow(clippy::too_many_arguments)]
    pub fn strike(
        &mut self,
        frequency: f32,
        velocity: f32,
        timbre: [f32; 3],
        attack: f32,
        decay: f32,
        cutoff: f32,
        pan: f32,
        vibrato: f32,
    ) {
        self.phase = 0.0;
        self.frequency = frequency;
        self.partials = timbre;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.cutoff = cutoff;
        self.pan = pan.clamp(-1.0, 1.0);
        self.vibrato = vibrato;
        self.filter.reset();
        self.envelope.trigger(attack, decay);
        self.active = true;
    }

    /// Whether the voice is still sounding.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// The voice's stereo position, `-1` left to `1` right.
    #[must_use]
    pub const fn pan(&self) -> f32 {
        self.pan
    }

    /// Renders one sample and returns `(left, right)`.
    pub fn next(&mut self, sample_rate: f32) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let dt = 1.0 / sample_rate;
        let level = self.envelope.step(sample_rate, dt);
        if self.envelope.is_finished() {
            self.active = false;
            return (0.0, 0.0);
        }

        // A slow vibrato, applied as a frequency multiplier rather than by
        // changing the phase increment abruptly, so it cannot click.
        let wobble = if self.vibrato > 0.0 {
            1.0 + (TAU * 4.7 * self.phase_like()).sin() * self.vibrato
        } else {
            1.0
        };

        let mut value = 0.0;
        self.phase += dt * self.frequency * wobble;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        for (ratio, partial) in self.ratios.iter().zip(self.partials.iter()) {
            // Skip a partial whose frequency is above Nyquist: it would alias
            // down as an inharmonic tone, which is the single most recognisable
            // "cheap synth" artefact.
            let partial_hz = self.frequency * ratio;
            if partial_hz >= sample_rate * 0.5 || *partial == 0.0 {
                continue;
            }
            value += (TAU * self.phase * ratio).sin() * partial;
        }

        let shaped = self.filter.process(value, self.cutoff, sample_rate);
        let out = shaped * level * self.velocity * 0.25;
        // Equal-power panning: a constant-amplitude law would dip in the middle.
        let angle = (self.pan + 1.0) * 0.25 * core::f32::consts::PI;
        (out * angle.cos(), out * angle.sin())
    }

    /// A cheap second time base for the vibrato, so it does not need its own
    /// field and a second phase accumulator.
    fn phase_like(&self) -> f32 {
        self.phase
    }
}

/// A bed of filtered noise, for rain and wind.
///
/// Noise is not decoration here: a rainy day in the game should *sound* like
/// rain, and a filtered noise bed does that far better than any melodic change.
#[derive(Clone, Debug)]
pub struct NoiseBed {
    rng: Rng,
    low: OnePole,
    high: OnePole,
    level: f32,
    /// Cutoff of the gentle band-pass, in Hz.
    cutoff: f32,
    /// How much the level wanders, `0` for a flat bed.
    gust: f32,
    phase: f32,
}

impl NoiseBed {
    /// A silent bed.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            low: OnePole::default(),
            high: OnePole::default(),
            level: 0.0,
            cutoff: 2400.0,
            gust: 0.0,
            phase: 0.0,
        }
    }

    /// Sets the level and character.
    pub fn set(&mut self, level: f32, cutoff: f32, gust: f32) {
        self.level = level.clamp(0.0, 1.0);
        self.cutoff = cutoff;
        self.gust = gust;
    }

    /// Renders one sample.
    pub fn next(&mut self, sample_rate: f32) -> f32 {
        if self.level <= 1e-5 {
            return 0.0;
        }
        let white = self.rng.next_bipolar();
        // A band-pass built from two one-poles: low-pass to take off the top,
        // then high-pass (input minus its low-pass) to take off the rumble.
        // Cheaper than a biquad and indistinguishable for a noise bed.
        let low = self.low.process(white, self.cutoff, sample_rate);
        let band = low - self.high.process(low, self.cutoff * 0.25, sample_rate);

        let shape = if self.gust > 0.0 {
            self.phase += 1.0 / sample_rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            1.0 - self.gust * 0.5 * (1.0 - (TAU * 0.08 * self.phase).cos())
        } else {
            1.0
        };
        band * self.level * shape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    #[test]
    fn the_rng_is_deterministic_and_never_sticks() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..64 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        // Zero is a fixed point of xorshift, so it has to be remapped.
        let mut zero = Rng::new(0);
        assert_ne!(zero.next_u32(), 0);
        assert_ne!(zero.next_u32(), 0);
    }

    #[test]
    fn rng_output_stays_in_range() {
        let mut rng = Rng::new(99);
        for _ in 0..1000 {
            let value = rng.next_f32();
            assert!((0.0..1.0).contains(&value), "{value}");
            let bipolar = rng.next_bipolar();
            assert!((-1.0..1.0).contains(&bipolar), "{bipolar}");
        }
    }

    #[test]
    fn a_note_maps_to_the_octave_it_should() {
        // A4 is 440, and an octave is a doubling. If this drifts, every note in
        // the game is out of tune by the same ratio and nothing else notices.
        assert!((note_hz(69.0) - 440.0).abs() < 0.01);
        assert!((note_hz(81.0) - 880.0).abs() < 0.01);
        assert!((note_hz(57.0) - 220.0).abs() < 0.01);
        assert!((note_hz(60.0) - 261.63).abs() < 0.05, "middle C");
    }

    #[test]
    fn a_one_pole_at_nyquist_passes_everything() {
        let mut filter = OnePole::default();
        for input in [-1.0, 0.5, 0.25, -0.75] {
            assert_eq!(filter.process(input, RATE * 0.5, RATE), input);
        }
    }

    #[test]
    fn a_one_pole_eventually_follows_a_constant_input() {
        let mut filter = OnePole::default();
        let mut out = 0.0;
        for _ in 0..4000 {
            out = filter.process(1.0, 200.0, RATE);
        }
        assert!((out - 1.0).abs() < 0.01, "the filter settled at {out}");
    }

    #[test]
    fn a_low_pass_removes_a_fast_alternation() {
        // The defining property: a signal that flips every sample is entirely
        // above the cutoff and must come out quiet.
        let mut filter = OnePole::default();
        let mut peak: f32 = 0.0;
        for index in 0..2000 {
            let input = if index % 2 == 0 { 1.0 } else { -1.0 };
            peak = peak.max(filter.process(input, 100.0, RATE).abs());
        }
        assert!(
            peak < 0.05,
            "the low pass let {peak} of a Nyquist signal through"
        );
    }

    #[test]
    fn an_envelope_rises_then_decays_to_nothing() {
        let mut envelope = Envelope::idle();
        envelope.trigger(0.01, 0.5);
        let mut peak: f32 = 0.0;
        for _ in 0..(RATE * 0.01) as u32 {
            peak = peak.max(envelope.step(RATE, 1.0 / RATE));
        }
        assert!(peak > 0.9, "the attack only reached {peak}");

        let mut samples = 0;
        while !envelope.is_finished() && samples < (RATE * 5.0) as u32 {
            envelope.step(RATE, 1.0 / RATE);
            samples += 1;
        }
        assert!(envelope.is_finished(), "the envelope never decayed");
    }

    #[test]
    fn a_voice_starts_silent_grows_and_ends_silent() {
        let mut voice = Voice::idle();
        assert!(!voice.is_active());
        assert_eq!(voice.next(RATE), (0.0, 0.0));

        voice.strike(440.0, 1.0, [1.0, 0.4, 0.1], 0.005, 0.4, 4000.0, 0.0, 0.0);
        assert!(voice.is_active());

        let mut peak: f32 = 0.0;
        let mut samples = 0;
        while voice.is_active() && samples < (RATE * 5.0) as u32 {
            let (left, right) = voice.next(RATE);
            peak = peak.max(left.abs()).max(right.abs());
            samples += 1;
        }
        assert!(peak > 0.02, "the voice never made a sound: peak {peak}");
        assert!(peak < 1.0, "the voice clipped at {peak}");
        assert!(!voice.is_active(), "the voice never finished");
    }

    #[test]
    fn a_centred_voice_is_equal_in_both_channels() {
        let mut voice = Voice::idle();
        voice.strike(440.0, 1.0, [1.0, 0.0, 0.0], 0.001, 0.5, 8000.0, 0.0, 0.0);
        let (left, right) = voice.next(RATE);
        assert!((left - right).abs() < 1e-6, "centre is {left} / {right}");
    }

    #[test]
    fn a_voice_panned_right_is_louder_on_the_right() {
        let mut voice = Voice::idle();
        voice.strike(440.0, 1.0, [1.0, 0.0, 0.0], 0.001, 0.5, 8000.0, 1.0, 0.0);
        let mut energy = (0.0f32, 0.0f32);
        for _ in 0..2000 {
            let (left, right) = voice.next(RATE);
            energy.0 += left * left;
            energy.1 += right * right;
        }
        assert!(
            energy.1 > energy.0 * 100.0,
            "panning did not take: {energy:?}"
        );
    }

    #[test]
    fn an_aliasing_partial_is_dropped_rather_than_folded() {
        // A partial above Nyquist folds back as an inharmonic tone. The voice
        // has to skip it, and the audible way to check is that the fundamental
        // survives and nothing screams.
        let mut voice = Voice::idle();
        voice.strike(
            20_000.0,
            1.0,
            [1.0, 1.0, 1.0],
            0.001,
            0.2,
            20_000.0,
            0.0,
            0.0,
        );
        let mut peak: f32 = 0.0;
        for _ in 0..2000 {
            let (left, _) = voice.next(RATE);
            peak = peak.max(left.abs());
        }
        // Only the fundamental is under Nyquist at 20 kHz, so it is quiet but
        // present — and crucially bounded.
        assert!(peak < 0.3, "a folded partial produced {peak}");
    }

    #[test]
    fn a_noise_bed_is_silent_at_zero_level_and_bounded_when_loud() {
        let mut bed = NoiseBed::new(1);
        bed.set(0.0, 2000.0, 0.0);
        for _ in 0..100 {
            assert_eq!(bed.next(RATE), 0.0);
        }
        bed.set(1.0, 2000.0, 0.4);
        let mut peak: f32 = 0.0;
        for _ in 0..20_000 {
            peak = peak.max(bed.next(RATE).abs());
        }
        assert!(peak > 0.01, "the bed never made a sound");
        assert!(peak < 1.0, "the bed clipped at {peak}");
    }

    #[test]
    fn rendering_is_deterministic() {
        // The engine's rule, applied to audio: same seed, same samples.
        let render = || {
            let mut voice = Voice::idle();
            voice.strike(329.63, 0.8, [1.0, 0.3, 0.1], 0.01, 0.7, 3000.0, -0.3, 0.01);
            let mut out = Vec::new();
            for _ in 0..4000 {
                out.push(voice.next(RATE).0);
            }
            out
        };
        assert_eq!(render(), render());
    }
}
