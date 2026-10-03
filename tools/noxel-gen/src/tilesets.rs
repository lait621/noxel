//! The tile sets: gameplay flags and texture regions for both atlases.
//!
//! Terrain and building tiles share one **global id space** — terrain is
//! `0..16`, buildings `16..28` — so a prefab voxel's numeric `tile` field is
//! unambiguous no matter which atlas the tile is drawn from. The `uv` region is
//! always derived from the tile's position in its own atlas, which is why ids
//! and columns are allowed to differ.

use noxel_asset::format::{TileDef, TileFlags, TileSet};

use crate::buildings;
use crate::terrain;

/// The id of the first terrain tile.
pub const TERRAIN_BASE_ID: u32 = 0;
/// The id of the first building tile.
pub const BUILDINGS_BASE_ID: u32 = terrain::NAMES.len() as u32;

/// Where a tile lives and what it is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRef {
    /// Tile name, e.g. `"wall_door"`.
    pub name: &'static str,
    /// Globally unique id.
    pub id: u32,
    /// Owning tile set: `"terrain"` or `"buildings"`.
    pub set: &'static str,
}

/// Every tile the generator defines, terrain first.
#[must_use]
pub fn all() -> Vec<TileRef> {
    let mut tiles = Vec::with_capacity(terrain::NAMES.len() + buildings::NAMES.len());
    for (index, name) in terrain::NAMES.iter().enumerate() {
        tiles.push(TileRef {
            name,
            id: TERRAIN_BASE_ID + index as u32,
            set: "terrain",
        });
    }
    for (index, name) in buildings::NAMES.iter().enumerate() {
        tiles.push(TileRef {
            name,
            id: BUILDINGS_BASE_ID + index as u32,
            set: "buildings",
        });
    }
    tiles
}

/// Looks a tile up by name.
#[must_use]
pub fn find(name: &str) -> Option<TileRef> {
    all().into_iter().find(|tile| tile.name == name)
}

/// Looks a tile up by id.
#[must_use]
pub fn find_id(id: u32) -> Option<TileRef> {
    all().into_iter().find(|tile| tile.id == id)
}

/// The flags of a tile id, or `None` when the id is unknown.
#[must_use]
pub fn flags_of(id: u32) -> Option<TileFlags> {
    terrain_tiles()
        .into_iter()
        .chain(building_tiles())
        .find(|tile| tile.id == id)
        .map(|tile| tile.flags)
}

/// The name of a tile id, or `None`.
#[must_use]
pub fn name_of(id: u32) -> Option<&'static str> {
    find_id(id).map(|tile| tile.name)
}

/// The terrain tile set, written to `tilesets/terrain.json`.
#[must_use]
pub fn terrain_set() -> TileSet {
    TileSet {
        name: "terrain".to_string(),
        texture: "textures/terrain.png".to_string(),
        tile_size: terrain::SIZE,
        tiles: terrain_tiles(),
    }
}

/// The building tile set, written to `tilesets/buildings.json`.
#[must_use]
pub fn buildings_set() -> TileSet {
    TileSet {
        name: "buildings".to_string(),
        texture: "textures/buildings.png".to_string(),
        tile_size: buildings::SIZE,
        tiles: building_tiles(),
    }
}

/// The terrain tile definitions, in atlas order.
#[must_use]
pub fn terrain_tiles() -> Vec<TileDef> {
    terrain::NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            // The uv column comes from the atlas module rather than from this
            // list's position, so the two can never drift apart.
            let column = terrain::index_of(name).unwrap_or(index);
            let mut tile = def(
                TERRAIN_BASE_ID + index as u32,
                name,
                column,
                terrain::SIZE,
                terrain_flags(name),
            );
            match *name {
                "rock" | "rock_dark" => {
                    tile.height = 1.0;
                    tile.layer = 1;
                }
                "cliff" => {
                    tile.height = 2.0;
                    tile.layer = 1;
                }
                "bridge" => {
                    tile.height = 0.25;
                    tile.layer = 1;
                }
                _ => {}
            }
            tile
        })
        .collect()
}

/// The building tile definitions, in atlas order.
#[must_use]
pub fn building_tiles() -> Vec<TileDef> {
    buildings::NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let column = buildings::index_of(name).unwrap_or(index);
            let mut tile = def(
                BUILDINGS_BASE_ID + index as u32,
                name,
                column,
                buildings::SIZE,
                building_flags(name),
            );
            match *name {
                "wall_door" => {
                    tile.height = 2.0;
                    tile.layer = 1;
                }
                "wall_plaster" | "wall_wood" | "wall_stone" | "wall_window" => {
                    tile.height = 2.0;
                    tile.layer = 1;
                }
                "roof_red" | "roof_slate" | "roof_edge" | "chimney" => {
                    tile.height = 1.0;
                    tile.layer = 2;
                }
                "counter" => {
                    tile.height = 0.5;
                    tile.layer = 1;
                }
                _ => {}
            }
            tile
        })
        .collect()
}

/// Builds one definition. `column` is the tile's index in its atlas.
fn def(id: u32, name: &str, column: usize, tile_size: u32, flags: TileFlags) -> TileDef {
    TileDef {
        id,
        name: name.to_string(),
        texture: String::new(),
        uv: [column as u32 * tile_size, 0, tile_size, tile_size],
        flags,
        height: 0.0,
        layer: 0,
    }
}

/// Gameplay flags for a terrain tile.
///
/// Ground is walkable and buildable, rock and cliff block movement *and* sight,
/// water is neither walkable nor buildable, and paving is walkable and marked
/// as a road so the world generator keeps props off it.
#[must_use]
pub fn terrain_flags(name: &str) -> TileFlags {
    match name {
        "rock" | "rock_dark" => TileFlags {
            walkable: false,
            blocks_sight: true,
            occluder: true,
            ..TileFlags::default()
        },
        "cliff" => TileFlags {
            walkable: false,
            blocks_sight: true,
            occluder: true,
            ..TileFlags::default()
        },
        "water_shallow" | "water_deep" => TileFlags {
            walkable: false,
            water: true,
            ..TileFlags::default()
        },
        "road" | "road_edge" | "plaza" | "bridge" => TileFlags {
            walkable: true,
            road: true,
            ..TileFlags::default()
        },
        // grass, dirt, sand and snow: plain buildable ground.
        _ => TileFlags {
            walkable: true,
            buildable: true,
            ..TileFlags::default()
        },
    }
}

/// Gameplay flags for a building tile.
#[must_use]
pub fn building_flags(name: &str) -> TileFlags {
    match name {
        // A door is a hole in the wall: passable and transparent.
        "wall_door" => TileFlags {
            walkable: true,
            ..TileFlags::default()
        },
        // Walls, windows and roofs stop movement and sight, and hide the
        // player when the camera is behind them.
        "wall_plaster" | "wall_wood" | "wall_stone" | "wall_window" | "roof_red" | "roof_slate"
        | "roof_edge" | "chimney" => TileFlags {
            walkable: false,
            blocks_sight: true,
            occluder: true,
            ..TileFlags::default()
        },
        // A counter is waist high: it hides the legs but not the room.
        "counter" => TileFlags {
            walkable: false,
            occluder: true,
            ..TileFlags::default()
        },
        // floor_wood, floor_stone.
        _ => TileFlags {
            walkable: true,
            buildable: true,
            ..TileFlags::default()
        },
    }
}
