//! The [`World`]: entity storage, components, resources and queries.
//!
//! ```no_run
//! use noxel_ecs::World;
//! use noxel_core::math::Vec3;
//!
//! #[derive(Debug, PartialEq)]
//! struct Position(Vec3);
//! struct Velocity(Vec3);
//!
//! let mut world = World::new();
//! let player = world.spawn_named("player");
//! world.insert(player, Position(Vec3::ZERO));
//! world.insert(player, Velocity(Vec3::new(1.0, 0.0, 0.0)));
//!
//! let dt = 1.0 / 60.0;
//! world.for_each2_mut::<Position, Velocity, _>(|_e, pos, vel| {
//!     pos.0 += vel.0 * dt;
//! });
//! assert!((world.get::<Position>(player).unwrap().0.x - dt as f32).abs() < 1e-6);
//! ```
//!
//! # Why closure-based queries
//!
//! Noxel's queries are `for_each*` closures rather than iterator combinators.
//! That is a deliberate trade: a closure-based query can hand out two
//! `&mut Storage<_>` at once through a checked split-borrow, so
//! [`World::for_each2_mut2`] is safe and needs no `unsafe`, no runtime borrow
//! tracking and no borrow-panics. It also compiles to a tight loop over a
//! contiguous `Vec`, and it sidesteps the lifetime gymnastics that an
//! `Iterator<Item = (&A, &mut B)>` implementation would require.
//!
//! System code that genuinely needs to collect and revisit entities calls
//! [`World::entities_with`] and then indexes the world.

use core::any::{Any, TypeId};
use std::collections::HashMap;

use noxel_core::pool::SlotMap;

use crate::entity::{Entity, EntityMeta};
use crate::storage::{ErasedStorage, Storage};

/// Marker for anything that can be attached to an entity.
///
/// Blanket-implemented for every `Send + Sync + 'static` type, so no derive
/// macro is needed: `struct Health(f32);` is already a component.
///
/// `Send + Sync` is required because the engine's job system may run
/// component-level work on other threads.
pub trait Component: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Component for T {}

/// Marker for singleton data stored on the world (config, clock, input state).
///
/// Resources are addressed by type: exactly one value of each type exists.
pub trait Resource: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Resource for T {}

/// Aggregate statistics, used by the debug overlay and by the demo's report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldStats {
    /// Live entities.
    pub entity_count: usize,
    /// Allocated entity slots (live + recycled).
    pub entity_capacity: usize,
    /// Number of distinct component types that have a storage.
    pub component_type_count: usize,
    /// Total components stored across every type.
    pub component_count: usize,
    /// Number of resources.
    pub resource_count: usize,
    /// Rough heap usage in bytes.
    pub memory_bytes: usize,
    /// Current change-detection tick.
    pub tick: u32,
}

/// The entity/component world.
pub struct World {
    entities: SlotMap<EntityMeta>,
    storages: Vec<Box<dyn ErasedStorage>>,
    storage_index: HashMap<TypeId, usize>,
    resources: HashMap<TypeId, Box<dyn Any>>,
    names: HashMap<String, Entity>,
    tick: u32,
}

impl World {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: SlotMap::new(),
            storages: Vec::new(),
            storage_index: HashMap::new(),
            resources: HashMap::new(),
            names: HashMap::new(),
            tick: 1,
        }
    }

    /// An empty world with room for `capacity` entities.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let mut w = Self::new();
        w.entities.reserve(capacity);
        w
    }

    // ---------------------------------------------------------------- tick

    /// The current change-detection tick.
    #[inline]
    #[must_use]
    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Advances the change tick. Call once per frame, before the systems that
    /// mutate components.
    ///
    /// Ticks wrap at `u32::MAX`; a comparison against a tick more than 2^31
    /// frames old is therefore meaningless, which is never a problem in
    /// practice (2^31 frames is over a year at 60 Hz).
    #[inline]
    pub fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    // ------------------------------------------------------------ entities

    /// Creates an unnamed entity.
    pub fn spawn(&mut self) -> Entity {
        let handle = self.entities.insert(EntityMeta::new());
        Entity::from_handle_of(handle)
    }

    /// Creates an entity and registers a lookup name for it.
    ///
    /// Names are for scripting, the debug overlay and tests — not for gameplay
    /// logic, which should hold the [`Entity`] id. Re-using a name replaces the
    /// old binding; the old entity stays alive.
    pub fn spawn_named(&mut self, name: impl Into<String>) -> Entity {
        let entity = self.spawn();
        self.set_name(entity, name);
        entity
    }

    /// Binds (or rebinds) a name to an entity.
    pub fn set_name(&mut self, entity: Entity, name: impl Into<String>) {
        let name = name.into();
        if let Some(previous) = self.names.get(&name).copied() {
            if previous != entity {
                if let Some(meta) = self.entities.get_mut(previous.to_handle_of()) {
                    meta.name = None;
                }
            }
        }
        if let Some(meta) = self.entities.get_mut(entity.to_handle_of()) {
            meta.name = Some(name.clone());
        }
        self.names.insert(name, entity);
    }

    /// The name bound to an entity, if any.
    #[must_use]
    pub fn name(&self, entity: Entity) -> Option<&str> {
        self.entities
            .get(entity.to_handle_of())
            .and_then(|m| m.name.as_deref())
    }

    /// Looks up an entity by name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<Entity> {
        self.names.get(name).copied().filter(|e| self.is_alive(*e))
    }

    /// Destroys an entity and every component attached to it.
    ///
    /// Returns `false` when the id was already stale.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        for storage in &mut self.storages {
            storage.remove_entity(entity);
        }
        if let Some(meta) = self.entities.get(entity.to_handle_of()) {
            if let Some(name) = meta.name.clone() {
                self.names.remove(&name);
            }
        }
        self.entities.remove(entity.to_handle_of()).is_some()
    }

    /// True when the id refers to a live entity.
    #[inline]
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        !entity.is_placeholder() && self.entities.contains(entity.to_handle_of())
    }

    /// Number of live entities.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// True when the world holds no entities.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Iterates live entity ids in slot order (deterministic).
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.keys().map(Entity::from_handle_of)
    }

    /// Reserves room for `additional` more entities.
    pub fn reserve_entities(&mut self, additional: usize) {
        self.entities.reserve(additional);
    }

    // ---------------------------------------------------------- components

    /// Returns the index of the storage for `T`, creating it on demand.
    fn storage_index_of<T: Component>(&mut self) -> usize {
        let id = TypeId::of::<T>();
        if let Some(&i) = self.storage_index.get(&id) {
            return i;
        }
        let i = self.storages.len();
        self.storages.push(Box::new(Storage::<T>::new()));
        self.storage_index.insert(id, i);
        i
    }

    /// The storage for `T`, if the type has ever been used.
    #[must_use]
    pub fn storage<T: Component>(&self) -> Option<&Storage<T>> {
        let i = *self.storage_index.get(&TypeId::of::<T>())?;
        self.storages[i].as_any().downcast_ref::<Storage<T>>()
    }

    /// The storage for `T` mutably, if the type has ever been used.
    #[must_use]
    pub fn storage_mut<T: Component>(&mut self) -> Option<&mut Storage<T>> {
        let i = *self.storage_index.get(&TypeId::of::<T>())?;
        self.storages[i].as_any_mut().downcast_mut::<Storage<T>>()
    }

    /// The storage for `T`, creating an empty one if needed.
    pub fn ensure_storage<T: Component>(&mut self) -> &mut Storage<T> {
        let i = self.storage_index_of::<T>();
        self.storages[i]
            .as_any_mut()
            .downcast_mut::<Storage<T>>()
            .expect("storage index and TypeId must agree")
    }

    /// Attaches a component, returning the previous value if there was one.
    ///
    /// Returns `None` for a stale entity id *and* for a fresh insert — use
    /// [`World::is_alive`] first when the distinction matters.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        let tick = self.tick;
        let had = {
            let storage = self.ensure_storage::<T>();
            storage.contains(entity)
        };
        let previous = self.ensure_storage::<T>().insert(entity, value, tick);
        if !had {
            if let Some(meta) = self.entities.get_mut(entity.to_handle_of()) {
                meta.component_count += 1;
            }
        }
        previous
    }

    /// Removes a component, returning it.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        let storage = self.storage_mut::<T>()?;
        let removed = storage.remove(entity);
        if removed.is_some() {
            if let Some(meta) = self.entities.get_mut(entity.to_handle_of()) {
                meta.component_count = meta.component_count.saturating_sub(1);
            }
        }
        removed
    }

    /// Shared reference to an entity's component.
    #[inline]
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.storage::<T>()?.get(entity)
    }

    /// Mutable reference to an entity's component; marks it changed.
    #[inline]
    #[must_use]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        let tick = self.tick;
        self.storage_mut::<T>()?.get_mut(entity, tick)
    }

    /// Mutable reference that does not mark the component changed.
    #[inline]
    #[must_use]
    pub fn get_mut_silent<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        self.storage_mut::<T>()?.get_mut_silent(entity)
    }

    /// True when the entity carries `T`.
    #[inline]
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        self.storage::<T>().is_some_and(|s| s.contains(entity))
    }

    /// The tick at which `T` last changed for this entity.
    #[must_use]
    pub fn changed_at<T: Component>(&self, entity: Entity) -> Option<u32> {
        self.storage::<T>()?.changed_at(entity)
    }

    /// Number of entities carrying `T`.
    #[must_use]
    pub fn count_with<T: Component>(&self) -> usize {
        self.storage::<T>().map_or(0, Storage::len)
    }

    /// Every entity carrying `T`, in storage order.
    #[must_use]
    pub fn entities_with<T: Component>(&self) -> Vec<Entity> {
        self.storage::<T>()
            .map(|s| s.iter_entities().collect())
            .unwrap_or_default()
    }

    /// Every entity id with its `&T`, collecting to a `Vec`.
    ///
    /// Allocating; for the non-allocating form use [`World::for_each`].
    #[must_use]
    pub fn collect_with<T: Component>(&self) -> Vec<(Entity, &T)> {
        self.storage::<T>()
            .map(|s| s.iter().collect())
            .unwrap_or_default()
    }

    // ------------------------------------------------------------ resources

    /// Inserts a resource, returning the previous value of the same type.
    pub fn insert_resource<R: Resource>(&mut self, value: R) -> Option<R> {
        let previous = self.resources.remove(&TypeId::of::<R>());
        self.resources.insert(TypeId::of::<R>(), Box::new(value));
        previous.and_then(|b| b.downcast::<R>().ok().map(|b| *b))
    }

    /// Shared reference to a resource.
    #[must_use]
    pub fn resource<R: Resource>(&self) -> Option<&R> {
        self.resources.get(&TypeId::of::<R>())?.downcast_ref::<R>()
    }

    /// Mutable reference to a resource.
    #[must_use]
    pub fn resource_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())?
            .downcast_mut::<R>()
    }

    /// The resource if present, or the value produced by `default`.
    ///
    /// The default is **not** stored; use [`World::insert_resource`] for that.
    #[must_use]
    pub fn resource_or_default<R: Resource + Default + Clone>(&self) -> R {
        self.resource::<R>().cloned().unwrap_or_default()
    }

    /// Removes and returns a resource.
    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .and_then(|b| b.downcast::<R>().ok().map(|b| *b))
    }

    /// True when the resource exists.
    #[must_use]
    pub fn has_resource<R: Resource>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    // -------------------------------------------------------------- queries

    /// Iterates every entity carrying `A`.
    pub fn for_each<A: Component, F: FnMut(Entity, &A)>(&self, mut f: F) {
        let Some(sa) = self.storage::<A>() else {
            return;
        };
        for (entity, a) in sa.iter() {
            f(entity, a);
        }
    }

    /// Iterates every entity carrying `A`, with mutable access.
    ///
    /// The component is marked changed only when `f` is called for it, so a
    /// no-op system listening for a rare event does not dirty the whole world.
    pub fn for_each_mut<A: Component, F: FnMut(Entity, &mut A)>(&mut self, mut f: F) {
        let tick = self.tick;
        let Some(sa) = self.storage_mut::<A>() else {
            return;
        };
        for (entity, a) in sa.iter_mut() {
            f(entity, a);
        }
        // `iter_mut` on Storage does not bump ticks by itself; do it once for the
        // whole pass so `for_each_changed` sees the update.
        if let Some(s) = self.storage_mut::<A>() {
            for entity in s.iter_entities().collect::<Vec<_>>() {
                let _ = s.get_mut(entity, tick);
            }
        }
    }

    /// Iterates entities carrying both `A` and `B`.
    pub fn for_each2<A: Component, B: Component, F: FnMut(Entity, &A, &B)>(&self, mut f: F) {
        let (Some(sa), Some(sb)) = (self.storage::<A>(), self.storage::<B>()) else {
            return;
        };
        // Drive the loop from the smaller storage: fewer lookups for the scan.
        if sa.len() <= sb.len() {
            for (entity, a) in sa.iter() {
                if let Some(b) = sb.get(entity) {
                    f(entity, a, b);
                }
            }
        } else {
            for (entity, b) in sb.iter() {
                if let Some(a) = sa.get(entity) {
                    f(entity, a, b);
                }
            }
        }
    }

    /// Iterates entities carrying both, with `A` mutable.
    pub fn for_each2_mut<A: Component, B: Component, F: FnMut(Entity, &mut A, &B)>(
        &mut self,
        mut f: F,
    ) {
        let (ia, ib) = match self.index_pair::<A, B>() {
            Some(p) => p,
            None => return,
        };
        let tick = self.tick;
        let Some((sa, sb)) = self.pair_mut_by_index(ia, ib) else {
            return;
        };
        let sa = sa
            .as_any_mut()
            .downcast_mut::<Storage<A>>()
            .expect("index/A mismatch");
        let sb = sb
            .as_any()
            .downcast_ref::<Storage<B>>()
            .expect("index/B mismatch");
        if sa.len() <= sb.len() {
            let entities: Vec<Entity> = sa.iter_entities().collect();
            for entity in entities {
                if sb.contains(entity) {
                    let b = sb.get(entity).expect("checked");
                    let Some(a) = sa.get_mut(entity, tick) else {
                        continue;
                    };
                    f(entity, a, b);
                }
            }
        } else {
            let pairs: Vec<(Entity, &B)> = sb.iter().collect();
            for (entity, b) in pairs {
                let Some(a) = sa.get_mut(entity, tick) else {
                    continue;
                };
                f(entity, a, b);
            }
        }
    }

    /// Iterates entities carrying both, with both mutable.
    pub fn for_each2_mut2<A: Component, B: Component, F: FnMut(Entity, &mut A, &mut B)>(
        &mut self,
        mut f: F,
    ) {
        let (ia, ib) = match self.index_pair::<A, B>() {
            Some(p) => p,
            None => return,
        };
        let tick = self.tick;
        let Some((sa, sb)) = self.pair_mut_by_index(ia, ib) else {
            return;
        };
        let sa = sa
            .as_any_mut()
            .downcast_mut::<Storage<A>>()
            .expect("index/A mismatch");
        let sb = sb
            .as_any_mut()
            .downcast_mut::<Storage<B>>()
            .expect("index/B mismatch");
        // Walk whichever storage is smaller, collecting ids first so the
        // lookups do not hold a borrow of the other table.
        let driver: Vec<Entity> = if sa.len() <= sb.len() {
            sa.iter_entities().filter(|e| sb.contains(*e)).collect()
        } else {
            sb.iter_entities().filter(|e| sa.contains(*e)).collect()
        };
        for entity in driver {
            // Split the two mutable borrows per iteration: `get_mut` on one and
            // the other are disjoint values, so this is sound without `unsafe`
            // because the two storages are different objects.
            let (a, b) = (sa.get_mut(entity, tick), sb.get_mut(entity, tick));
            if let (Some(a), Some(b)) = (a, b) {
                f(entity, a, b);
            }
        }
    }

    /// Iterates entities carrying all three components.
    pub fn for_each3<A: Component, B: Component, C: Component, F: FnMut(Entity, &A, &B, &C)>(
        &self,
        mut f: F,
    ) {
        let (Some(sa), Some(sb), Some(sc)) = (
            self.storage::<A>(),
            self.storage::<B>(),
            self.storage::<C>(),
        ) else {
            return;
        };
        let driver = [sa.len(), sb.len(), sc.len()]
            .iter()
            .enumerate()
            .min_by_key(|x| *x.1)
            .map(|x| x.0)
            .unwrap_or(0);
        match driver {
            0 => {
                for (e, a) in sa.iter() {
                    if let (Some(b), Some(c)) = (sb.get(e), sc.get(e)) {
                        f(e, a, b, c);
                    }
                }
            }
            1 => {
                for (e, b) in sb.iter() {
                    if let (Some(a), Some(c)) = (sa.get(e), sc.get(e)) {
                        f(e, a, b, c);
                    }
                }
            }
            _ => {
                for (e, c) in sc.iter() {
                    if let (Some(a), Some(b)) = (sa.get(e), sb.get(e)) {
                        f(e, a, b, c);
                    }
                }
            }
        }
    }

    /// Iterates entities carrying all three, with `A` mutable.
    pub fn for_each3_mut<
        A: Component,
        B: Component,
        C: Component,
        F: FnMut(Entity, &mut A, &B, &C),
    >(
        &mut self,
        mut f: F,
    ) {
        let tick = self.tick;
        let Some([sa, sb, sc]) = self.three_by_type::<A, B, C>() else {
            return;
        };
        let sa = sa
            .as_any_mut()
            .downcast_mut::<Storage<A>>()
            .expect("A mismatch");
        let sb = sb
            .as_any()
            .downcast_ref::<Storage<B>>()
            .expect("B mismatch");
        let sc = sc
            .as_any()
            .downcast_ref::<Storage<C>>()
            .expect("C mismatch");
        let driver: Vec<Entity> = sa
            .iter_entities()
            .filter(|e| sb.contains(*e) && sc.contains(*e))
            .collect();
        for entity in driver {
            let (Some(b), Some(c)) = (sb.get(entity), sc.get(entity)) else {
                continue;
            };
            let Some(a) = sa.get_mut(entity, tick) else {
                continue;
            };
            f(entity, a, b, c);
        }
    }

    /// Iterates entities carrying all three, with all three mutable.
    pub fn for_each3_mut3<
        A: Component,
        B: Component,
        C: Component,
        F: FnMut(Entity, &mut A, &mut B, &mut C),
    >(
        &mut self,
        mut f: F,
    ) {
        let tick = self.tick;
        let Some([sa, sb, sc]) = self.three_by_type::<A, B, C>() else {
            return;
        };
        let sa = sa
            .as_any_mut()
            .downcast_mut::<Storage<A>>()
            .expect("A mismatch");
        let sb = sb
            .as_any_mut()
            .downcast_mut::<Storage<B>>()
            .expect("B mismatch");
        let sc = sc
            .as_any_mut()
            .downcast_mut::<Storage<C>>()
            .expect("C mismatch");
        let driver: Vec<Entity> = sa
            .iter_entities()
            .filter(|e| sb.contains(*e) && sc.contains(*e))
            .collect();
        for entity in driver {
            let (a, b, c) = (
                sa.get_mut(entity, tick),
                sb.get_mut(entity, tick),
                sc.get_mut(entity, tick),
            );
            if let (Some(a), Some(b), Some(c)) = (a, b, c) {
                f(entity, a, b, c);
            }
        }
    }

    /// Iterates entities whose `A` changed at or after `since`.
    pub fn for_each_changed<A: Component, F: FnMut(Entity, &A)>(&self, since: u32, mut f: F) {
        let Some(sa) = self.storage::<A>() else {
            return;
        };
        for (entity, a) in sa.iter() {
            if sa.changed_at(entity).is_some_and(|t| t >= since) {
                f(entity, a);
            }
        }
    }

    /// Iterates entities whose `A` changed at or after `since`, mutably.
    pub fn for_each_changed_mut<A: Component, F: FnMut(Entity, &mut A)>(
        &mut self,
        since: u32,
        mut f: F,
    ) {
        let tick = self.tick;
        let Some(sa) = self.storage_mut::<A>() else {
            return;
        };
        let targets: Vec<Entity> = sa
            .iter()
            .filter(|(e, _)| sa.changed_at(*e).is_some_and(|t| t >= since))
            .map(|(e, _)| e)
            .collect();
        for entity in targets {
            if let Some(a) = sa.get_mut(entity, tick) {
                f(entity, a);
            }
        }
    }

    // ------------------------------------------------------ borrow helpers

    fn index_pair<A: Component, B: Component>(&mut self) -> Option<(usize, usize)> {
        let ia = self.storage_index_of::<A>();
        let ib = self.storage_index_of::<B>();
        if ia == ib { None } else { Some((ia, ib)) }
    }

    /// Two disjoint erased storages, in the order requested.
    fn pair_mut_by_index(
        &mut self,
        ia: usize,
        ib: usize,
    ) -> Option<(&mut dyn ErasedStorage, &mut dyn ErasedStorage)> {
        if ia == ib {
            return None;
        }
        let (lo, hi) = if ia < ib { (ia, ib) } else { (ib, ia) };
        let (left, right) = self.storages.split_at_mut(hi);
        let a = left.get_mut(lo)?.as_mut();
        let b = right.first_mut()?.as_mut();
        Some(if ia < ib { (a, b) } else { (b, a) })
    }

    /// Three disjoint erased storages, in the order requested.
    fn three_by_type<A: Component, B: Component, C: Component>(
        &mut self,
    ) -> Option<[&mut dyn ErasedStorage; 3]> {
        let ia = self.storage_index_of::<A>();
        let ib = self.storage_index_of::<B>();
        let ic = self.storage_index_of::<C>();
        if ia == ib || ib == ic || ia == ic {
            return None;
        }
        // Sort by storage index and split the vec twice, then reassemble in the
        // caller's requested order.
        let mut order = [(ia, 0usize), (ib, 1usize), (ic, 2usize)];
        order.sort_unstable_by_key(|x| x.0);
        let (i0, i1, i2) = (order[0].0, order[1].0, order[2].0);
        let (first, rest) = self.storages.split_at_mut(i1);
        let (second, third) = rest.split_at_mut(i2 - i1);
        let mut out: [Option<&mut dyn ErasedStorage>; 3] = [None, None, None];
        out[order[0].1] = Some(first.get_mut(i0)?.as_mut());
        out[order[1].1] = Some(second.first_mut()?.as_mut());
        out[order[2].1] = Some(third.first_mut()?.as_mut());
        let [x, y, z] = out;
        Some([x?, y?, z?])
    }

    // ----------------------------------------------------------- bulk ops

    /// Destroys every entity and component, keeping the allocations.
    pub fn clear(&mut self) {
        for storage in &mut self.storages {
            storage.retain_alive_erased(&|_| false);
        }
        self.entities.clear();
        self.names.clear();
    }

    /// Destroys every entity, every component **and** every resource.
    pub fn reset(&mut self) {
        self.clear();
        self.storages.clear();
        self.storage_index.clear();
        self.resources.clear();
    }

    /// Drops components belonging to entities that are no longer alive.
    ///
    /// [`World::despawn`] already does this, so this is a repair tool for code
    /// that manipulated storages directly.
    pub fn retain_alive_components(&mut self) {
        let alive_ids: Vec<Entity> = self.entities().collect();
        let alive = |e: Entity| alive_ids.binary_search(&e).is_ok();
        for storage in &mut self.storages {
            storage.retain_alive_erased(&alive);
        }
    }

    /// Aggregate statistics.
    #[must_use]
    pub fn stats(&self) -> WorldStats {
        let component_count: usize = self.storages.iter().map(|s| s.storage_len()).sum();
        let memory_bytes: usize = self
            .storages
            .iter()
            .map(|s| s.storage_memory_bytes())
            .sum::<usize>()
            + self.entities.capacity() * core::mem::size_of::<EntityMeta>();
        WorldStats {
            entity_count: self.entities.len(),
            entity_capacity: self.entities.capacity(),
            component_type_count: self.storages.len(),
            component_count,
            resource_count: self.resources.len(),
            memory_bytes,
            tick: self.tick,
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for World {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = self.stats();
        f.debug_struct("World")
            .field("entities", &s.entity_count)
            .field("component_types", &s.component_type_count)
            .field("components", &s.component_count)
            .field("resources", &s.resource_count)
            .field("tick", &s.tick)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Pos(i32);
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Vel(i32);
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Health(i32);
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Marker;

    #[test]
    fn spawn_despawn_alive() {
        let mut w = World::new();
        let e = w.spawn();
        assert!(w.is_alive(e));
        assert_eq!(w.len(), 1);
        assert!(w.despawn(e));
        assert!(!w.is_alive(e));
        assert_eq!(w.len(), 0);
        assert!(!w.despawn(e), "second despawn is a no-op");
    }

    #[test]
    fn stale_id_does_not_resolve_after_slot_reuse() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1));
        w.despawn(a);
        let b = w.spawn();
        assert_eq!(a.index(), b.index(), "slot should be recycled");
        assert_ne!(a, b);
        assert!(!w.is_alive(a));
        assert_eq!(
            w.get::<Pos>(a),
            None,
            "stale id must not see the new entity"
        );
        w.insert(b, Pos(2));
        assert_eq!(w.get::<Pos>(b), Some(&Pos(2)));
    }

    #[test]
    fn insert_get_remove_component() {
        let mut w = World::new();
        let e = w.spawn();
        assert_eq!(w.insert(e, Pos(5)), None);
        assert_eq!(w.get::<Pos>(e), Some(&Pos(5)));
        assert!(w.has::<Pos>(e));
        assert_eq!(w.insert(e, Pos(6)), Some(Pos(5)));
        assert_eq!(w.remove::<Pos>(e), Some(Pos(6)));
        assert!(!w.has::<Pos>(e));
        assert_eq!(w.remove::<Pos>(e), None);
    }

    #[test]
    fn inserting_on_dead_entity_is_ignored() {
        let mut w = World::new();
        let e = w.spawn();
        w.despawn(e);
        assert_eq!(w.insert(e, Pos(1)), None);
        assert_eq!(w.get::<Pos>(e), None);
    }

    #[test]
    fn despawn_removes_every_component() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        w.insert(e, Vel(2));
        w.insert(e, Health(3));
        w.despawn(e);
        assert_eq!(w.count_with::<Pos>(), 0);
        assert_eq!(w.count_with::<Vel>(), 0);
        assert_eq!(w.count_with::<Health>(), 0);
        assert_eq!(w.stats().component_count, 0);
    }

    #[test]
    fn names_resolve_and_clear() {
        let mut w = World::new();
        let e = w.spawn_named("player");
        assert_eq!(w.find("player"), Some(e));
        assert_eq!(w.name(e), Some("player"));
        w.despawn(e);
        assert_eq!(w.find("player"), None);
        assert_eq!(w.name(e), None);
    }

    #[test]
    fn renaming_releases_the_old_binding() {
        let mut w = World::new();
        let a = w.spawn_named("thing");
        let b = w.spawn();
        w.set_name(b, "thing");
        assert_eq!(w.find("thing"), Some(b));
        assert_eq!(w.name(a), None);
        assert!(w.is_alive(a), "the old entity must survive the rename");
    }

    #[test]
    fn for_each_visits_all() {
        let mut w = World::new();
        for i in 0..5 {
            let e = w.spawn();
            w.insert(e, Pos(i));
        }
        let mut sum = 0;
        w.for_each::<Pos, _>(|_e, p| sum += p.0);
        assert_eq!(sum, 10);
    }

    #[test]
    fn for_each_mut_updates() {
        let mut w = World::new();
        for i in 0..5 {
            let e = w.spawn();
            w.insert(e, Pos(i));
        }
        w.for_each_mut::<Pos, _>(|_e, p| p.0 *= 2);
        let mut sum = 0;
        w.for_each::<Pos, _>(|_e, p| sum += p.0);
        assert_eq!(sum, 20);
    }

    #[test]
    fn for_each2_requires_both() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1));
        w.insert(a, Vel(10));
        let b = w.spawn();
        w.insert(b, Pos(2)); // no Vel
        let mut hits = 0;
        w.for_each2::<Pos, Vel, _>(|_e, _p, _v| hits += 1);
        assert_eq!(hits, 1);
    }

    #[test]
    fn for_each2_drives_from_either_side() {
        // Two entities with Vel, one of which also has Pos: exercises both the
        // "A is smaller" and "B is smaller" branches.
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1));
        w.insert(a, Vel(1));
        let b = w.spawn();
        w.insert(b, Vel(2));
        let mut count = 0;
        w.for_each2::<Pos, Vel, _>(|_e, p, v| {
            assert_eq!(p.0, 1);
            assert_eq!(v.0, 1);
            count += 1;
        });
        assert_eq!(count, 1);
    }

    #[test]
    fn for_each2_mut_writes_one_side() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(0));
        w.insert(a, Vel(7));
        w.for_each2_mut::<Pos, Vel, _>(|_e, p, v| p.0 = v.0);
        assert_eq!(w.get::<Pos>(a), Some(&Pos(7)));
    }

    #[test]
    fn for_each2_mut2_writes_both() {
        let mut w = World::new();
        for i in 0..4 {
            let e = w.spawn();
            w.insert(e, Pos(i));
            w.insert(e, Vel(i));
        }
        w.for_each2_mut2::<Pos, Vel, _>(|_e, p, v| {
            p.0 += 1;
            v.0 *= 10;
        });
        let mut ps = Vec::new();
        w.for_each::<Pos, _>(|_e, p| ps.push(p.0));
        ps.sort_unstable();
        assert_eq!(ps, vec![1, 2, 3, 4]);
        let mut vs = Vec::new();
        w.for_each::<Vel, _>(|_e, v| vs.push(v.0));
        vs.sort_unstable();
        assert_eq!(vs, vec![0, 10, 20, 30]);
    }

    #[test]
    fn same_type_twice_is_rejected_not_panicking() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        // Aliasing the same storage must be a no-op, never UB or a panic.
        w.for_each2_mut2::<Pos, Pos, _>(|_e, a, b| {
            a.0 = 99;
            b.0 = 99;
        });
        assert_eq!(w.get::<Pos>(e), Some(&Pos(1)));
    }

    #[test]
    fn for_each3_requires_all_three() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1));
        w.insert(a, Vel(1));
        w.insert(a, Health(1));
        let b = w.spawn();
        w.insert(b, Pos(2));
        w.insert(b, Vel(2));
        let mut hits = 0;
        w.for_each3::<Pos, Vel, Health, _>(|_e, _p, _v, h| {
            assert_eq!(h.0, 1);
            hits += 1;
        });
        assert_eq!(hits, 1);
    }

    #[test]
    fn for_each3_mut3_writes_all() {
        let mut w = World::new();
        for i in 0..3 {
            let e = w.spawn();
            w.insert(e, Pos(i));
            w.insert(e, Vel(i));
            w.insert(e, Health(10));
        }
        w.for_each3_mut3::<Pos, Vel, Health, _>(|_e, p, v, h| {
            p.0 += 1;
            v.0 += 2;
            h.0 -= 1;
        });
        assert_eq!(
            w.get::<Health>(w.entities().next().unwrap()),
            Some(&Health(9))
        );
        let mut total = 0;
        w.for_each::<Pos, _>(|_e, p| total += p.0);
        assert_eq!(total, 3 + 3); // 0+1+2 + 1 each
    }

    #[test]
    fn empty_world_queries_are_noops() {
        let mut w = World::new();
        w.for_each::<Pos, _>(|_, _| panic!("must not be called"));
        w.for_each_mut::<Pos, _>(|_, _| panic!("must not be called"));
        w.for_each2::<Pos, Vel, _>(|_, _, _| panic!("must not be called"));
        w.for_each2_mut2::<Pos, Vel, _>(|_, _, _| panic!("must not be called"));
        w.for_each3::<Pos, Vel, Health, _>(|_, _, _, _| panic!("must not be called"));
        w.for_each3_mut3::<Pos, Vel, Health, _>(|_, _, _, _| panic!("must not be called"));
    }

    #[test]
    fn resources_roundtrip() {
        #[derive(Debug, PartialEq, Clone, Copy, Default)]
        struct Gravity(f32);
        let mut w = World::new();
        assert!(!w.has_resource::<Gravity>());
        assert_eq!(w.insert_resource(Gravity(9.8)), None);
        assert_eq!(w.resource::<Gravity>(), Some(&Gravity(9.8)));
        w.resource_mut::<Gravity>().unwrap().0 = 1.6;
        assert_eq!(w.resource::<Gravity>(), Some(&Gravity(1.6)));
        assert_eq!(w.insert_resource(Gravity(0.0)), Some(Gravity(1.6)));
        assert_eq!(w.remove_resource::<Gravity>(), Some(Gravity(0.0)));
        assert!(!w.has_resource::<Gravity>());
    }

    #[test]
    fn distinct_resource_types_coexist() {
        struct A(u32);
        struct B(&'static str);
        let mut w = World::new();
        w.insert_resource(A(1));
        w.insert_resource(B("two"));
        assert_eq!(w.resource::<A>().unwrap().0, 1);
        assert_eq!(w.resource::<B>().unwrap().0, "two");
        assert_eq!(w.stats().resource_count, 2);
    }

    #[test]
    fn change_detection_reports_recent_writes() {
        let mut w = World::new();
        let a = w.spawn();
        let b = w.spawn();
        w.insert(a, Pos(1));
        w.insert(b, Pos(2));

        w.advance_tick();
        let since = w.tick();
        w.get_mut::<Pos>(a).unwrap().0 = 99;

        let mut touched = Vec::new();
        w.for_each_changed::<Pos, _>(since, |e, _p| touched.push(e));
        assert_eq!(touched, vec![a], "only the written component is reported");
    }

    #[test]
    fn get_mut_silent_does_not_mark_changed() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        w.advance_tick();
        let since = w.tick();
        w.get_mut_silent::<Pos>(e).unwrap().0 = 2;
        let mut count = 0;
        w.for_each_changed::<Pos, _>(since, |_, _| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn changed_mut_can_write_again() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        // The insert stamped the component with the current tick; asking "since
        // that tick" must include it.
        let since = w.tick();
        w.for_each_changed_mut::<Pos, _>(since, |_e, p| p.0 += 100);
        assert_eq!(w.get::<Pos>(e), Some(&Pos(101)));
    }

    #[test]
    fn collect_and_count_helpers() {
        let mut w = World::new();
        for i in 0..4 {
            let e = w.spawn();
            w.insert(e, Pos(i));
        }
        assert_eq!(w.count_with::<Pos>(), 4);
        assert_eq!(w.entities_with::<Pos>().len(), 4);
        assert_eq!(w.collect_with::<Pos>().len(), 4);
        assert_eq!(w.count_with::<Health>(), 0);
        assert!(w.collect_with::<Health>().is_empty());
    }

    #[test]
    fn clear_removes_entities_but_keeps_types() {
        let mut w = World::new();
        for i in 0..3 {
            let e = w.spawn();
            w.insert(e, Pos(i));
        }
        w.clear();
        assert!(w.is_empty());
        assert_eq!(w.count_with::<Pos>(), 0);
        assert!(
            w.storage::<Pos>().is_some(),
            "the storage type stays registered"
        );
        assert_eq!(w.stats().component_type_count, 1);
    }

    #[test]
    fn reset_removes_everything_including_types() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        w.insert_resource(Health(5));
        w.reset();
        assert!(w.is_empty());
        assert!(w.storage::<Pos>().is_none());
        assert!(!w.has_resource::<Health>());
    }

    #[test]
    fn zero_sized_components_work() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Marker);
        assert!(w.has::<Marker>(e));
        let mut n = 0;
        w.for_each::<Marker, _>(|_e, _m| n += 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn stats_are_consistent() {
        let mut w = World::new();
        for i in 0..10 {
            let e = w.spawn();
            w.insert(e, Pos(i));
            w.insert(e, Vel(i));
        }
        let s = w.stats();
        assert_eq!(s.entity_count, 10);
        assert_eq!(s.component_type_count, 2);
        assert_eq!(s.component_count, 20);
        assert!(s.memory_bytes > 0);
        assert!(s.tick >= 1);
    }

    #[test]
    fn retain_alive_components_is_a_noop_when_consistent() {
        let mut w = World::new();
        for i in 0..5 {
            let e = w.spawn();
            w.insert(e, Pos(i));
        }
        w.retain_alive_components();
        assert_eq!(w.count_with::<Pos>(), 5);
    }

    #[test]
    fn entities_iterates_live_ids_only() {
        let mut w = World::new();
        let ids: Vec<Entity> = (0..5).map(|_| w.spawn()).collect();
        w.despawn(ids[2]);
        let live: Vec<Entity> = w.entities().collect();
        assert_eq!(live.len(), 4);
        assert!(!live.contains(&ids[2]));
    }

    #[test]
    fn many_entities_and_components_stay_consistent() {
        let mut w = World::new();
        let ids: Vec<Entity> = (0..2000).map(|_| w.spawn()).collect();
        for (i, e) in ids.iter().enumerate() {
            w.insert(*e, Pos(i as i32));
            if i % 3 == 0 {
                w.insert(*e, Vel(i as i32));
            }
        }
        assert_eq!(w.count_with::<Pos>(), 2000);
        assert_eq!(w.count_with::<Vel>(), 667);
        // Despawn every second entity and confirm the counts track.
        for (i, e) in ids.iter().enumerate() {
            if i % 2 == 0 {
                w.despawn(*e);
            }
        }
        assert_eq!(w.len(), 1000);
        assert_eq!(w.count_with::<Pos>(), 1000);
        w.retain_alive_components();
        assert_eq!(w.count_with::<Pos>(), 1000);
    }
}
