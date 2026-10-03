//! Small containers: generational handles, free lists, bitsets and ring buffers.
//!
//! These exist so that no other Noxel crate has to reach for a dependency. Each
//! one is deliberately minimal — if you need a general-purpose map, use `std`.

use core::marker::PhantomData;

/// A versioned index into a [`SlotMap`].
///
/// The generation counter is what makes this safe: removing an element and
/// reusing its slot bumps the generation, so a stale handle from three frames
/// ago resolves to `None` instead of silently pointing at a different object.
/// This is the same trick the ECS entity ids use, and it is why Noxel can hand
/// out handles to chunks, colliders and audio sources without a lifetime
/// parameter.
///
/// ```
/// use noxel_core::pool::SlotMap;
///
/// let mut map = SlotMap::new();
/// let a = map.insert("first");
/// assert_eq!(map.get(a), Some(&"first"));
/// assert_eq!(map.remove(a), Some("first"));
/// assert_eq!(map.get(a), None, "stale handle must not resolve");
/// ```
pub struct Handle<T> {
    index: u32,
    generation: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// A handle that never resolves to anything.
    pub const INVALID: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
        _marker: PhantomData,
    };

    /// The slot index.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation at which this handle was issued.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// True when this is [`Handle::INVALID`].
    #[inline]
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.index == u32::MAX
    }

    /// Packs the handle into a single `u64` (index in the low 32 bits).
    ///
    /// Used by the ECS to store handles compactly and by the asset database's
    /// serialised manifests.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Rebuilds a handle from [`Handle::to_bits`].
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: (bits & 0xFFFF_FFFF) as u32,
            generation: (bits >> 32) as u32,
            _marker: PhantomData,
        }
    }
}

impl<T> Clone for Handle<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T> Eq for Handle<T> {}

impl<T> PartialOrd for Handle<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Handle<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        (self.index, self.generation).cmp(&(other.index, other.generation))
    }
}

impl<T> core::hash::Hash for Handle<T> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> core::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_invalid() {
            f.write_str("Handle::INVALID")
        } else {
            write!(f, "Handle({}v{})", self.index, self.generation)
        }
    }
}

#[derive(Clone, Debug)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// An arena that hands out stable, versioned [`Handle`]s.
///
/// The core storage of the ECS's component tables, the physics world's collider
/// registry and the asset database's loaded-asset table.
#[derive(Clone, Debug)]
pub struct SlotMap<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> SlotMap<T> {
    /// An empty map.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// An empty map with room for `capacity` elements.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Inserts a value and returns its handle.
    pub fn insert(&mut self, value: T) -> Handle<T> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(
                slot.value.is_none(),
                "free list pointed at an occupied slot"
            );
            slot.value = Some(value);
            self.len += 1;
            Handle {
                index,
                generation: slot.generation,
                _marker: PhantomData,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                value: Some(value),
            });
            self.len += 1;
            Handle {
                index,
                generation: 0,
                _marker: PhantomData,
            }
        }
    }

    /// Removes the value behind `handle`, returning it when the handle is live.
    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        let value = slot.value.take()?;
        // Bump on removal so stale handles are permanently dead. Wrapping is
        // fine: a slot would have to be reused 4 billion times.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        self.len -= 1;
        Some(value)
    }

    /// Shared reference to the value behind `handle`.
    #[inline]
    #[must_use]
    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    /// Mutable reference to the value behind `handle`.
    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_mut()
    }

    /// True when the handle resolves.
    #[inline]
    #[must_use]
    pub fn contains(&self, handle: Handle<T>) -> bool {
        self.get(handle).is_some()
    }

    /// Mutable references to two different elements at once.
    ///
    /// Returns `None` for equal handles or a stale handle. The physics solver's
    /// contact loop and the ECS's two-entity queries both need this.
    #[must_use]
    pub fn get_two_mut(&mut self, a: Handle<T>, b: Handle<T>) -> Option<(&mut T, &mut T)> {
        if a.index == b.index {
            return None;
        }
        let (ai, bi) = (a.index as usize, b.index as usize);
        if ai >= self.slots.len() || bi >= self.slots.len() {
            return None;
        }
        if self.slots[ai].generation != a.generation || self.slots[bi].generation != b.generation {
            return None;
        }
        if self.slots[ai].value.is_none() || self.slots[bi].value.is_none() {
            return None;
        }
        // Split the slice so the borrow checker can see the two disjoint
        // accesses; no `unsafe` needed.
        let (lo, hi) = if ai < bi { (ai, bi) } else { (bi, ai) };
        let (left, right) = self.slots.split_at_mut(hi);
        let (la, lb) = (&mut left[lo], &mut right[0]);
        let va = la.value.as_mut()?;
        let vb = lb.value.as_mut()?;
        if ai < bi {
            Some((va, vb))
        } else {
            Some((vb, va))
        }
    }

    /// Number of live elements.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing is stored.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of slots allocated (live + free).
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Iterates over live `(handle, &value)` pairs. Order is slot order, which
    /// is stable for a given insertion/removal history and is what makes
    /// iteration deterministic.
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            slot.value.as_ref().map(|v| {
                (
                    Handle {
                        index: i as u32,
                        generation: slot.generation,
                        _marker: PhantomData,
                    },
                    v,
                )
            })
        })
    }

    /// Iterates over live `(handle, &mut value)` pairs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Handle<T>, &mut T)> + '_ {
        self.slots.iter_mut().enumerate().filter_map(|(i, slot)| {
            let generation = slot.generation;
            slot.value.as_mut().map(move |v| {
                (
                    Handle {
                        index: i as u32,
                        generation,
                        _marker: PhantomData,
                    },
                    v,
                )
            })
        })
    }

    /// Iterates over live handles.
    pub fn keys(&self) -> impl Iterator<Item = Handle<T>> + '_ {
        self.iter().map(|(h, _)| h)
    }

    /// Iterates over live values.
    pub fn values(&self) -> impl Iterator<Item = &T> + '_ {
        self.iter().map(|(_, v)| v)
    }

    /// Iterates over live values mutably.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.iter_mut().map(|(_, v)| v)
    }

    /// Removes every element for which `keep` returns `false`, returning how
    /// many were removed. `keep` is called exactly once per live element.
    pub fn retain(&mut self, mut keep: impl FnMut(Handle<T>, &mut T) -> bool) -> usize {
        let mut removed = 0;
        for i in 0..self.slots.len() {
            let generation = self.slots[i].generation;
            let Some(value) = self.slots[i].value.as_mut() else {
                continue;
            };
            let handle = Handle {
                index: i as u32,
                generation,
                _marker: PhantomData,
            };
            if keep(handle, value) {
                continue;
            }
            let slot = &mut self.slots[i];
            slot.value = None;
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(i as u32);
            self.len -= 1;
            removed += 1;
        }
        removed
    }

    /// Removes everything, keeping the allocated slots for reuse.
    pub fn clear(&mut self) {
        self.free.clear();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.value.take().is_some() {
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(i as u32);
            }
        }
        self.len = 0;
    }

    /// Removes everything and releases the memory.
    pub fn reset(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.len = 0;
    }

    /// Reserves room for `additional` more elements.
    pub fn reserve(&mut self, additional: usize) {
        self.slots.reserve(additional);
    }

    /// Fraction of allocated slots that are in use, in `[0, 1]`.
    #[must_use]
    pub fn occupancy(&self) -> f32 {
        if self.slots.is_empty() {
            1.0
        } else {
            self.len as f32 / self.slots.len() as f32
        }
    }
}

impl<T> Default for SlotMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Extend<T> for SlotMap<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for v in iter {
            self.insert(v);
        }
    }
}

impl<'a, T> IntoIterator for &'a SlotMap<T> {
    type Item = (Handle<T>, &'a T);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// A LIFO pool that recycles values instead of reallocating them.
///
/// The renderer reuses vertex arrays and the NPC system reuses path buffers;
/// both would otherwise churn the allocator every frame.
#[derive(Clone, Debug)]
pub struct FreeList<T> {
    free: Vec<T>,
    created: usize,
    live: usize,
}

impl<T> FreeList<T> {
    /// An empty pool.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            free: Vec::new(),
            created: 0,
            live: 0,
        }
    }

    /// An empty pool with room for `capacity` recycled values.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            free: Vec::with_capacity(capacity),
            created: 0,
            live: 0,
        }
    }

    /// Takes a value from the pool, creating one with `make` when it is empty.
    pub fn acquire(&mut self, make: impl FnOnce() -> T) -> T {
        self.live += 1;
        self.free.pop().unwrap_or_else(|| {
            self.created += 1;
            make()
        })
    }

    /// Returns a value to the pool.
    pub fn release(&mut self, value: T) {
        self.free.push(value);
        self.live = self.live.saturating_sub(1);
    }

    /// Returns a value after clearing it, the common pattern for a reusable
    /// buffer.
    pub fn release_cleared(&mut self, mut value: T)
    where
        T: Clear,
    {
        value.clear_items();
        self.release(value);
    }

    /// Number of values currently held by callers.
    #[inline]
    #[must_use]
    pub fn live(&self) -> usize {
        self.live
    }

    /// Number of values sitting in the pool, ready for reuse.
    #[inline]
    #[must_use]
    pub fn available(&self) -> usize {
        self.free.len()
    }

    /// Total values ever created. If this keeps rising, the pool is too small.
    #[inline]
    #[must_use]
    pub fn created(&self) -> usize {
        self.created
    }
}

impl<T> Default for FreeList<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Types that can be emptied for reuse (implemented by `Vec` and `String`).
pub trait Clear {
    /// Empty the value, keeping its allocation.
    fn clear_items(&mut self);
}

impl<T> Clear for Vec<T> {
    fn clear_items(&mut self) {
        self.clear();
    }
}

impl Clear for String {
    fn clear_items(&mut self) {
        self.clear();
    }
}

/// A dense, growable bit set.
///
/// The visibility system keeps one of these per frame ("which entities survived
/// culling"), and the ECS uses it for changed-component tracking. 64 elements
/// per word, so a 10 000-entity mask is 157 words and copies in microseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
        }
    }

    /// An empty set able to hold `bits` bits without reallocating.
    #[must_use]
    pub fn with_capacity(bits: usize) -> Self {
        Self {
            words: vec![0; bits.div_ceil(64)],
            len: 0,
        }
    }

    /// The number of bits the set can address without growing.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.words.len() * 64
    }

    /// The logical length: one past the highest set bit.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no bit is set.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count_ones() == 0
    }

    /// Grows the set so `bit` is addressable.
    pub fn reserve_bits(&mut self, bit: usize) {
        let needed = bit / 64 + 1;
        if needed > self.words.len() {
            self.words.resize(needed, 0);
        }
    }

    /// Sets a bit, growing as needed.
    #[inline]
    pub fn insert(&mut self, bit: usize) {
        self.reserve_bits(bit);
        self.words[bit / 64] |= 1u64 << (bit % 64);
        if bit >= self.len {
            self.len = bit + 1;
        }
    }

    /// Sets a bit without growing; returns `false` when it was out of range.
    #[inline]
    pub fn try_insert(&mut self, bit: usize) -> bool {
        if bit >= self.capacity() {
            return false;
        }
        self.words[bit / 64] |= 1u64 << (bit % 64);
        if bit >= self.len {
            self.len = bit + 1;
        }
        true
    }

    /// Clears a bit.
    #[inline]
    pub fn remove(&mut self, bit: usize) {
        if bit / 64 < self.words.len() {
            self.words[bit / 64] &= !(1u64 << (bit % 64));
        }
    }

    /// True when the bit is set.
    #[inline]
    #[must_use]
    pub fn contains(&self, bit: usize) -> bool {
        self.words
            .get(bit / 64)
            .is_some_and(|w| w & (1u64 << (bit % 64)) != 0)
    }

    /// Flips a bit, growing as needed.
    #[inline]
    pub fn toggle(&mut self, bit: usize) {
        if self.contains(bit) {
            self.remove(bit)
        } else {
            self.insert(bit)
        }
    }

    /// Sets every bit in `0..bits`.
    pub fn set_all(&mut self, bits: usize) {
        self.words.clear();
        self.words.resize(bits.div_ceil(64), u64::MAX);
        // Clear the tail bits beyond `bits` so `count_ones` is exact.
        if bits % 64 != 0 {
            let last = self.words.len() - 1;
            self.words[last] &= (1u64 << (bits % 64)) - 1;
        }
        self.len = bits;
    }

    /// Clears every bit, keeping the allocation.
    pub fn clear(&mut self) {
        for w in &mut self.words {
            *w = 0;
        }
        self.len = 0;
    }

    /// Number of set bits.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// True when no bit is shared with `other`.
    #[must_use]
    pub fn is_disjoint(&self, other: &BitSet) -> bool {
        let n = self.words.len().min(other.words.len());
        (0..n).all(|i| self.words[i] & other.words[i] == 0)
    }

    /// In-place union with `other`.
    pub fn union_with(&mut self, other: &BitSet) {
        self.reserve_bits(other.capacity().saturating_sub(1).max(0));
        for (i, w) in other.words.iter().enumerate() {
            if i < self.words.len() {
                self.words[i] |= *w;
            } else {
                self.words.push(*w);
            }
        }
        self.len = self.len.max(other.len);
    }

    /// In-place intersection with `other`.
    pub fn intersect_with(&mut self, other: &BitSet) {
        for (i, w) in self.words.iter_mut().enumerate() {
            *w &= other.words.get(i).copied().unwrap_or(0);
        }
        self.len = self.len.min(other.len);
    }

    /// In-place difference (`self \ other`).
    pub fn difference_with(&mut self, other: &BitSet) {
        for (i, w) in self.words.iter_mut().enumerate() {
            *w &= !other.words.get(i).copied().unwrap_or(0);
        }
    }

    /// Iterates the indices of set bits, ascending.
    pub fn iter(&self) -> BitSetIter<'_> {
        BitSetIter {
            set: self,
            word: 0,
            bits: 0,
        }
    }

    /// Number of words backing the set, for the GUI's statistics panel.
    #[must_use]
    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    /// An estimate of heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.words.len() * core::mem::size_of::<u64>()
    }
}

/// Iterator over the set bits of a [`BitSet`], ascending.
pub struct BitSetIter<'a> {
    set: &'a BitSet,
    word: usize,
    bits: u64,
}

impl Iterator for BitSetIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        loop {
            if self.bits != 0 {
                let b = self.bits.trailing_zeros() as usize;
                self.bits &= self.bits - 1;
                // `word` already points *past* the word these bits came from.
                return Some((self.word - 1) * 64 + b);
            }
            if self.word >= self.set.words.len() {
                return None;
            }
            self.bits = self.set.words[self.word];
            self.word += 1;
        }
    }
}

impl<'a> IntoIterator for &'a BitSet {
    type Item = usize;
    type IntoIter = BitSetIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A fixed-capacity ring buffer that overwrites the oldest entry when full.
///
/// The frame-time history, the render queue's recent-frame stats and the NPC
/// systems' debug traces all use it. Never allocates after construction.
#[derive(Clone, Debug)]
pub struct RingBuffer<T> {
    data: Vec<Option<T>>,
    head: usize,
    len: usize,
}

impl<T> RingBuffer<T> {
    /// Creates a buffer holding at most `capacity` values.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let n = capacity.max(1);
        let mut data = Vec::with_capacity(n);
        data.resize_with(n, || None);
        Self {
            data,
            head: 0,
            len: 0,
        }
    }

    /// Capacity.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Number of stored values.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True when full.
    #[inline]
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.len == self.data.len()
    }

    /// Pushes a value, evicting the oldest when full.
    pub fn push(&mut self, value: T) {
        let cap = self.data.len();
        self.data[self.head] = Some(value);
        self.head = (self.head + 1) % cap;
        if self.len < cap {
            self.len += 1;
        }
    }

    /// The most recently pushed value.
    #[must_use]
    pub fn last(&self) -> Option<&T> {
        if self.len == 0 {
            None
        } else {
            self.data[(self.head + self.data.len() - 1) % self.data.len()].as_ref()
        }
    }

    /// The oldest surviving value.
    #[must_use]
    pub fn first(&self) -> Option<&T> {
        if self.len == 0 {
            None
        } else {
            let start = (self.head + self.data.len() - self.len) % self.data.len();
            self.data[start].as_ref()
        }
    }

    /// Value at `index`, oldest first.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        let start = (self.head + self.data.len() - self.len) % self.data.len();
        self.data[(start + index) % self.data.len()].as_ref()
    }

    /// Iterates oldest-to-newest.
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        let cap = self.data.len();
        let start = (self.head + cap - self.len) % cap;
        (0..self.len).filter_map(move |i| self.data[(start + i) % cap].as_ref())
    }

    /// Mean of the stored values.
    #[must_use]
    pub fn mean(&self) -> f32
    where
        T: Copy + Into<f32>,
    {
        if self.len == 0 {
            return 0.0;
        }
        self.iter().map(|v| (*v).into()).sum::<f32>() / self.len as f32
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        for slot in &mut self.data {
            *slot = None;
        }
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slotmap_insert_get_remove() {
        let mut m = SlotMap::new();
        let a = m.insert(10u32);
        let b = m.insert(20);
        assert_eq!(m.get(a), Some(&10));
        assert_eq!(m.get(b), Some(&20));
        assert_eq!(m.len(), 2);
        assert_eq!(m.remove(a), Some(10));
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(a), None);
        assert!(m.contains(b));
    }

    #[test]
    fn stale_handle_does_not_resolve_after_reuse() {
        let mut m = SlotMap::new();
        let a = m.insert("a");
        m.remove(a);
        let b = m.insert("b");
        assert_eq!(a.index(), b.index(), "the slot should be reused");
        assert_ne!(a.generation(), b.generation());
        assert_eq!(
            m.get(a),
            None,
            "the stale handle must not see the new value"
        );
        assert_eq!(m.get(b), Some(&"b"));
    }

    #[test]
    fn handle_bits_roundtrip() {
        let mut m = SlotMap::new();
        let a = m.insert(1);
        let bits = a.to_bits();
        assert_eq!(Handle::<i32>::from_bits(bits), a);
    }

    #[test]
    fn get_two_mut_disjoint() {
        let mut m = SlotMap::new();
        let a = m.insert(1);
        let b = m.insert(2);
        {
            let (x, y) = m.get_two_mut(a, b).unwrap();
            *x = 10;
            *y = 20;
        }
        assert_eq!(m.get(a), Some(&10));
        assert_eq!(m.get(b), Some(&20));
        assert!(m.get_two_mut(a, a).is_none());
    }

    #[test]
    fn get_two_mut_rejects_stale() {
        let mut m = SlotMap::new();
        let a = m.insert(1);
        let b = m.insert(2);
        m.remove(a);
        assert!(m.get_two_mut(a, b).is_none());
    }

    #[test]
    fn slotmap_iteration_is_slot_order() {
        let mut m = SlotMap::new();
        let a = m.insert(1);
        let _b = m.insert(2);
        let c = m.insert(3);
        m.remove(a);
        let d = m.insert(4);
        let keys: Vec<u32> = m.keys().map(|h| h.index()).collect();
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "keys must ascend: {keys:?}"
        );
        assert!(m.contains(c) && m.contains(d));
    }

    #[test]
    fn slotmap_retain_removes_matching() {
        let mut m = SlotMap::new();
        for i in 0..10i32 {
            m.insert(i);
        }
        let removed = m.retain(|_h, v| *v % 2 == 0);
        assert_eq!(removed, 5);
        assert_eq!(m.len(), 5);
        assert!(m.values().all(|v| v % 2 == 0));
    }

    #[test]
    fn slotmap_clear_keeps_capacity() {
        let mut m = SlotMap::new();
        for i in 0..5 {
            m.insert(i);
        }
        let cap = m.capacity();
        m.clear();
        assert!(m.is_empty());
        assert_eq!(m.capacity(), cap);
    }

    #[test]
    fn freelist_reuses_values() {
        let mut p: FreeList<Vec<u8>> = FreeList::new();
        let a = p.acquire(Vec::new);
        assert_eq!(p.created(), 1);
        p.release(a);
        let _b = p.acquire(Vec::new);
        assert_eq!(p.created(), 1, "must not allocate a second buffer");
    }

    #[test]
    fn freelist_release_cleared() {
        let mut p: FreeList<Vec<u8>> = FreeList::new();
        let mut v = p.acquire(Vec::new);
        v.push(1);
        p.release_cleared(v);
        let v2 = p.acquire(Vec::new);
        assert!(v2.is_empty());
    }

    #[test]
    fn bitset_basic_ops() {
        let mut b = BitSet::new();
        b.insert(0);
        b.insert(63);
        b.insert(64);
        b.insert(1000);
        assert!(b.contains(0) && b.contains(63) && b.contains(64) && b.contains(1000));
        assert!(!b.contains(1));
        assert_eq!(b.count_ones(), 4);
        b.remove(63);
        assert_eq!(b.count_ones(), 3);
    }

    #[test]
    fn bitset_iterates_ascending() {
        let mut b = BitSet::new();
        for i in [70usize, 3, 64, 0, 129] {
            b.insert(i);
        }
        assert_eq!(b.iter().collect::<Vec<_>>(), vec![0, 3, 64, 70, 129]);
    }

    #[test]
    fn bitset_set_then_union() {
        let mut a = BitSet::new();
        a.insert(5);
        let mut b = BitSet::new();
        b.insert(300);
        a.union_with(&b);
        assert!(a.contains(5) && a.contains(300));
    }

    #[test]
    fn bitset_intersect_and_difference() {
        let mut a = BitSet::new();
        for i in 0..10 {
            a.insert(i);
        }
        let mut b = BitSet::new();
        for i in 5..15 {
            b.insert(i);
        }
        let mut c = a.clone();
        c.intersect_with(&b);
        assert_eq!(c.count_ones(), 5);
        let mut d = a.clone();
        d.difference_with(&b);
        assert_eq!(d.count_ones(), 5);
        assert!(!d.contains(5) && d.contains(4));
    }

    #[test]
    fn bitset_set_all_and_disjoint() {
        let mut a = BitSet::new();
        a.set_all(100);
        assert_eq!(a.count_ones(), 100);
        let mut b = BitSet::new();
        b.insert(100);
        assert!(a.is_disjoint(&b));
    }

    #[test]
    fn ring_buffer_wraps_and_keeps_order() {
        let mut r = RingBuffer::new(3);
        for i in 0..5 {
            r.push(i);
        }
        assert_eq!(r.len(), 3);
        assert_eq!(r.iter().copied().collect::<Vec<_>>(), vec![2, 3, 4]);
        assert_eq!(r.first(), Some(&2));
        assert_eq!(r.last(), Some(&4));
        assert_eq!(r.get(1), Some(&3));
    }

    #[test]
    fn ring_buffer_mean() {
        let mut r: RingBuffer<f32> = RingBuffer::new(4);
        r.push(1.0);
        r.push(2.0);
        assert!((r.mean() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn ring_buffer_capacity_one() {
        let mut r = RingBuffer::new(0);
        r.push(7);
        assert_eq!(r.len(), 1);
        assert_eq!(r.last(), Some(&7));
        assert_eq!(r.first(), Some(&7));
    }
}
