//! Biomes: the fixed table that maps a terrain sample to ground cover, water
//! and vegetation.
//!
//! The table is deliberately *closed*: [`BiomeId::ALL`] has eight entries, the
//! ids are stable across versions, and [`BiomeTable`] can be handed out by
//! reference without allocating. A biome is pure data — it names tiles in the
//! tile set, states how dense its trees and rocks are and records the elevation
//! it typically occupies — so the generator, the renderer and the debug overlay
//! all agree on what "forest" means.
//!
//! ```
//! use noxel_world::biome::{BiomeId, BiomeTable};
//!
//! let table = BiomeTable::new();
//! assert_eq!(table.get(BiomeId::FOREST).name, "forest");
//! assert_eq!(BiomeId::ALL.len(), 8);
//! ```
//!
//! Tile names in the table are *requests*, not guarantees: a tile set that does
//! not contain `"marsh"` still generates, because the generator resolves every
//! name through a documented fallback chain (see [`crate::gen::WorldGenerator`]).

/// Identifies one of the eight biomes.
///
/// The numeric value is stable and is what should be stored in save files:
/// [`BiomeId::PLAINS`] is always `0`, [`BiomeId::LAKE`] is always `7`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BiomeId(pub u8);

impl BiomeId {
    /// Open grassland: the default, most common biome.
    pub const PLAINS: Self = Self(0);
    /// Dense woodland.
    pub const FOREST: Self = Self(1);
    /// Hot and dry: sand, bare rock.
    pub const DESERT: Self = Self(2);
    /// Cold: snow and ice.
    pub const TUNDRA: Self = Self(3);
    /// Wet lowland: marsh grass and reeds.
    pub const SWAMP: Self = Self(4);
    /// Rolling uplands between plains and mountains.
    pub const HILLS: Self = Self(5);
    /// High, steep and rocky.
    pub const MOUNTAINS: Self = Self(6);
    /// Anywhere below sea level: sea, lake or inland sea.
    pub const LAKE: Self = Self(7);

    /// Every biome, in ascending id order.
    pub const ALL: [Self; 8] = [
        Self::PLAINS,
        Self::FOREST,
        Self::DESERT,
        Self::TUNDRA,
        Self::SWAMP,
        Self::HILLS,
        Self::MOUNTAINS,
        Self::LAKE,
    ];

    /// The biome's lowercase name, or `"unknown"` for an id outside `0..8`.
    ///
    /// This never panics; an out-of-range id coming out of a corrupt save file
    /// degrades to the name rather than aborting the load.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::PLAINS => "plains",
            Self::FOREST => "forest",
            Self::DESERT => "desert",
            Self::TUNDRA => "tundra",
            Self::SWAMP => "swamp",
            Self::HILLS => "hills",
            Self::MOUNTAINS => "mountains",
            Self::LAKE => "lake",
            _ => "unknown",
        }
    }

    /// The id as an array index, clamped into `0..8`.
    ///
    /// Useful for per-biome side tables indexed by `[T; 8]`.
    #[must_use]
    pub const fn index(self) -> usize {
        if self.0 as usize >= Self::ALL.len() {
            0
        } else {
            self.0 as usize
        }
    }

    /// True when this id is one of [`BiomeId::ALL`].
    #[must_use]
    pub const fn is_known(self) -> bool {
        (self.0 as usize) < Self::ALL.len()
    }

    /// True for biomes the generator only produces below sea level.
    #[must_use]
    pub const fn is_water(self) -> bool {
        matches!(self, Self::LAKE)
    }
}

/// One biome's data.
///
/// Every field is public so a game can ship a patched table (a snowy desert for
/// a themed level, say) without touching the generator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biome {
    /// Which biome this is.
    pub id: BiomeId,
    /// Lowercase display name.
    pub name: &'static str,
    /// Tile used for the bulk of the biome's ground.
    pub base_tile: &'static str,
    /// Tile used sparsely, where the accent noise crosses its threshold, so the
    /// ground does not read as one flat colour.
    pub accent_tile: &'static str,
    /// Tile for standing water in this biome, when it has any.
    pub water_tile: Option<&'static str>,
    /// Tree density multiplier; `0.0` means "no trees here".
    pub tree_density: f32,
    /// Rock density multiplier; `0.0` means "no rocks here".
    pub rock_density: f32,
    /// The elevation above sea level, in metres, that characterises the biome.
    ///
    /// Metadata for tools and overlays (it is what a biome map prints next to
    /// the legend); it is deliberately *not* fed back into the height field,
    /// because a height field that depends on a biome that depends on height
    /// has no continuous solution.
    pub height_bias: f32,
}

/// The fixed biome lookup table.
///
/// Constructing one is free — the data is a `const` array — so the usual thing
/// is to build it once next to the world generator and borrow it everywhere.
#[derive(Clone, Copy, Debug)]
pub struct BiomeTable {
    biomes: [Biome; BiomeId::ALL.len()],
}

/// The canonical biome data. Kept as a free `const` so [`BiomeTable::new`] is
/// `const`-evaluable and the table can also be used without a `BiomeTable`.
pub(crate) const BIOMES: [Biome; BiomeId::ALL.len()] = [
    Biome {
        id: BiomeId::PLAINS,
        name: "plains",
        base_tile: "grass",
        accent_tile: "grass_tuft",
        water_tile: None,
        tree_density: 0.02,
        rock_density: 0.004,
        height_bias: 1.5,
    },
    Biome {
        id: BiomeId::FOREST,
        name: "forest",
        base_tile: "grass_dark",
        accent_tile: "moss",
        water_tile: None,
        tree_density: 0.14,
        rock_density: 0.006,
        height_bias: 2.5,
    },
    Biome {
        id: BiomeId::DESERT,
        name: "desert",
        base_tile: "sand",
        accent_tile: "sand_rock",
        water_tile: None,
        tree_density: 0.002,
        rock_density: 0.02,
        height_bias: 1.0,
    },
    Biome {
        id: BiomeId::TUNDRA,
        name: "tundra",
        base_tile: "snow",
        accent_tile: "ice",
        water_tile: Some("ice"),
        tree_density: 0.012,
        rock_density: 0.03,
        height_bias: 3.0,
    },
    Biome {
        id: BiomeId::SWAMP,
        name: "swamp",
        base_tile: "marsh",
        accent_tile: "reeds",
        water_tile: Some("water"),
        tree_density: 0.07,
        rock_density: 0.002,
        height_bias: 0.4,
    },
    Biome {
        id: BiomeId::HILLS,
        name: "hills",
        base_tile: "grass_rocky",
        accent_tile: "stone",
        water_tile: None,
        tree_density: 0.04,
        rock_density: 0.08,
        height_bias: 6.0,
    },
    Biome {
        id: BiomeId::MOUNTAINS,
        name: "mountains",
        base_tile: "stone",
        accent_tile: "scree",
        water_tile: None,
        tree_density: 0.004,
        rock_density: 0.12,
        height_bias: 14.0,
    },
    Biome {
        id: BiomeId::LAKE,
        name: "lake",
        base_tile: "water",
        accent_tile: "water_deep",
        water_tile: Some("water"),
        tree_density: 0.0,
        rock_density: 0.0,
        height_bias: -1.0,
    },
];

impl BiomeTable {
    /// The fixed table.
    #[must_use]
    pub const fn new() -> Self {
        Self { biomes: BIOMES }
    }

    /// Looks a biome up by id.
    ///
    /// An unknown id — a corrupted save, a mod that invented biome 42 — returns
    /// [`BiomeId::PLAINS`] rather than panicking.
    #[must_use]
    pub fn get(&self, id: BiomeId) -> &Biome {
        &self.biomes[id.index()]
    }

    /// Every biome, in ascending id order.
    #[must_use]
    pub fn all(&self) -> &[Biome] {
        &self.biomes
    }

    /// The water tile for a biome, if it declares one.
    #[must_use]
    pub fn water_tile(&self, id: BiomeId) -> Option<&'static str> {
        self.get(id).water_tile
    }

    /// Tree density for a biome, clamped into `[0, 1]`.
    #[must_use]
    pub fn tree_density(&self, id: BiomeId) -> f32 {
        self.get(id).tree_density.clamp(0.0, 1.0)
    }

    /// Rock density for a biome, clamped into `[0, 1]`.
    #[must_use]
    pub fn rock_density(&self, id: BiomeId) -> f32 {
        self.get(id).rock_density.clamp(0.0, 1.0)
    }
}

impl Default for BiomeTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ids_are_distinct_and_ordered() {
        let mut ids = BiomeId::ALL;
        ids.sort_unstable();
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(id.0 as usize, i, "ids must be dense and ascending");
        }
    }

    #[test]
    fn table_is_indexed_by_id() {
        let table = BiomeTable::new();
        for (i, id) in BiomeId::ALL.iter().enumerate() {
            assert_eq!(table.get(*id).id, *id);
            assert_eq!(table.all()[i].id, *id);
        }
    }

    #[test]
    fn names_are_lowercase_and_match_the_table() {
        let table = BiomeTable::new();
        for id in BiomeId::ALL {
            let name = id.name();
            assert_eq!(name, table.get(id).name);
            assert!(!name.is_empty());
            assert_eq!(name, name.to_lowercase());
            assert_ne!(name, "unknown");
        }
    }

    #[test]
    fn unknown_ids_degrade_to_plains() {
        let table = BiomeTable::new();
        let weird = BiomeId(200);
        assert_eq!(weird.name(), "unknown");
        assert!(!weird.is_known());
        assert_eq!(weird.index(), 0);
        assert_eq!(table.get(weird).id, BiomeId::PLAINS);
    }

    #[test]
    fn every_biome_names_a_base_and_accent_tile() {
        let table = BiomeTable::new();
        for biome in table.all() {
            assert!(!biome.base_tile.is_empty(), "{}", biome.name);
            assert!(!biome.accent_tile.is_empty(), "{}", biome.name);
            assert_ne!(
                biome.base_tile, biome.accent_tile,
                "{} needs a distinct accent",
                biome.name
            );
        }
    }

    #[test]
    fn densities_are_in_range() {
        let table = BiomeTable::new();
        for biome in table.all() {
            assert!((0.0..=1.0).contains(&biome.tree_density), "{}", biome.name);
            assert!((0.0..=1.0).contains(&biome.rock_density), "{}", biome.name);
            assert!(table.tree_density(biome.id) <= 1.0);
            assert!(table.rock_density(biome.id) <= 1.0);
        }
    }

    #[test]
    fn forest_is_treer_than_desert_and_hills_rockier_than_swamp() {
        let table = BiomeTable::new();
        assert!(table.tree_density(BiomeId::FOREST) > table.tree_density(BiomeId::DESERT));
        assert!(table.rock_density(BiomeId::HILLS) > table.rock_density(BiomeId::SWAMP));
        assert_eq!(table.tree_density(BiomeId::LAKE), 0.0);
    }

    #[test]
    fn water_biome_declares_a_water_tile() {
        let table = BiomeTable::new();
        assert!(BiomeId::LAKE.is_water());
        assert_eq!(table.water_tile(BiomeId::LAKE), Some("water"));
        assert!(table.water_tile(BiomeId::PLAINS).is_none());
    }

    #[test]
    fn id_round_trips_through_u8() {
        for id in BiomeId::ALL {
            assert_eq!(BiomeId(id.0), id);
        }
    }

    #[test]
    fn table_default_equals_new() {
        assert_eq!(BiomeTable::default().all(), BiomeTable::new().all());
    }
}
