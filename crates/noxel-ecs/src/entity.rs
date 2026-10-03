//! Entities and generational ids.
//!
//! An [`Entity`] is a 64-bit value: a 32-bit slot index plus a 32-bit
//! generation. When an entity is despawned its slot is recycled with a bumped
//! generation, so a stale `Entity` copied out of a system three frames ago
//! resolves to `None` instead of silently addressing a different object.
//!
//! This is the same scheme as [`noxel_core::pool::Handle`], but with a concrete
//! non-generic type so entities can be stored in components, serialised, and
//! compared without a type parameter following them around.

use noxel_core::pool::Handle;

/// A live-or-stale reference to an entity in a [`World`](crate::World).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// A handle that never resolves. Use it as a "none" sentinel in components
    /// such as `Target(Entity)`.
    pub const PLACEHOLDER: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
    };

    /// Builds an entity id from its parts.
    #[inline]
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The slot index.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation this id was issued at.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// True for [`Entity::PLACEHOLDER`].
    #[inline]
    #[must_use]
    pub const fn is_placeholder(self) -> bool {
        self.index == u32::MAX
    }

    /// Packs the id into a `u64` (index in the low 32 bits), for save files and
    /// for compact component payloads.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Rebuilds an id from [`Entity::to_bits`].
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: (bits & 0xFFFF_FFFF) as u32,
            generation: (bits >> 32) as u32,
        }
    }

    /// Converts to a [`Handle`] of any element type.
    ///
    /// [`Handle`] carries no data for `T` beyond a `PhantomData`, so the same
    /// 64 bits address the world's entity slot map directly.
    #[inline]
    #[must_use]
    pub fn to_handle_of<T>(self) -> Handle<T> {
        Handle::from_bits(self.to_bits())
    }

    /// Converts from a [`Handle`] of any element type.
    #[inline]
    #[must_use]
    pub fn from_handle_of<T>(handle: Handle<T>) -> Self {
        Self::from_bits(handle.to_bits())
    }
}

impl core::fmt::Display for Entity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_placeholder() {
            f.write_str("Entity::PLACEHOLDER")
        } else {
            write!(f, "Entity({}v{})", self.index, self.generation)
        }
    }
}

/// The per-entity bookkeeping the world keeps in its slot map.
#[derive(Clone, Debug)]
pub(crate) struct EntityMeta {
    /// Number of components currently attached, used to decide when a slot can
    /// be recycled. Keeping the count here makes `despawn` O(components) rather
    /// than O(storages).
    pub component_count: u32,
    /// Optional human-readable name, for the debug overlay and for
    /// `world.find("player")`.
    pub name: Option<String>,
}

impl EntityMeta {
    pub fn new() -> Self {
        Self {
            component_count: 0,
            name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_roundtrip() {
        let e = Entity::from_parts(42, 7);
        assert_eq!(Entity::from_bits(e.to_bits()), e);
        assert_eq!(e.index(), 42);
        assert_eq!(e.generation(), 7);
    }

    #[test]
    fn handle_roundtrip() {
        let e = Entity::from_parts(3, 9);
        let h: Handle<String> = e.to_handle_of();
        assert_eq!(Entity::from_handle_of(h), e);
    }

    #[test]
    fn placeholder_is_detectable() {
        assert!(Entity::PLACEHOLDER.is_placeholder());
        assert!(!Entity::from_parts(0, 0).is_placeholder());
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(Entity::from_parts(1, 2).to_string(), "Entity(1v2)");
        assert_eq!(Entity::PLACEHOLDER.to_string(), "Entity::PLACEHOLDER");
    }

    #[test]
    fn ordering_is_by_index_then_generation() {
        let a = Entity::from_parts(1, 5);
        let b = Entity::from_parts(2, 0);
        assert!(a < b);
    }
}
