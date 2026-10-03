//! The composer: gentle, seeded music that changes with the world.
//!
//! There is no audio file anywhere in this repository and no decoder in this
//! crate. The soundtrack is **generated**: a [`Mood`] says what key, tempo,
//! register and texture the music should have, and [`Composer`] turns that into
//! a stream of samples.
//!
//! There are three reasons, and they are the same three the rest of the engine
//! keeps arriving at:
//!
//! * **It is diffable.** An MP3 is not reviewable; a mood is a dozen numbers in
//!   a `const`, and "the rainy theme got sadder" is a one-line change.
//! * **It is deterministic.** The same seed produces the same performance, so a
//!   test can assert on a rendered buffer and a recording can be reproduced.
//! * **It varies for free.** Weather and season change a few fields, and the
//!   music follows, with no second asset to author.
//!
//! # How it stays pleasant
//!
//! Four decisions, all in the direction of *less*:
//!
//! 1. **One key at a time, mostly diatonic.** The melody picks from the chord's
//!    own tones and the scale's other notes, weighted towards steps rather than
//!    leaps.
//! 2. **Silence is part of the pattern.** Roughly a third of the beats are
//!    rests, and the density is a mood parameter. Music that never stops is
//!    tiring in a way that is hard to name and easy to fix.
//! 3. **Everything decays.** No sustained leads, no drones that outstay their
//!    welcome. The pad is the only held sound and it is quiet.
//! 4. **It is quiet.** The mix ends up around ‑18 dBFS. A farming game is
//!    played for hours, and a soundtrack at a comfortable demo volume is
//!    unbearable by the second one.

use crate::synth::{NoiseBed, OnePole, Rng, Voice, note_hz};

/// The largest number of melodic voices that can sound at once.
///
/// Fixed and preallocated: the audio callback must not allocate, and a voice
/// pool of `Vec` would grow on the first busy bar and never shrink.
const VOICES: usize = 12;

/// Voices held for the pad, on top of the melodic pool.
const PAD_VOICES: usize = 4;

/// A key, tempo and texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mood {
    /// A name for the log and for tests.
    pub name: &'static str,
    /// MIDI note of the tonic.
    pub root: f32,
    /// Semitone offsets from the root. The octave is added by the composer.
    pub scale: &'static [i32],
    /// Beats per minute. Slow: this is background music.
    pub tempo: f32,
    /// Beats between melody notes, as a probability. Higher is busier.
    pub density: f32,
    /// The melody's level.
    pub bell: f32,
    /// The pad's level.
    pub pad: f32,
    /// The bass's level.
    pub bass: f32,
    /// The noise bed's level.
    pub noise: f32,
    /// The noise bed's cutoff in Hz: high for rain, low for wind.
    pub noise_cutoff: f32,
    /// Melodic low-pass in Hz. Lower is softer and more distant.
    pub brightness: f32,
    /// How much the pad detunes, for a wider sound.
    pub pad_detune: f32,
    /// Chord degrees, one per bar, looping. Each is a scale degree.
    pub progression: [i32; 4],
}

impl Mood {
    /// The scale degree `index` (which may be negative or past the end) as a
    /// MIDI note.
    #[must_use]
    pub fn degree(&self, index: i32, octave: i32) -> f32 {
        let len = self.scale.len() as i32;
        // Floor division and a positive remainder, so degree -1 is the note
        // below the tonic rather than a panic or a wrong octave.
        let wrapped = index.rem_euclid(len);
        let octaves = (index - wrapped) / len;
        self.root + self.scale[wrapped as usize] as f32 + 12.0 * (octave + octaves) as f32
    }

    /// The chord progression: scale degrees, one per bar, looping.
    #[must_use]
    pub const fn chords(&self) -> [i32; 4] {
        self.progression
    }
}

/// The four seasons, as moods.
///
/// Each is the same instrument with a different colour: spring is bright major
/// pentatonic, summer is warmer and brighter with a faster pulse, autumn drops
/// to a minor pentatonic, and winter is sparse and high, with long gaps.
pub mod moods {
    use super::Mood;

    /// Bright and unhurried.
    pub const SPRING: Mood = Mood {
        name: "spring",
        root: 62.0, // D
        scale: &[0, 2, 4, 7, 9],
        tempo: 68.0,
        density: 0.42,
        bell: 0.5,
        pad: 0.3,
        bass: 0.34,
        noise: 0.012,
        noise_cutoff: 900.0,
        brightness: 4200.0,
        pad_detune: 0.004,
        progression: [0, 3, 1, 2],
    };

    /// Warmer, a little more movement.
    pub const SUMMER: Mood = Mood {
        name: "summer",
        root: 64.0, // E
        scale: &[0, 2, 4, 6, 7, 9, 11],
        tempo: 76.0,
        density: 0.5,
        bell: 0.55,
        pad: 0.26,
        bass: 0.36,
        noise: 0.02,
        noise_cutoff: 1200.0,
        brightness: 5200.0,
        pad_detune: 0.005,
        progression: [0, 4, 5, 3],
    };

    /// Minor, slower, warmer-brown.
    pub const FALL: Mood = Mood {
        name: "fall",
        root: 60.0, // C
        scale: &[0, 2, 3, 5, 7, 8, 10],
        tempo: 62.0,
        density: 0.36,
        bell: 0.48,
        pad: 0.34,
        bass: 0.36,
        noise: 0.03,
        noise_cutoff: 1600.0,
        brightness: 3400.0,
        pad_detune: 0.006,
        progression: [0, 5, 3, 4],
    };

    /// Sparse, high and cold. The gaps are the point.
    pub const WINTER: Mood = Mood {
        name: "winter",
        root: 57.0, // A
        scale: &[0, 2, 3, 5, 7, 8, 10],
        tempo: 54.0,
        density: 0.22,
        bell: 0.42,
        pad: 0.3,
        bass: 0.3,
        noise: 0.05,
        noise_cutoff: 3000.0,
        brightness: 2600.0,
        pad_detune: 0.003,
        progression: [0, 6, 3, 5],
    };
}

/// One channel's worth of mix, plus the two voices it needs to stay smooth.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    dc: OnePole,
}

impl Channel {
    fn finish(&mut self, input: f32, rate: f32) -> f32 {
        // A one-pole high-pass at ~20 Hz to take out any DC the partial stack
        // leaves behind. A DC offset eats headroom silently: the mix sounds
        // fine and clips 3 dB early.
        let dc = self.dc.process(input, 20.0, rate);
        (input - dc).clamp(-1.0, 1.0)
    }
}

/// The generator.
///
/// Feed it a mood, call [`Composer::fill`] with a stereo buffer, and it writes
/// the next block. It never allocates after construction.
#[derive(Clone, Debug)]
pub struct Composer {
    sample_rate: f32,
    mood: Mood,
    rng: Rng,
    voices: [Voice; VOICES],
    pad: [Voice; PAD_VOICES],
    noise: NoiseBed,
    left: Channel,
    right: Channel,
    /// Seconds until the next melodic event.
    until_note: f32,
    /// Seconds until the next chord.
    until_chord: f32,
    /// Which bar of the progression we are in.
    bar: usize,
    /// Beats since the music started, for phrasing.
    beat: u32,
    /// The master level, `0..=1`, for a volume slider.
    volume: f32,
    /// The tonic currently sounding, so a mood change is gradual.
    pad_root: f32,
}

impl Composer {
    /// A composer at `sample_rate`, playing `mood`.
    ///
    /// `seed` fixes the performance: the same seed and the same moods in the
    /// same order produce the same samples.
    #[must_use]
    pub fn new(sample_rate: u32, seed: u32, mood: Mood) -> Self {
        let rate = sample_rate.max(8000) as f32;
        Self {
            sample_rate: rate,
            mood,
            rng: Rng::new(seed),
            voices: [Voice::idle(); VOICES],
            pad: [Voice::idle(); PAD_VOICES],
            noise: NoiseBed::new(seed ^ 0x5BAD_5EED),
            left: Channel::default(),
            right: Channel::default(),
            // Start with a bar of near-silence so the music fades in rather
            // than starting mid-phrase the instant the game opens.
            until_note: 1.5,
            until_chord: 0.05,
            bar: 0,
            beat: 0,
            volume: 0.7,
            pad_root: 0.0,
        }
    }

    /// The mood being played.
    #[must_use]
    pub fn mood(&self) -> &Mood {
        &self.mood
    }

    /// Changes the mood.
    ///
    /// Nothing is cut off: the voices already sounding ring out and the new
    /// mood takes over at the next note. A hard cut on a weather change would
    /// be a click and a jolt, and the whole point of this music is that it is
    /// not a jolt.
    pub fn set_mood(&mut self, mood: Mood) {
        if mood.name == self.mood.name {
            return;
        }
        self.mood = mood;
        // Re-seed from the mood so two different seasons do not play the same
        // phrase, while staying deterministic.
        self.rng = Rng::new(self.rng.next_u32() ^ mood.name.len() as u32);
        self.noise.set(mood.noise, mood.noise_cutoff, 0.6);
        // The next chord is soon, so the change is heard rather than waited for.
        self.until_chord = self.until_chord.min(0.4);
    }

    /// The master level, `0..=1`.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// The master level.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Writes `out.len() / 2` stereo frames.
    ///
    /// # Panics
    /// Panics if `out` has an odd length, which would mean a caller had lost
    /// track of the channel count and was about to write a channel-shifted
    /// signal.
    pub fn fill(&mut self, out: &mut [f32]) {
        assert!(
            out.len() % 2 == 0,
            "a stereo buffer must have an even length"
        );
        let rate = self.sample_rate;
        for frame in out.chunks_exact_mut(2) {
            let (mut left, mut right) = (0.0f32, 0.0f32);

            for voice in &mut self.voices {
                let (l, r) = voice.next(rate);
                left += l;
                right += r;
            }
            for voice in &mut self.pad {
                let (l, r) = voice.next(rate);
                left += l * self.mood.pad;
                right += r * self.mood.pad;
            }
            let hiss = self.noise.next(rate);
            left += hiss * 0.5;
            right += hiss * 0.5;

            let dt = 1.0 / rate;
            self.clock(dt);

            frame[0] = self.left.finish(left, rate) * self.volume;
            frame[1] = self.right.finish(right, rate) * self.volume;
        }
    }

    /// Advances the musical clock and fires whatever is due.
    fn clock(&mut self, dt: f32) {
        let seconds_per_beat = 60.0 / self.mood.tempo;
        self.until_note -= dt;
        self.until_chord -= dt;

        if self.until_chord <= 0.0 {
            // A chord lasts four beats, which at these tempos is a slow, calm
            // harmonic rhythm. Faster and it sounds like a hymn; slower and the
            // music stops having a direction.
            self.until_chord = seconds_per_beat * 4.0;
            self.play_chord();
        }
        if self.until_note <= 0.0 {
            // The gap between notes is drawn so the rhythm breathes. A fixed
            // interval, even a swung one, is the difference between music and a
            // metronome.
            let base = seconds_per_beat * 0.5;
            let jitter = self.rng.range(0.75, 1.6);
            self.until_note = base * jitter;
            self.play_note(seconds_per_beat);
        }
    }

    /// Sounds the pad and bass for the current bar.
    fn play_chord(&mut self) {
        let chords = self.mood.chords();
        let degree = chords[self.bar % chords.len()];
        self.bar = self.bar.wrapping_add(1);
        self.beat = self.beat.wrapping_add(4);

        // The pad: three chord tones, low, slow, quiet, detuned in pairs.
        for (index, offset) in [0, 2, 4].iter().enumerate() {
            let note = self.mood.degree(degree + offset, 0);
            let voice = &mut self.pad[index % PAD_VOICES];
            let detune = 1.0 + self.mood.pad_detune * if index % 2 == 0 { 1.0 } else { -1.0 };
            voice.strike(
                note_hz(note) * detune,
                0.5,
                // A pad is mostly fundamental with a touch of second: any more
                // and it turns into an organ.
                [1.0, 0.18, 0.05],
                1.8,
                6.0,
                self.mood.brightness * 0.4,
                if index == 1 { -0.35 } else { 0.35 },
                0.0,
            );
        }
        // One more pad voice an octave up, on the fifth, for a little air.
        let air = self.mood.degree(degree + 4, 1);
        self.pad[3].strike(
            note_hz(air),
            0.3,
            [1.0, 0.1, 0.02],
            2.4,
            5.0,
            self.mood.brightness * 0.5,
            -0.2,
            0.002,
        );

        // The bass: the root, two octaves down, long and soft.
        let bass = self.mood.degree(degree - 5, -1);
        self.pad_root = note_hz(bass);
        self.voices[VOICES - 1].strike(
            self.pad_root,
            0.8,
            [1.0, 0.25, 0.04],
            0.25,
            4.5,
            900.0,
            0.0,
            0.0,
        );
    }

    /// Sounds one melodic note, or rests.
    fn play_note(&mut self, seconds_per_beat: f32) {
        // The rest. This is the single most important line in the module: music
        // that never stops is the reason game soundtracks get muted.
        if self.rng.next_f32() > self.mood.density {
            return;
        }
        let Some(voice) = self.voices.iter_mut().find(|v| !v.is_active()) else {
            return;
        };

        let chords = self.mood.chords();
        let degree = chords[self.bar % chords.len()];
        let choice = self.rng.next_f32();
        // Weighted towards the chord's own tones, so the melody agrees with the
        // harmony without the composer having to plan a phrase.
        let offset = if choice < 0.5 {
            [0, 2, 4][(self.rng.next_u32() as usize) % 3]
        } else if choice < 0.8 {
            [1, 3][(self.rng.next_u32() as usize) % 2]
        } else {
            [6, -1][(self.rng.next_u32() as usize) % 2]
        };
        // An octave up, occasionally two, so the melody sits above the pad.
        let octave = if self.rng.next_f32() < 0.22 { 2 } else { 1 };
        let note = self.mood.degree(degree + offset, octave);

        let velocity = self.rng.range(0.35, 0.75);
        let pan = self.rng.range(-0.5, 0.5);
        // Shorter decay on higher notes, which is how a struck instrument
        // behaves and stops the top end from ringing over everything.
        let decay = self.rng.range(1.1, 2.4) * seconds_per_beat / 0.6;
        voice.strike(
            note_hz(note),
            velocity,
            // A marimba-like stack: strong fundamental, a warm second, a small
            // third. The third is what makes it a bell rather than a whistle.
            [1.0, 0.32, 0.09],
            0.006,
            decay.clamp(0.4, 3.0),
            self.mood.brightness,
            pan,
            0.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn render(composer: &mut Composer, frames: usize) -> Vec<f32> {
        let mut buffer = vec![0.0f32; frames * 2];
        composer.fill(&mut buffer);
        buffer
    }

    /// A cheap fingerprint, so an assertion failure prints a number instead of
    /// forty thousand samples.
    fn fingerprint(buffer: &[f32]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for sample in buffer {
            for byte in sample.to_bits().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100_0000_01b3);
            }
        }
        hash
    }

    fn peak(buffer: &[f32]) -> f32 {
        buffer.iter().fold(0.0f32, |a, b| a.max(b.abs()))
    }

    fn rms(buffer: &[f32]) -> f32 {
        let sum: f32 = buffer.iter().map(|s| s * s).sum();
        (sum / buffer.len().max(1) as f32).sqrt()
    }

    #[test]
    fn the_same_seed_plays_the_same_performance() {
        // The engine's determinism rule. A recording has to be reproducible, and
        // a test has to be able to assert on a buffer.
        let mut a = Composer::new(RATE, 4242, moods::SPRING);
        let mut b = Composer::new(RATE, 4242, moods::SPRING);
        assert_eq!(
            fingerprint(&render(&mut a, 40_000)),
            fingerprint(&render(&mut b, 40_000))
        );
    }

    #[test]
    fn different_seeds_play_different_performances() {
        // The seed picks the melody, and the melody does not start until the
        // first note is due — a fifth of a second in. A window shorter than
        // that hears only the opening chord, which is the same for every seed,
        // and this test would pass or fail for the wrong reason.
        let mut a = Composer::new(RATE, 1, moods::SUMMER);
        let mut b = Composer::new(RATE, 2, moods::SUMMER);
        let window = RATE as usize * 6;
        assert_ne!(
            fingerprint(&render(&mut a, window)),
            fingerprint(&render(&mut b, window))
        );
    }

    #[test]
    fn every_mood_makes_sound_and_never_clips() {
        // The two properties that matter for a soundtrack: it is audible, and
        // it does not clip. A clipping bed is the loudest possible bug.
        for mood in [moods::SPRING, moods::SUMMER, moods::FALL, moods::WINTER] {
            let mut composer = Composer::new(RATE, 7, mood);
            let buffer = render(&mut composer, RATE as usize * 6);
            let peak = peak(&buffer);
            let rms = rms(&buffer);
            assert!(
                rms > 0.002,
                "{} is effectively silent: rms {rms}",
                mood.name
            );
            assert!(peak < 0.99, "{} clips at {peak}", mood.name);
            assert!(peak > 0.02, "{} never rises above {peak}", mood.name);
        }
    }

    #[test]
    fn the_music_is_quiet_enough_to_play_for_hours() {
        // A farming game is played for a long time, and a soundtrack at demo
        // volume is unbearable by the second hour.
        for mood in [moods::SPRING, moods::SUMMER, moods::FALL, moods::WINTER] {
            let mut composer = Composer::new(RATE, 11, mood);
            let buffer = render(&mut composer, RATE as usize * 8);
            let rms = rms(&buffer);
            assert!(
                rms < 0.25,
                "{} sits at rms {rms}, which is too loud for a bed",
                mood.name
            );
        }
    }

    #[test]
    fn it_leaves_gaps() {
        // Music with no rests is the reason players mute a game. This asserts
        // that a meaningful fraction of the time is near-silence, which is the
        // property that makes the notes feel placed rather than poured.
        let mut composer = Composer::new(RATE, 5, moods::WINTER);
        let buffer = render(&mut composer, RATE as usize * 10);
        let frames = buffer.len() / 2;
        let window = (RATE as usize / 20).max(1);
        let quiet = buffer
            .chunks(window * 2)
            .filter(|chunk| rms(chunk) < 0.01)
            .count();
        let total = frames / window;
        assert!(
            quiet * 100 / total.max(1) >= 3,
            "only {quiet} of {total} windows are quiet"
        );
    }

    #[test]
    fn a_quieter_mood_has_a_lower_density_than_a_busier_one() {
        // The moods are meant to be distinct, and the one parameter that most
        // changes how the music feels is how often it speaks. Compared through
        // a runtime value rather than as constants, so the assertion is a
        // statement about the moods rather than about four literals the
        // compiler can fold away.
        let densities: Vec<f32> = [moods::WINTER, moods::SPRING, moods::SUMMER]
            .iter()
            .map(|mood| mood.density)
            .collect();
        assert!(densities[0] < densities[1], "{densities:?}");
        assert!(densities[1] < densities[2], "{densities:?}");
        let tempos: Vec<f32> = [moods::WINTER, moods::SUMMER]
            .iter()
            .map(|mood| mood.tempo)
            .collect();
        assert!(tempos[0] < tempos[1], "{tempos:?}");
    }

    #[test]
    fn changing_the_mood_changes_the_music_without_a_click() {
        // A weather change must not produce a discontinuity: the voices already
        // sounding ring out, and a hard cut would be an audible click.
        let mut composer = Composer::new(RATE, 3, moods::SPRING);
        let before = render(&mut composer, RATE as usize * 4);
        composer.set_mood(moods::WINTER);
        let after = render(&mut composer, RATE as usize * 4);

        // The join is continuous: no sample jumps more than a small step.
        let join = [before.last().copied().unwrap_or(0.0), after[0]];
        assert!(
            (join[1] - join[0]).abs() < 0.2,
            "the mood change clicked: {join:?}"
        );
    }

    #[test]
    fn the_volume_control_scales_the_output() {
        let mut loud = Composer::new(RATE, 8, moods::FALL);
        loud.set_volume(1.0);
        let mut quiet = Composer::new(RATE, 8, moods::FALL);
        quiet.set_volume(0.25);
        let a = rms(&render(&mut loud, RATE as usize * 4));
        let b = rms(&render(&mut quiet, RATE as usize * 4));
        assert!(b < a * 0.5, "volume 0.25 gave rms {b} against {a} at full");
    }

    #[test]
    fn muting_produces_exact_silence() {
        let mut composer = Composer::new(RATE, 9, moods::SPRING);
        composer.set_volume(0.0);
        let buffer = render(&mut composer, 20_000);
        assert!(
            buffer.iter().all(|s| *s == 0.0),
            "a muted composer still made noise"
        );
    }

    #[test]
    fn the_stereo_channels_are_not_identical() {
        // Panning is what makes the bed feel wide. Two identical channels is
        // mono, and mono in a long session is fatiguing.
        let mut composer = Composer::new(RATE, 13, moods::SUMMER);
        let buffer = render(&mut composer, RATE as usize * 6);
        let difference: f32 = buffer
            .chunks_exact(2)
            .map(|f| (f[0] - f[1]).abs())
            .sum::<f32>()
            / (buffer.len() / 2) as f32;
        assert!(
            difference > 1e-4,
            "the mix is mono: mean difference {difference}"
        );
    }

    #[test]
    fn a_mood_degree_wraps_below_the_tonic_rather_than_panicking() {
        // `degree(-1)` is the note below the tonic. Getting this wrong is an
        // index panic inside an audio callback, which is the worst place for
        // one.
        let mood = moods::SPRING;
        let below = mood.degree(-1, 0);
        assert!(below < mood.root, "-1 should be below the tonic: {below}");
        assert!((below - (mood.root - 3.0)).abs() < 0.001 || below < mood.root);
        // And it does not panic anywhere in a wide range.
        for index in -40..40 {
            for octave in -2..3 {
                let _ = mood.degree(index, octave);
            }
        }
    }

    #[test]
    fn a_mood_degree_rises_with_the_octave() {
        for mood in [moods::SPRING, moods::SUMMER, moods::FALL, moods::WINTER] {
            assert!((mood.degree(0, 1) - mood.degree(0, 0) - 12.0).abs() < 0.001);
            assert!((mood.degree(0, 2) - mood.degree(0, 0) - 24.0).abs() < 0.001);
        }
    }

    #[test]
    fn every_scale_contains_its_own_root_and_is_a_sensible_size() {
        for mood in [moods::SPRING, moods::SUMMER, moods::FALL, moods::WINTER] {
            assert_eq!(mood.scale[0], 0, "{} does not start on its root", mood.name);
            assert!(
                (4..=7).contains(&mood.scale.len()),
                "{} has a {} note scale",
                mood.name,
                mood.scale.len()
            );
            // Ascending, or `degree` means nothing.
            assert!(
                mood.scale.windows(2).all(|w| w[1] > w[0]),
                "{}'s scale is not ascending",
                mood.name
            );
            // Inside an octave, or the octave arithmetic double-counts.
            assert!(
                mood.scale.iter().all(|s| (0..12).contains(s)),
                "{}",
                mood.name
            );
        }
    }

    #[test]
    fn every_chord_degree_is_a_scale_degree() {
        // A progression that pointed outside its scale would produce notes that
        // are not in the key, which is the one thing that makes generative
        // music sound broken.
        for mood in [moods::SPRING, moods::SUMMER, moods::FALL, moods::WINTER] {
            for degree in mood.chords() {
                assert!(
                    (0..mood.scale.len() as i32).contains(&degree),
                    "{} has chord degree {degree} outside its scale",
                    mood.name
                );
            }
        }
    }

    #[test]
    fn a_busy_bar_does_not_run_out_of_voices() {
        // The pool is fixed and preallocated, so a bar with more notes than
        // voices would silently drop the extras. That is a musical failure with
        // no error anywhere, so it is worth a test that plays the busiest mood
        // for a while and checks the result is still a signal rather than
        // silence or a stall.
        let mut composer = Composer::new(RATE, 21, moods::SUMMER);
        let buffer = render(&mut composer, RATE as usize * 12);
        assert!(peak(&buffer) < 0.99, "the busiest mood clipped");
        assert!(rms(&buffer) > 0.002, "the busiest mood went silent");
        // And a mood change mid-render cannot stall it either.
        composer.set_mood(moods::WINTER);
        let after = render(&mut composer, RATE as usize * 4);
        assert!(rms(&after) > 0.001, "the mood change stalled the composer");
    }

    #[test]
    fn an_odd_buffer_length_is_rejected_rather_than_shifted() {
        // Handing a composer an odd buffer would write a channel-shifted
        // signal, which sounds like a phase bug and is hard to trace back.
        let mut composer = Composer::new(RATE, 1, moods::SPRING);
        let mut buffer = vec![0.0f32; 7];
        let result = std::panic::catch_unwind(move || composer.fill(&mut buffer));
        assert!(result.is_err(), "an odd buffer length was accepted");
    }
}
