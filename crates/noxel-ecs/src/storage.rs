//! Component storage.
//!
//! One [`Storage<T>`] per component type, held behind a [`ErasedStorage`] trait
//! object so the world can keep them in a flat `Vec` and split-borrow two of
//! them at once (which is what makes a safe two-mutable-component query
//! possible without `unsafe`).
//!
//! # Layout
//!
//! A **sparse set**, not an archetype table:
//!
//! ```text
//! sparse:  [ slot 0 | slot 1 | slot 2 | ... ]      indexed by entity index
//!            { gen, dense } { INVALID } ...
//! dense:   [ (entity, value) | (entity, value) | ... ]   contiguous, iterable
//! ```
//!
//! Adding and removing a component is O(1); iterating is a linear scan of a
//! contiguous `Vec`. Noxel's game entities hold 4–20 components and the systems
//! are almost all "iterate one component type and look up one or two others",
//! which is exactly what a sparse set is good at. An archetype layout would win
//! on cache locality for very wide queries but costs a move on every component
//! add/remove; for a top-down RPG the sparse set is the better trade and it is
//! far easier to reason about.
//!
//! Removal uses `swap_remove`, so iteration order is *deterministic for a given
//! sequence of operations* but not insertion-ordered. Every system that cares
//! (pathfinding tie-breaks, physics contact ordering) sorts explicitly.

use core::any::Any;

use crate::entity::Entity;

/// Marker for the "no dense slot" sentinel.
const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct SparseEntry {
    generation: u32,
    dense: u32,
}

impl SparseEntry {
    const INVALID: Self = Self {
        generation: u32::MAX,
        dense: NONE,
    };
}

/// A sparse set of component values indexed by entity.
#[derive(Debug)]
pub struct Storage<T> {
    sparse: Vec<SparseEntry>,
    dense: Vec<(Entity, T)>,
    /// Monotonic change tick per dense slot, for change detection.
    changed: Vec<u32>,
}

impl<T> Storage<T> {
    /// An empty storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sparse: Vec::new(),
            dense: Vec::new(),
            changed: Vec::new(),
        }
    }

    /// An empty storage with room for `capacity` components.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            sparse: Vec::new(),
            dense: Vec::with_capacity(capacity),
            changed: Vec::with_capacity(capacity),
        }
    }

    /// Number of components stored.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    /// True when empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// Iterates `(entity, &value)` in dense order.
    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> + '_ {
        self.dense.iter().map(|(e, v)| (*e, v))
    }

    /// Iterates `(entity, &mut value)` in dense order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> + '_ {
        self.dense.iter_mut().map(|(e, v)| (*e, v))
    }

    /// Iterates entity ids in dense order.
    pub fn iter_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.dense.iter().map(|(e, _)| *e)
    }

    /// Inserts a value, returning the previous one if the entity already had it.
    pub fn insert(&mut self, entity: Entity, value: T, tick: u32) -> Option<T> {
        let idx = entity.index() as usize;
        if idx >= self.sparse.len() {
            self.sparse.resize(idx + 1, SparseEntry::INVALID);
        }
        let entry = self.sparse[idx];
        if entry.dense != NONE && entry.generation == entity.generation() {
            let slot = entry.dense as usize;
            let previous = core::mem::replace(&mut self.dense[slot].1, value);
            self.changed[slot] = tick;
            return Some(previous);
        }
        let dense = self.dense.len() as u32;
        self.dense.push((entity, value));
        self.changed.push(tick);
        self.sparse[idx] = SparseEntry {
            generation: entity.generation(),
            dense,
        };
        None
    }

    /// Removes and returns the value for `entity`.
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let idx = entity.index() as usize;
        let entry = *self.sparse.get(idx)?;
        if entry.dense == NONE || entry.generation != entity.generation() {
            return None;
        }
        let slot = entry.dense as usize;
        // Invalidate the entity's sparse entry *before* the swap so the moved
        // element can be re-pointed safely.
        self.sparse[idx] = SparseEntry::INVALID;
        // `swap_remove` returns the element at `slot` (the one being removed)
        // and moves the *last* element into `slot`. The moved element is
        // therefore whatever now sits at `slot`, not what was returned.
        let (_, value) = self.dense.swap_remove(slot);
        self.changed.swap_remove(slot);
        if slot < self.dense.len() {
            let moved = self.dense[slot].0;
            let moved_idx = moved.index() as usize;
            if let Some(s) = self.sparse.get_mut(moved_idx) {
                // Its dense index was the old last index, which is exactly the
                // (now shorter) length.
                if s.dense as usize == self.dense.len() {
                    s.dense = slot as u32;
                }
            }
        }
        Some(value)
    }

    /// Removes the component for `entity` if present, without returning it.
    #[inline]
    pub fn remove_dropped(&mut self, entity: Entity) {
        let _ = self.remove(entity);
    }

    /// Shared reference to an entity's component.
    #[inline]
    #[must_use]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        let idx = entity.index() as usize;
        let entry = *self.sparse.get(idx)?;
        if entry.dense == NONE || entry.generation != entity.generation() {
            return None;
        }
        self.dense.get(entry.dense as usize).map(|(_, v)| v)
    }

    /// Mutable reference to an entity's component.
    ///
    /// Bumps the change tick so change-detecting systems see the update. Use
    /// [`Storage::get_mut_silent`] when the mutation is not semantically a
    /// change (for example a cache refill).
    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, entity: Entity, tick: u32) -> Option<&mut T> {
        let idx = entity.index() as usize;
        let entry = *self.sparse.get(idx)?;
        if entry.dense == NONE || entry.generation != entity.generation() {
            return None;
        }
        let slot = entry.dense as usize;
        self.changed[slot] = tick;
        self.dense.get_mut(slot).map(|(_, v)| v)
    }

    /// Mutable reference that does **not** bump the change tick.
    #[inline]
    #[must_use]
    pub fn get_mut_silent(&mut self, entity: Entity) -> Option<&mut T> {
        let idx = entity.index() as usize;
        let entry = *self.sparse.get(idx)?;
        if entry.dense == NONE || entry.generation != entity.generation() {
            return None;
        }
        self.dense.get_mut(entry.dense as usize).map(|(_, v)| v)
    }

    /// True when the entity has this component.
    #[inline]
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.get(entity).is_some()
    }

    /// The change tick of an entity's component.
    #[inline]
    #[must_use]
    pub fn changed_at(&self, entity: Entity) -> Option<u32> {
        let idx = entity.index() as usize;
        let entry = *self.sparse.get(idx)?;
        if entry.dense == NONE || entry.generation != entity.generation() {
            return None;
        }
        self.changed.get(entry.dense as usize).copied()
    }

    /// Removes every component.
    pub fn clear(&mut self) {
        self.sparse.clear();
        self.dense.clear();
        self.changed.clear();
    }

    /// Removes components for entities that are no longer alive.
    ///
    /// Called by [`World::despawn`](crate::World::despawn) through the erased
    /// trait so a despawn does not need to know every component type.
    ///
    /// [`World::despawn`]: crate::World::despawn
    pub(crate) fn retain_alive(&mut self, alive: &dyn Fn(Entity) -> bool) {
        let mut i = 0;
        while i < self.dense.len() {
            if alive(self.dense[i].0) {
                i += 1;
                continue;
            }
            let (entity, _) = self.dense.swap_remove(i);
            self.changed.swap_remove(i);
            let idx = entity.index() as usize;
            if let Some(s) = self.sparse.get_mut(idx) {
                if s.generation == entity.generation() {
                    *s = SparseEntry::INVALID;
                }
            }
            // Re-point the swapped-in element.
            if i < self.dense.len() {
                let moved = self.dense[i].0;
                let m = moved.index() as usize;
                if let Some(s) = self.sparse.get_mut(m) {
                    if s.dense as usize == self.dense.len() {
                        s.dense = i as u32;
                    }
                }
            }
        }
    }

    /// Rough heap usage in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.sparse.capacity() * core::mem::size_of::<SparseEntry>()
            + self.dense.capacity() * (core::mem::size_of::<Entity>() + core::mem::size_of::<T>())
            + self.changed.capacity() * core::mem::size_of::<u32>()
    }
}

impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A type-erased component storage, so the world can hold many of them in one
/// collection and split-borrow two at a time.
///
/// Implemented by [`Storage<T>`] for every `T: 'static`. Contributors never
/// implement it by hand.
pub trait ErasedStorage: 'static {
    /// Downcast to a concrete [`Storage<T>`].
    fn as_any(&self) -> &dyn Any;
    /// Downcast to a concrete [`Storage<T>`] mutably.
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Removes the entity's component whatever the concrete type is.
    fn remove_entity(&mut self, entity: Entity);
    /// True when the entity has a component in this storage.
    fn contains_entity(&self, entity: Entity) -> bool;
    /// Number of components stored.
    fn storage_len(&self) -> usize;
    /// The concrete type's name, for diagnostics.
    fn type_name(&self) -> &'static str;
    /// Drops components belonging to dead entities.
    fn retain_alive_erased(&mut self, alive: &dyn Fn(Entity) -> bool);
    /// Heap usage estimate.
    fn storage_memory_bytes(&self) -> usize;
}

impl<T: 'static> ErasedStorage for Storage<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn remove_entity(&mut self, entity: Entity) {
        let _ = self.remove(entity);
    }
    fn contains_entity(&self, entity: Entity) -> bool {
        self.contains(entity)
    }
    fn storage_len(&self) -> usize {
        self.len()
    }
    fn type_name(&self) -> &'static str {
        core::any::type_name::<T>()
    }
    fn retain_alive_erased(&mut self, alive: &dyn Fn(Entity) -> bool) {
        self.retain_alive(alive);
    }
    fn storage_memory_bytes(&self) -> usize {
        self.memory_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32, g: u32) -> Entity {
        Entity::from_parts(i, g)
    }

    #[test]
    fn insert_get_remove() {
        let mut s: Storage<i32> = Storage::new();
        assert_eq!(s.insert(e(0, 0), 10, 0), None);
        assert_eq!(s.get(e(0, 0)), Some(&10));
        assert_eq!(s.len(), 1);
        assert_eq!(s.remove(e(0, 0)), Some(10));
        assert!(s.is_empty());
        assert_eq!(s.get(e(0, 0)), None);
    }

    #[test]
    fn insert_replaces() {
        let mut s: Storage<i32> = Storage::new();
        s.insert(e(0, 0), 1, 0);
        assert_eq!(s.insert(e(0, 0), 2, 0), Some(1));
        assert_eq!(s.get(e(0, 0)), Some(&2));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut s: Storage<i32> = Storage::new();
        s.insert(e(5, 1), 100, 0);
        assert_eq!(s.get(e(5, 2)), None, "wrong generation must not resolve");
        assert_eq!(s.remove(e(5, 2)), None);
        assert_eq!(s.get(e(5, 1)), Some(&100));
    }

    #[test]
    fn swap_remove_keeps_sparse_consistent() {
        let mut s: Storage<&'static str> = Storage::new();
        s.insert(e(0, 0), "a", 0);
        s.insert(e(1, 0), "b", 0);
        s.insert(e(2, 0), "c", 0);
        // Remove the first: "c" is swapped into slot 0.
        assert_eq!(s.remove(e(0, 0)), Some("a"));
        assert_eq!(s.get(e(1, 0)), Some(&"b"));
        assert_eq!(s.get(e(2, 0)), Some(&"c"));
        assert_eq!(s.len(), 2);
        // And the moved element must still be removable.
        assert_eq!(s.remove(e(2, 0)), Some("c"));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn iteration_visits_everything() {
        let mut s: Storage<u32> = Storage::new();
        for i in 0..10 {
            s.insert(e(i, 0), i * 3, 0);
        }
        let mut seen: Vec<u32> = s.iter().map(|(_, v)| *v).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..10).map(|i| i * 3).collect::<Vec<_>>());
    }

    #[test]
    fn change_tracking() {
        let mut s: Storage<i32> = Storage::new();
        s.insert(e(0, 0), 1, 7);
        assert_eq!(s.changed_at(e(0, 0)), Some(7));
        let _ = s.get_mut(e(0, 0), 9);
        assert_eq!(s.changed_at(e(0, 0)), Some(9));
        let _ = s.get_mut_silent(e(0, 0));
        assert_eq!(
            s.changed_at(e(0, 0)),
            Some(9),
            "silent access must not bump"
        );
    }

    #[test]
    fn retain_alive_drops_dead_entities() {
        let mut s: Storage<i32> = Storage::new();
        for i in 0..6 {
            s.insert(e(i, 0), i as i32, 0);
        }
        s.retain_alive(&|entity| entity.index() % 2 == 0);
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|(entity, _)| entity.index() % 2 == 0));
        // Sparse entries for dead entities must be invalidated.
        assert_eq!(s.get(e(1, 0)), None);
        // And surviving ones must still resolve.
        assert_eq!(s.get(e(4, 0)), Some(&4));
    }

    #[test]
    fn retain_alive_handles_adjacent_dead_entries() {
        let mut s: Storage<i32> = Storage::new();
        for i in 0..4 {
            s.insert(e(i, 0), i as i32, 0);
        }
        // Kill 1 and 2, which become adjacent after the first swap.
        s.retain_alive(&|entity| entity.index() == 0 || entity.index() == 3);
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(e(0, 0)), Some(&0));
        assert_eq!(s.get(e(3, 0)), Some(&3));
    }

    #[test]
    fn clear_empties() {
        let mut s: Storage<i32> = Storage::new();
        s.insert(e(0, 0), 1, 0);
        s.clear();
        assert!(s.is_empty());
        assert!(s.get(e(0, 0)).is_none());
    }

    #[test]
    fn erased_trait_works() {
        let mut boxed: Box<dyn ErasedStorage> = Box::new(Storage::<f32>::new());
        assert!(boxed.as_any_mut().downcast_mut::<Storage<f32>>().is_some());
        assert!(boxed.as_any().downcast_ref::<Storage<u8>>().is_none());
        assert_eq!(boxed.storage_len(), 0);
        assert!(boxed.type_name().contains("f32"));
        assert_eq!(boxed.storage_memory_bytes(), 0);
    }

    #[test]
    fn memory_estimate_grows_with_content() {
        let mut s: Storage<[u8; 64]> = Storage::new();
        s.insert(e(0, 0), [0; 64], 0);
        assert!(s.memory_bytes() >= 64);
    }
}
