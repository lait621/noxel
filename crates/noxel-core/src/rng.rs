//! Deterministic random numbers.
//!
//! Two layers, and the distinction matters for a streaming world:
//!
//! * [`Pcg32`] — a fast, high-quality generator. Use it for anything that
//!   happens *within* a frame: NPC idle chatter, particle jitter, AI tie-breaks.
//! * [`RngStream`] — a **splittable, addressable** stream. Ask it for the
//!   generator belonging to "the buildings of chunk (17, -4)" and you get the
//!   same sequence every time, in any process, on any machine, regardless of
//!   which chunks were generated before it.
//!
//! That second property is what makes Noxel's large worlds reproducible: the
//! world is a pure function of `(seed, address)`, never of visit order. Never
//! derive world content from a single shared mutable generator — see
//! `docs/adr/0003-deterministic-world-generation.md`.

use core::ops::Range;

/// An FNV-1a 64-bit hash.
///
/// Used for stream labels and for the cheap "hash a coordinate" trick in the
/// world generator. Not cryptographic and not meant to be.
#[inline]
#[must_use]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Hashes a string into a 64-bit value. Stable across runs and platforms.
#[inline]
#[must_use]
pub fn hash_str(s: &str) -> u64 {
    fnv1a_64(s.as_bytes())
}

/// Mixes an integer into a hash (SplitMix64 finaliser).
///
/// Use this to combine `(x, y, z, seed)` into one value instead of building a
/// temporary string.
#[inline]
#[must_use]
pub fn hash_u64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Combines two hashes.
#[inline]
#[must_use]
pub fn hash_combine(a: u64, b: u64) -> u64 {
    hash_u64(a ^ hash_u64(b))
}

/// Hashes a 2D integer coordinate together with a seed.
#[inline]
#[must_use]
pub fn hash_2d(x: i32, y: i32, seed: u64) -> u64 {
    let mut h = seed;
    h = hash_combine(h, x as u64 ^ 0x9E37_79B9);
    h = hash_combine(h, y as u64 ^ 0x85EB_CA6B);
    h
}

/// Hashes a 3D integer coordinate together with a seed.
#[inline]
#[must_use]
pub fn hash_3d(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    hash_combine(hash_2d(x, y, seed), z as u64 ^ 0xC2B2_AE35)
}

/// A permuted congruential generator (PCG-XSH-RR 64/32).
///
/// * 64-bit state, 32-bit output, period 2^64.
/// * Passes BigCrush; statistically far better than an LCG or xorshift.
/// * ~2 ns per draw, which is fast enough to generate a chunk of world inside a
///   single frame budget without a job system.
///
/// ```
/// use noxel_core::rng::Pcg32;
///
/// let mut a = Pcg32::new(1234);
/// let mut b = Pcg32::new(1234);
/// assert_eq!(a.next_u32(), b.next_u32(), "same seed -> same sequence");
///
/// let x = a.range_i32(0, 9); // inclusive
/// assert!((0..=9).contains(&x));
/// ```
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    /// The default stream selector used by [`Pcg32::new`].
    pub const DEFAULT_STREAM: u64 = 0x853C_49E6_748F_EA9B;

    /// Creates a generator from a seed.
    #[inline]
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::from_seed_stream(seed, Self::DEFAULT_STREAM)
    }

    /// Creates a generator from a seed and a stream selector.
    ///
    /// Two generators with the same seed but different streams produce
    /// independent sequences.
    #[must_use]
    pub fn from_seed_stream(seed: u64, stream: u64) -> Self {
        let mut g = Self { state: 0, inc: (stream << 1) | 1 };
        g.next_u32();
        g.state = g.state.wrapping_add(seed);
        g.next_u32();
        g
    }

    /// Advances the generator and returns 32 random bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Advances the generator and returns 64 random bits.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }

    /// Returns a float in `[0, 1)`.
    ///
    /// Uses 24 bits so every result is exactly representable; this avoids the
    /// subtle bias of dividing a 32-bit integer by `u32::MAX`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Returns a float in `[0, 1)` as `f64`.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Returns `true` with probability `p` (clamped to `[0, 1]`).
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }

    /// Returns a uniformly distributed integer in `[min, max]` **inclusive**.
    ///
    /// Returns `min` when the range is empty.
    #[inline]
    pub fn range_i32(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max as i64 - min as i64 + 1) as u64;
        min + (self.next_u64() % span) as i32
    }

    /// Returns a uniformly distributed integer in `[min, max)`.
    #[inline]
    pub fn range_usize(&mut self, min: usize, max: usize) -> usize {
        if max <= min {
            return min;
        }
        min + (self.next_u64() % (max - min) as u64) as usize
    }

    /// Returns a float in `[min, max)`.
    #[inline]
    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
    }

    /// Returns a float in `[-1, 1)`.
    #[inline]
    pub fn signed_f32(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Picks a random element, or `None` for an empty slice.
    #[inline]
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() { None } else { Some(&items[self.range_usize(0, items.len())]) }
    }

    /// Picks an index weighted by `weights`.
    ///
    /// Returns `None` when the slice is empty or every weight is non-positive.
    /// This is how the world generator chooses between building archaeology
    /// styles and how the NPC scheduler picks idle behaviours.
    pub fn weighted_index(&mut self, weights: &[f32]) -> Option<usize> {
        let total: f32 = weights.iter().filter(|w| w.is_finite() && **w > 0.0).sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = self.next_f32() * total;
        for (i, w) in weights.iter().enumerate() {
            if !w.is_finite() || *w <= 0.0 {
                continue;
            }
            pick -= *w;
            if pick <= 0.0 {
                return Some(i);
            }
        }
        // Floating-point drift can leave a tiny remainder; fall back to the last
        // positive entry rather than returning None.
        weights.iter().rposition(|w| w.is_finite() && *w > 0.0)
    }

    /// Fisher–Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        if items.len() < 2 {
            return;
        }
        for i in (1..items.len()).rev() {
            let j = self.range_usize(0, i + 1);
            items.swap(i, j);
        }
    }

    /// A random unit vector in the XZ plane.
    #[inline]
    pub fn unit_vec2(&mut self) -> crate::math::Vec2 {
        let a = self.range_f32(0.0, crate::math::TAU);
        crate::math::Vec2::new(a.cos(), a.sin())
    }

    /// A random point uniformly distributed on the unit sphere.
    #[inline]
    pub fn unit_vec3(&mut self) -> crate::math::Vec3 {
        // Marsaglia's method: uniform in the cube, reject until inside the
        // unit ball, then normalise. Average 1.9 iterations.
        loop {
            let x = self.signed_f32();
            let y = self.signed_f32();
            let z = self.signed_f32();
            let len_sq = x * x + y * y + z * z;
            if len_sq > 1e-6 && len_sq <= 1.0 {
                let inv = 1.0 / len_sq.sqrt();
                return crate::math::Vec3::new(x * inv, y * inv, z * inv);
            }
        }
    }

    /// A random point uniformly distributed inside the unit XZ disc.
    #[inline]
    pub fn inside_unit_disc(&mut self) -> crate::math::Vec2 {
        let r = self.next_f32().sqrt();
        let a = self.range_f32(0.0, crate::math::TAU);
        crate::math::Vec2::new(r * a.cos(), r * a.sin())
    }

    /// A normally distributed sample (mean 0, standard deviation 1).
    #[inline]
    pub fn gaussian(&mut self) -> f32 {
        // Box–Muller, discarding the second sample. Cheap and good enough for
        // spawn jitter and camera shake.
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (crate::math::TAU * u2).cos()
    }

    /// A random value in a range, with an approximately Gaussian distribution.
    #[inline]
    pub fn gaussian_range(&mut self, min: f32, max: f32) -> f32 {
        let mid = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        (mid + self.gaussian() * half * 0.5).clamp(min, max)
    }

    /// Derives an independent child generator. Useful for handing each worker
    /// in the job pool a non-overlapping stream.
    #[inline]
    pub fn fork(&mut self, index: u64) -> Self {
        Self::from_seed_stream(self.next_u64(), index)
    }

    /// The current internal state, for snapshotting a world generator.
    #[inline]
    #[must_use]
    pub fn state(&self) -> u64 {
        self.state
    }

    /// Restores a previously snapshotted state.
    #[inline]
    pub fn set_state(&mut self, state: u64) {
        self.state = state;
    }
}

/// An addressable, splittable random stream.
///
/// A `RngStream` is a *coordinate*, not a sequence: asking for
/// `RngStream::for_chunk(seed, "buildings", chunk)` in a fresh process yields
/// exactly the same generator as it did in the last one. That property is what
/// lets Noxel stream an effectively unbounded world without storing it.
///
/// ```
/// use noxel_core::rng::RngStream;
///
/// let a = RngStream::for_chunk(7, "buildings", 12, -3).rng().next_u32();
/// let b = RngStream::for_chunk(7, "buildings", 12, -3).rng().next_u32();
/// let c = RngStream::for_chunk(7, "buildings", 12, -2).rng().next_u32();
/// assert_eq!(a, b);
/// assert_ne!(a, c);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RngStream {
    /// The world seed this stream descends from.
    pub seed: u64,
    /// A hash of the stream's label path.
    pub label: u64,
    /// Disambiguates streams that share a label and seed.
    pub index: u64,
}

impl RngStream {
    /// The root stream for a world seed.
    #[inline]
    #[must_use]
    pub fn root(seed: u64) -> Self {
        Self { seed, label: 0, index: 0 }
    }

    /// A stream addressed by `(seed, label, index)`.
    #[inline]
    #[must_use]
    pub fn indexed(seed: u64, label: &str, index: u64) -> Self {
        Self { seed, label: hash_str(label), index }
    }

    /// A stream addressed by a 2D chunk coordinate.
    #[inline]
    #[must_use]
    pub fn for_chunk(seed: u64, label: &str, x: i32, y: i32) -> Self {
        Self {
            seed,
            label: hash_combine(hash_str(label), 0xC0DE_0001),
            index: hash_2d(x, y, seed),
        }
    }

    /// A stream addressed by a 3D coordinate (chunks with vertical structure, or
    /// a position within a chunk).
    #[inline]
    #[must_use]
    pub fn for_grid3(seed: u64, label: &str, x: i32, y: i32, z: i32) -> Self {
        Self {
            seed,
            label: hash_combine(hash_str(label), 0xC0DE_0002),
            index: hash_3d(x, y, z, seed),
        }
    }

    /// Derives a labelled sub-stream. The label is hashed together with the
    /// parent's, so `"town/roads"` and `"roads/town"` differ.
    #[inline]
    #[must_use]
    pub fn fork(&self, label: &str) -> Self {
        Self {
            seed: self.seed,
            label: hash_combine(self.label, hash_str(label)),
            index: self.index,
        }
    }

    /// Derives a sub-stream with an additional index.
    #[inline]
    #[must_use]
    pub fn fork_indexed(&self, label: &str, index: u64) -> Self {
        Self {
            seed: self.seed,
            label: hash_combine(self.label, hash_str(label)),
            index: hash_combine(self.index, index),
        }
    }

    /// Materialises a generator for this stream.
    #[inline]
    #[must_use]
    pub fn rng(&self) -> Pcg32 {
        Pcg32::from_seed_stream(
            hash_combine(self.seed, self.index),
            hash_combine(self.label, 0x5EED_5EED),
        )
    }

    /// A convenience: `n` draws from this stream.
    #[must_use]
    pub fn values(&self, n: usize) -> Vec<u32> {
        let mut r = self.rng();
        (0..n).map(|_| r.next_u32()).collect()
    }

    /// A convenience: a random `f32` in `[0, 1)` at position `i` of this stream.
    ///
    /// Equivalent to `stream.rng().nth(i)` but without materialising anything;
    /// useful for sampling one value out of a procedural texture.
    #[inline]
    #[must_use]
    pub fn value_at(&self, i: u64) -> f32 {
        self.fork_indexed("value_at", i).rng().next_f32()
    }
}

/// A deterministic shuffle bag.
///
/// Returns every element of a list exactly once per cycle in a random order,
/// which is what "random without repeating" wants (loot tables, weather
/// rotation). A plain RNG would produce runs of the same value.
#[derive(Clone, Debug)]
pub struct ShuffleBag<T: Clone> {
    items: Vec<T>,
    order: Vec<usize>,
    cursor: usize,
}

impl<T: Clone> ShuffleBag<T> {
    /// Creates a bag over `items`.
    #[must_use]
    pub fn new(items: Vec<T>) -> Self {
        let order = (0..items.len()).collect();
        Self { items, order, cursor: 0 }
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the bag is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Draws the next item, reshuffling when the cycle completes.
    pub fn next(&mut self, rng: &mut Pcg32) -> Option<&T> {
        if self.items.is_empty() {
            return None;
        }
        if self.cursor >= self.order.len() {
            rng.shuffle(&mut self.order);
            self.cursor = 0;
        }
        let idx = self.order[self.cursor];
        self.cursor += 1;
        self.items.get(idx)
    }
}

/// Returns `true` when `value` is a power of two (and non-zero).
#[inline]
#[must_use]
pub const fn is_power_of_two(value: u32) -> bool {
    value != 0 && (value & (value - 1)) == 0
}

/// Uniformly picks an index in `range`, given a random `u32`.
#[inline]
#[must_use]
pub fn pick_in_range(r: u32, range: Range<usize>) -> usize {
    let span = range.end.saturating_sub(range.start);
    if span == 0 { range.start } else { range.start + (r as usize) % span }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Pcg32::new(99);
        let mut b = Pcg32::new(99);
        for _ in 0..64 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_streams_diverge() {
        let mut a = Pcg32::from_seed_stream(1, 1);
        let mut b = Pcg32::from_seed_stream(1, 2);
        let sa: Vec<u32> = (0..8).map(|_| a.next_u32()).collect();
        let sb: Vec<u32> = (0..8).map(|_| b.next_u32()).collect();
        assert_ne!(sa, sb);
    }

    #[test]
    fn range_is_inclusive_and_bounded() {
        let mut r = Pcg32::new(5);
        let mut seen_lo = false;
        let mut seen_hi = false;
        for _ in 0..2000 {
            let v = r.range_i32(0, 3);
            assert!((0..=3).contains(&v));
            seen_lo |= v == 0;
            seen_hi |= v == 3;
        }
        assert!(seen_lo && seen_hi, "endpoints must be reachable");
    }

    #[test]
    fn empty_range_returns_min() {
        let mut r = Pcg32::new(1);
        assert_eq!(r.range_i32(7, 7), 7);
        assert_eq!(r.range_i32(7, 3), 7);
    }

    #[test]
    fn next_f32_is_in_unit_interval() {
        let mut r = Pcg32::new(11);
        for _ in 0..10_000 {
            let v = r.next_f32();
            assert!((0.0..1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Pcg32::new(3);
        let mut v: Vec<u32> = (0..64).collect();
        r.shuffle(&mut v);
        v.sort_unstable();
        assert_eq!(v, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn weighted_index_respects_zero_weights() {
        let mut r = Pcg32::new(3);
        for _ in 0..100 {
            let i = r.weighted_index(&[0.0, 1.0, 0.0]).unwrap();
            assert_eq!(i, 1);
        }
        assert_eq!(r.weighted_index(&[0.0, 0.0]), None);
        assert_eq!(r.weighted_index(&[]), None);
    }

    #[test]
    fn weighted_index_distribution_is_roughly_right() {
        let mut r = Pcg32::new(17);
        let mut counts = [0u32; 3];
        for _ in 0..30_000 {
            counts[r.weighted_index(&[1.0, 3.0, 6.0]).unwrap()] += 1;
        }
        // Expect ~10% / 30% / 60%; allow generous slack.
        assert!(counts[0] > 2000 && counts[0] < 4000, "{counts:?}");
        assert!(counts[1] > 7500 && counts[1] < 10500, "{counts:?}");
        assert!(counts[2] > 15500 && counts[2] < 20500, "{counts:?}");
    }

    #[test]
    fn unit_vec3_is_normalised() {
        let mut r = Pcg32::new(21);
        for _ in 0..1000 {
            let v = r.unit_vec3();
            assert!((v.length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn gaussian_is_centred() {
        let mut r = Pcg32::new(31);
        let n = 20_000;
        let mean: f32 = (0..n).map(|_| r.gaussian()).sum::<f32>() / n as f32;
        assert!(mean.abs() < 0.05, "mean {mean}");
    }

    #[test]
    fn stream_is_addressable_and_reproducible() {
        let a = RngStream::for_chunk(7, "buildings", 12, -3).rng().next_u32();
        let b = RngStream::for_chunk(7, "buildings", 12, -3).rng().next_u32();
        let c = RngStream::for_chunk(7, "buildings", 12, -2).rng().next_u32();
        let d = RngStream::for_chunk(7, "roads", 12, -3).rng().next_u32();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn stream_fork_order_matters() {
        let base = RngStream::root(1);
        let ab = base.fork("a").fork("b").rng().next_u32();
        let ba = base.fork("b").fork("a").rng().next_u32();
        assert_ne!(ab, ba);
    }

    #[test]
    fn stream_value_at_is_stable() {
        let s = RngStream::indexed(5, "texture", 9);
        let v1 = s.value_at(42);
        let v2 = s.value_at(42);
        let v3 = s.value_at(43);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
        assert!((0.0..1.0).contains(&v1));
    }

    #[test]
    fn shuffle_bag_covers_everything_once() {
        let mut bag = ShuffleBag::new(vec![1, 2, 3, 4]);
        let mut r = Pcg32::new(8);
        let mut cycle: Vec<i32> = (0..4).map(|_| *bag.next(&mut r).unwrap()).collect();
        cycle.sort_unstable();
        assert_eq!(cycle, vec![1, 2, 3, 4]);
    }

    #[test]
    fn hash_is_stable_and_well_mixed() {
        assert_eq!(hash_str("noxel"), hash_str("noxel"));
        assert_ne!(hash_str("noxel"), hash_str("noxeL"));
        assert_ne!(hash_2d(0, 0, 1), hash_2d(0, 1, 1));
        assert_ne!(hash_3d(1, 2, 3, 0), hash_3d(3, 2, 1, 0));
    }

    #[test]
    fn hash_2d_neighbours_are_uncorrelated() {
        // Adjacent coordinates must not produce correlated outputs, or a
        // generated world shows visible banding. Measure the mean Hamming
        // distance between neighbours: 32 bits is the uncorrelated expectation.
        let mut bits = 0u32;
        let n = 256;
        for i in 0..n {
            let a = hash_2d(100 + i, 200, 0);
            let b = hash_2d(101 + i, 200, 0);
            bits += (a ^ b).count_ones();
        }
        let mean = bits as f32 / n as f32;
        assert!((25.0..=39.0).contains(&mean), "mean hamming distance {mean}");

        // And the values must all be distinct over a small window.
        let mut seen: Vec<u64> = (0..512).map(|i| hash_2d(i, 7, 1)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 512, "hash must not collide over a 512-cell run");
    }

    #[test]
    fn fork_generators_are_independent() {
        let mut root = Pcg32::new(4);
        let mut a = root.fork(0);
        let mut b = root.fork(1);
        let sa: Vec<u32> = (0..4).map(|_| a.next_u32()).collect();
        let sb: Vec<u32> = (0..4).map(|_| b.next_u32()).collect();
        assert_ne!(sa, sb);
    }

    #[test]
    fn power_of_two_check() {
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(1000));
    }
}
