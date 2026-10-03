//! The prefabs: small, reusable slices of town.
//!
//! Every prefab is built by a recipe — a foundation, a storey of walls with a
//! door punched through it, a roof — rather than by listing voxels by hand, so
//! the shapes stay consistent and a change to `Builder` improves all twelve at
//! once.
//!
//! Two structural properties are non-negotiable and are enforced by
//! [`validate`], which every prefab is tested against:
//!
//! * the voxel list is a *set*: every voxel is inside `size` and no two share a
//!   position, so expanding a prefab into the world cannot silently overwrite;
//! * a building has a door on its perimeter, a clear cell behind that door and
//!   a walkable floor across its interior, so it can actually be entered.

use std::f32::consts::{FRAC_PI_2, PI};

use noxel_asset::format::{Prefab, PrefabProp, PrefabSpawn, PrefabVoxel};
use noxel_asset::json::JsonValue;

use crate::error::{Error, Result};
use crate::tilesets;

/// Facing `-Z`, the engine's forward axis: a door on the near wall.
pub const NORTH: f32 = 0.0;
/// Facing `+Z`.
pub const SOUTH: f32 = PI;
/// Facing `+X`.
pub const EAST: f32 = -FRAC_PI_2;
/// Facing `-X`.
pub const WEST: f32 = FRAC_PI_2;

/// The prefabs, in emission order. Order is part of the file layout: the
/// manifest and `list` both follow it.
pub const NAMES: [&str; 12] = [
    "house_small",
    "house_large",
    "shop",
    "inn",
    "barn",
    "well",
    "fence_segment",
    "tree_pine",
    "tree_oak",
    "rock_cluster",
    "lamp",
    "market_stall",
];

/// Every prefab, in [`NAMES`] order.
pub fn all() -> Result<Vec<Prefab>> {
    NAMES.iter().map(|name| build(name)).collect()
}

/// Builds one prefab by name.
pub fn build(name: &str) -> Result<Prefab> {
    let prefab = match name {
        "house_small" => house_small()?,
        "house_large" => house_large()?,
        "shop" => shop()?,
        "inn" => inn()?,
        "barn" => barn()?,
        "well" => well()?,
        "fence_segment" => fence_segment()?,
        "tree_pine" => tree_pine()?,
        "tree_oak" => tree_oak()?,
        "rock_cluster" => rock_cluster()?,
        "lamp" => lamp()?,
        "market_stall" => market_stall()?,
        other => {
            return Err(Error::invalid(
                "prefab",
                format!("unknown prefab `{other}`"),
            ));
        }
    };
    validate(&prefab).map_err(|message| Error::invalid(format!("prefab `{name}`"), message))?;
    Ok(prefab)
}

// --- the recipes -----------------------------------------------------------

/// A cottage: 5x4 tiles of floor, one wall storey, a pitched roof.
fn house_small() -> Result<Prefab> {
    let mut b = Builder::new("house_small", [5, 3, 4], &["building", "residential"]);
    b.foundation("wall_stone", "floor_wood");
    b.ring(1, "wall_plaster");
    b.door(2, 0, 1, "floor_wood");
    b.set(0, 1, 1, "wall_window");
    b.set(4, 1, 2, "wall_window");
    b.slab(2, "roof_red");
    b.occluder([0, 1, 0, 5, 2, 4]);
    b.prop("sign", [3.6, 0.0, 0.5], WEST, 1.0);
    b.spawn("door", [2.5, 0.0, -0.5], NORTH);
    b.spawn("resident", [2.5, 0.0, 2.0], SOUTH);
    b.build()
}

/// A two-storey town house with an upper floor and a chimney.
fn house_large() -> Result<Prefab> {
    let mut b = Builder::new("house_large", [7, 4, 5], &["building", "residential"]);
    b.foundation("wall_stone", "floor_wood");
    b.ring(1, "wall_plaster");
    b.door(3, 0, 1, "floor_wood");
    b.set(0, 1, 2, "wall_window");
    b.set(6, 1, 2, "wall_window");
    b.ring(2, "wall_plaster");
    b.interior(2, "floor_wood");
    b.set(0, 2, 2, "wall_window");
    b.set(6, 2, 2, "wall_window");
    b.set(3, 2, 4, "wall_window");
    b.slab(3, "roof_red");
    b.set(5, 3, 0, "chimney");
    b.occluder([0, 1, 0, 7, 3, 5]);
    b.prop("sign", [3.6, 0.0, 0.5], WEST, 1.0);
    b.prop("barrel", [0.5, 0.0, 4.4], NORTH, 1.0);
    b.prop("crate", [6.5, 0.0, 4.4], NORTH, 1.0);
    b.prop("lamp_post", [4.5, 0.0, 0.5], NORTH, 1.0);
    b.spawn("door", [3.5, 0.0, -0.5], NORTH);
    b.spawn("resident", [3.5, 0.0, 2.5], SOUTH);
    b.spawn("guest", [1.5, 0.0, 2.5], SOUTH);
    b.build()
}

/// A shop: open floor, a counter across the back, a sign over the door.
fn shop() -> Result<Prefab> {
    let mut b = Builder::new("shop", [6, 3, 5], &["building", "commercial"]);
    b.foundation("wall_stone", "floor_stone");
    b.ring(1, "wall_plaster");
    b.door(2, 0, 1, "floor_stone");
    b.set(0, 1, 2, "wall_window");
    b.set(5, 1, 2, "wall_window");
    for x in 1..5 {
        b.set(x, 1, 3, "counter");
    }
    b.slab(2, "roof_red");
    b.occluder([0, 1, 0, 6, 2, 5]);
    b.prop("sign", [2.5, 0.0, 0.4], NORTH, 1.0);
    b.prop("crate", [5.4, 0.0, 4.4], NORTH, 1.0);
    b.prop("barrel", [0.4, 0.0, 4.4], NORTH, 1.0);
    b.spawn("door", [2.5, 0.0, -0.5], NORTH);
    b.spawn("shopkeeper", [2.5, 0.0, 2.5], SOUTH);
    b.spawn("customer", [1.5, 0.0, 4.2], NORTH);
    b.build()
}

/// An inn: a slate roof, a taproom counter and a guard post by the door.
fn inn() -> Result<Prefab> {
    let mut b = Builder::new("inn", [8, 4, 6], &["building", "commercial", "residential"]);
    b.foundation("wall_stone", "floor_stone");
    b.ring(1, "wall_wood");
    b.door(3, 0, 1, "floor_stone");
    b.set(0, 1, 3, "wall_window");
    b.set(7, 1, 3, "wall_window");
    for x in 1..3 {
        b.set(x, 1, 4, "counter");
    }
    b.ring(2, "wall_wood");
    b.interior(2, "floor_wood");
    b.set(0, 2, 3, "wall_window");
    b.set(7, 2, 3, "wall_window");
    b.slab(3, "roof_slate");
    b.set(6, 3, 1, "chimney");
    b.occluder([0, 1, 0, 8, 3, 6]);
    b.prop("sign", [4.5, 0.0, 0.5], NORTH, 1.0);
    b.prop("barrel", [0.5, 0.0, 5.4], NORTH, 1.0);
    b.prop("barrel", [1.5, 0.0, 5.4], NORTH, 1.0);
    b.prop("crate", [7.4, 0.0, 5.4], NORTH, 1.0);
    b.prop("lamp_post", [2.5, 0.0, 0.5], NORTH, 1.0);
    b.spawn("door", [3.5, 0.0, -0.5], NORTH);
    b.spawn("innkeeper", [2.5, 0.0, 3.5], SOUTH);
    b.spawn("guest", [5.5, 0.0, 3.5], SOUTH);
    b.spawn("guard", [2.5, 0.0, 0.6], NORTH);
    b.build()
}

/// A barn: a double door wide enough for a cart, crate storage inside.
fn barn() -> Result<Prefab> {
    let mut b = Builder::new("barn", [7, 4, 5], &["building", "farm"]);
    b.foundation("wall_stone", "floor_stone");
    b.ring(1, "wall_wood");
    b.door(2, 0, 1, "floor_stone");
    b.set(3, 0, 0, "floor_stone");
    b.set(3, 1, 0, "wall_door");
    b.set(0, 1, 2, "wall_window");
    b.ring(2, "wall_wood");
    b.interior(2, "floor_wood");
    b.slab(3, "roof_slate");
    b.occluder([0, 1, 0, 7, 3, 5]);
    b.prop("crate", [1.5, 0.0, 3.5], NORTH, 1.0);
    b.prop("crate", [2.5, 0.0, 3.5], NORTH, 1.0);
    b.prop("crate", [1.5, 0.0, 4.4], NORTH, 1.0);
    b.prop("barrel", [5.5, 0.0, 4.4], NORTH, 1.0);
    b.spawn("door", [3.0, 0.0, -0.5], NORTH);
    b.spawn("animal", [3.0, 0.0, 3.5], SOUTH);
    b.build()
}

/// A stone well: a ring of blocks around a shaft of deep water.
fn well() -> Result<Prefab> {
    let mut b = Builder::new("well", [3, 1, 3], &["prop", "water"]);
    b.ring(0, "wall_stone");
    b.set(1, 0, 1, "water_deep");
    b.prop("well", [1.5, 0.0, 1.5], NORTH, 1.0);
    b.spawn("water", [1.5, 0.0, 1.5], NORTH);
    b.build()
}

/// A four-tile run of fencing, on its own strip of grass.
fn fence_segment() -> Result<Prefab> {
    let mut b = Builder::new("fence_segment", [4, 1, 1], &["prop", "fence"]);
    b.ground(0, "grass");
    for x in 0..4 {
        b.prop("fence_h", [x as f32 + 0.5, 0.0, 0.5], NORTH, 1.0);
    }
    b.spawn("anchor", [0.5, 0.0, 0.5], NORTH);
    b.build()
}

/// A pine: a short trunk under a single canopy frame.
fn tree_pine() -> Result<Prefab> {
    let mut b = Builder::new("tree_pine", [1, 1, 1], &["prop", "tree"]);
    b.ground(0, "grass");
    b.prop("tree_trunk", [0.5, 0.0, 0.5], NORTH, 1.0);
    b.prop("tree_canopy_0", [0.5, 1.0, 0.5], NORTH, 1.0);
    b.spawn("chop", [0.5, 0.0, 0.5], NORTH);
    b.build()
}

/// An oak: a taller trunk and a wider, swaying canopy.
fn tree_oak() -> Result<Prefab> {
    let mut b = Builder::new("tree_oak", [1, 1, 1], &["prop", "tree"]);
    b.ground(0, "grass");
    b.prop("tree_trunk", [0.5, 0.0, 0.5], NORTH, 1.25);
    b.prop("tree_canopy_2", [0.5, 1.25, 0.5], NORTH, 1.5);
    b.spawn("chop", [0.5, 0.0, 0.5], NORTH);
    b.build()
}

/// A cluster of boulders on a patch of grass.
fn rock_cluster() -> Result<Prefab> {
    let mut b = Builder::new("rock_cluster", [3, 1, 3], &["prop", "rock"]);
    b.ground(0, "grass");
    b.prop("rock_large", [1.0, 0.0, 1.0], NORTH, 1.0);
    b.prop("rock_small", [0.4, 0.0, 2.3], EAST, 1.0);
    b.prop("rock_small", [2.4, 0.0, 0.5], WEST, 0.8);
    b.spawn("harvest", [1.5, 0.0, 1.5], NORTH);
    b.build()
}

/// A single street lamp on a paved square.
fn lamp() -> Result<Prefab> {
    let mut b = Builder::new("lamp", [1, 1, 1], &["prop", "street"]);
    b.ground(0, "plaza");
    b.prop("lamp_post", [0.5, 0.0, 0.5], NORTH, 1.0);
    b.spawn("light", [0.5, 0.0, 0.5], NORTH);
    b.build()
}

/// A market stall: a counter, two posts and the trader's stock.
fn market_stall() -> Result<Prefab> {
    let mut b = Builder::new("market_stall", [4, 2, 3], &["prop", "commercial"]);
    b.ground(0, "plaza");
    for x in 0..4 {
        b.set(x, 1, 0, "counter");
    }
    b.set(0, 1, 2, "wall_wood");
    b.set(3, 1, 2, "wall_wood");
    b.prop("sign", [1.5, 1.5, -0.3], NORTH, 1.0);
    b.prop("crate", [0.5, 0.0, 2.5], NORTH, 1.0);
    b.prop("barrel", [3.4, 0.0, 2.4], NORTH, 1.0);
    b.spawn("vendor", [2.0, 0.0, 1.5], NORTH);
    b.spawn("customer", [2.0, 0.0, -0.5], SOUTH);
    b.build()
}

// --- the builder -----------------------------------------------------------

/// Assembles a prefab, resolving tile names to ids and keeping the voxel list
/// sorted and duplicate free.
struct Builder {
    name: &'static str,
    size: [u8; 3],
    tags: Vec<&'static str>,
    voxels: Vec<(u8, u8, u8, &'static str)>,
    props: Vec<PrefabProp>,
    spawns: Vec<PrefabSpawn>,
    occluders: Vec<[u8; 6]>,
}

impl Builder {
    fn new(name: &'static str, size: [u8; 3], tags: &[&'static str]) -> Self {
        Self {
            name,
            size,
            tags: tags.to_vec(),
            voxels: Vec::new(),
            props: Vec::new(),
            spawns: Vec::new(),
            occluders: Vec::new(),
        }
    }

    /// Places one voxel, replacing whatever was there before.
    fn set(&mut self, x: u8, y: u8, z: u8, tile: &'static str) {
        self.voxels
            .retain(|voxel| (voxel.0, voxel.1, voxel.2) != (x, y, z));
        self.voxels.push((x, y, z, tile));
    }

    /// Fills the whole footprint at one height: a ceiling or a roof.
    fn slab(&mut self, y: u8, tile: &'static str) {
        let (sx, sz) = (self.size[0], self.size[2]);
        for z in 0..sz {
            for x in 0..sx {
                self.set(x, y, z, tile);
            }
        }
    }

    /// Fills the whole footprint at one height: an open ground plane.
    fn ground(&mut self, y: u8, tile: &'static str) {
        self.slab(y, tile);
    }

    /// Fills the perimeter at one height: a wall.
    fn ring(&mut self, y: u8, tile: &'static str) {
        let (sx, sz) = (self.size[0], self.size[2]);
        for x in 0..sx {
            self.set(x, y, 0, tile);
            self.set(x, y, sz.saturating_sub(1), tile);
        }
        for z in 0..sz {
            self.set(0, y, z, tile);
            self.set(sx.saturating_sub(1), y, z, tile);
        }
    }

    /// Fills the cells strictly inside the perimeter at one height.
    fn interior(&mut self, y: u8, tile: &'static str) {
        let (sx, sz) = (self.size[0], self.size[2]);
        for x in 1..sx.saturating_sub(1) {
            for z in 1..sz.saturating_sub(1) {
                self.set(x, y, z, tile);
            }
        }
    }

    /// The ground floor: a stone footing around a wooden floor.
    fn foundation(&mut self, wall: &'static str, floor: &'static str) {
        self.ring(0, wall);
        self.interior(0, floor);
    }

    /// Puts a door in a wall, and replaces the footing beneath it with a
    /// walkable threshold so the doorway is not blocked by its own foundation.
    fn door(&mut self, x: u8, z: u8, y: u8, threshold: &'static str) {
        self.set(x, 0, z, threshold);
        self.set(x, y, z, "wall_door");
    }

    fn prop(&mut self, kind: &'static str, position: [f32; 3], yaw: f32, scale: f32) {
        self.props.push(PrefabProp {
            kind: kind.to_string(),
            position: noxel_core::math::Vec3::new(position[0], position[1], position[2]),
            yaw,
            scale,
        });
    }

    fn spawn(&mut self, name: &'static str, position: [f32; 3], yaw: f32) {
        self.spawns.push(PrefabSpawn {
            name: name.to_string(),
            position: noxel_core::math::Vec3::new(position[0], position[1], position[2]),
            yaw,
        });
    }

    fn occluder(&mut self, bounds: [u8; 6]) {
        self.occluders.push(bounds);
    }

    fn build(mut self) -> Result<Prefab> {
        let (sx, sy, sz) = (self.size[0], self.size[1], self.size[2]);
        let mut voxels = Vec::with_capacity(self.voxels.len());
        for (x, y, z, name) in self.voxels.drain(..) {
            if x >= sx || y >= sy || z >= sz {
                return Err(Error::invalid(
                    format!("prefab `{}`", self.name),
                    format!("voxel ({x}, {y}, {z}) is outside size {sx}x{sy}x{sz}"),
                ));
            }
            let Some(tile) = tilesets::find(name) else {
                return Err(Error::invalid(
                    format!("prefab `{}`", self.name),
                    format!("unknown tile `{name}`"),
                ));
            };
            voxels.push(PrefabVoxel {
                x,
                y,
                z,
                tile: tile.id,
            });
        }
        // Deterministic order: by height, then depth, then across. The JSON is
        // written in this order, so it reads like a stack of floor plans.
        voxels.sort_by_key(|voxel| (voxel.y, voxel.z, voxel.x));

        Ok(Prefab {
            name: self.name.to_string(),
            size: self.size,
            tags: self.tags.iter().map(|tag| (*tag).to_string()).collect(),
            voxels,
            props: self.props,
            spawns: self.spawns,
            occluders: self.occluders,
        })
    }
}

// --- validation ------------------------------------------------------------

/// Checks the structural rules every prefab must satisfy.
///
/// Returns a human-readable reason on failure. This is the same function the
/// tests call, so a prefab cannot be added without meeting the contract.
pub fn validate(prefab: &Prefab) -> std::result::Result<(), String> {
    let (sx, sy, sz) = (prefab.size[0], prefab.size[1], prefab.size[2]);
    if sx == 0 || sy == 0 || sz == 0 {
        return Err(format!(
            "size {:?} must be non-zero on every axis",
            prefab.size
        ));
    }

    // 1. Voxels: in bounds, unique, known.
    let mut seen: Vec<(u8, u8, u8)> = Vec::with_capacity(prefab.voxels.len());
    for voxel in &prefab.voxels {
        if voxel.x >= sx || voxel.y >= sy || voxel.z >= sz {
            return Err(format!(
                "voxel ({}, {}, {}) is outside size {sx}x{sy}x{sz}",
                voxel.x, voxel.y, voxel.z
            ));
        }
        if seen.contains(&(voxel.x, voxel.y, voxel.z)) {
            return Err(format!(
                "two voxels share ({}, {}, {})",
                voxel.x, voxel.y, voxel.z
            ));
        }
        seen.push((voxel.x, voxel.y, voxel.z));
        if tilesets::find_id(voxel.tile).is_none() {
            return Err(format!(
                "voxel ({}, {}, {}) uses unknown tile id {}",
                voxel.x, voxel.y, voxel.z, voxel.tile
            ));
        }
    }

    // 2. Markers sit on or just outside the prefab, never far away.
    for prop in &prefab.props {
        check_marker(&prop.position, prefab.size, "prop", &prop.kind)?;
    }
    let mut spawn_names: Vec<&str> = Vec::with_capacity(prefab.spawns.len());
    for spawn in &prefab.spawns {
        check_marker(&spawn.position, prefab.size, "spawn", &spawn.name)?;
        if spawn_names.contains(&spawn.name.as_str()) {
            return Err(format!("duplicate spawn `{}`", spawn.name));
        }
        spawn_names.push(&spawn.name);
    }

    // 3. Occluder boxes are inside the prefab and non-empty.
    for bounds in &prefab.occluders {
        let [x0, y0, z0, x1, y1, z1] = *bounds;
        if x1 > sx || y1 > sy || z1 > sz || x0 >= x1 || y0 >= y1 || z0 >= z1 {
            return Err(format!(
                "occluder {bounds:?} is not a box inside size {sx}x{sy}x{sz}"
            ));
        }
    }

    if prefab.has_tag("building") {
        validate_building(prefab)?;
    }
    Ok(())
}

/// Checks a prop or spawn position: inside the footprint, or within one tile of
/// it (a door marker sits just outside the wall it belongs to).
fn check_marker(
    position: &noxel_core::math::Vec3,
    size: [u8; 3],
    what: &str,
    name: &str,
) -> std::result::Result<(), String> {
    let limits = [
        (position.x, size[0] as f32),
        (position.y, size[1] as f32),
        (position.z, size[2] as f32),
    ];
    for (value, limit) in limits {
        if !value.is_finite() || value < -1.0 || value > limit + 1.0 {
            return Err(format!(
                "{what} `{name}` at {:?} is outside the prefab and its one-tile margin",
                position
            ));
        }
    }
    Ok(())
}

/// The extra rules a prefab tagged `building` must satisfy.
fn validate_building(prefab: &Prefab) -> std::result::Result<(), String> {
    let (sx, sy, sz) = (prefab.size[0], prefab.size[1], prefab.size[2]);

    let door = prefab
        .voxels
        .iter()
        .find(|voxel| tilesets::name_of(voxel.tile) == Some("wall_door"))
        .ok_or_else(|| "building has no `wall_door` voxel".to_string())?;
    if door.y == 0 {
        return Err("the door is in the foundation, not in a wall".to_string());
    }
    let on_perimeter = door.x == 0 || door.x == sx - 1 || door.z == 0 || door.z == sz - 1;
    if !on_perimeter {
        return Err(format!(
            "the door at ({}, {}, {}) is not on the perimeter",
            door.x, door.y, door.z
        ));
    }

    // The cell behind the door must be clear, and there must be floor under
    // both the doorway and that cell.
    let inside = if door.x == 0 {
        (1, door.z)
    } else if door.x == sx - 1 {
        (sx - 2, door.z)
    } else if door.z == 0 {
        (door.x, 1)
    } else {
        (door.x, sz - 2)
    };
    if prefab.voxel(inside.0, door.y, inside.1).is_some() {
        return Err(format!(
            "the cell ({}, {}, {}) behind the door is blocked",
            inside.0, door.y, inside.1
        ));
    }
    for (x, z, what) in [
        (door.x, door.z, "the doorway"),
        (inside.0, inside.1, "the door cell"),
    ] {
        let Some(tile) = prefab.voxel(x, 0, z) else {
            return Err(format!("there is no floor under {what} at ({x}, 0, {z})"));
        };
        if !tilesets::flags_of(tile).is_some_and(|flags| flags.walkable) {
            return Err(format!(
                "the floor under {what} at ({x}, 0, {z}) is `{}`, which is not walkable",
                tilesets::name_of(tile).unwrap_or("?")
            ));
        }
    }

    // Every interior cell has a walkable floor.
    for x in 1..sx.saturating_sub(1) {
        for z in 1..sz.saturating_sub(1) {
            let Some(tile) = prefab.voxel(x, 0, z) else {
                return Err(format!("the interior cell ({x}, 0, {z}) has no floor"));
            };
            if !tilesets::flags_of(tile).is_some_and(|flags| flags.walkable) {
                return Err(format!(
                    "the interior floor at ({x}, 0, {z}) is `{}`, which is not walkable",
                    tilesets::name_of(tile).unwrap_or("?")
                ));
            }
        }
    }

    // The shell is closed: every perimeter cell above the foundation holds a
    // wall, a window or the door.
    for y in 1..sy {
        for x in 0..sx {
            for z in [0, sz - 1] {
                if prefab.voxel(x, y, z).is_none() {
                    return Err(format!("the wall at ({x}, {y}, {z}) is missing"));
                }
            }
        }
        for z in 0..sz {
            for x in [0, sx - 1] {
                if prefab.voxel(x, y, z).is_none() {
                    return Err(format!("the wall at ({x}, {y}, {z}) is missing"));
                }
            }
        }
    }

    // A marker stands at the door, so the demo can find the entrance.
    let doorway =
        noxel_core::math::Vec3::new(door.x as f32 + 0.5, door.y as f32, door.z as f32 + 0.5);
    let has_door_spawn = prefab
        .spawns
        .iter()
        .any(|spawn| spawn.name == "door" && (spawn.position - doorway).length() <= 1.6);
    if !has_door_spawn {
        return Err("building has no `door` spawn at its doorway".to_string());
    }
    Ok(())
}

/// Writes a prefab as JSON, adding the tile **name** next to each voxel's id.
///
/// `Prefab::from_json` ignores the extra key, so the file still round-trips as
/// a `Prefab`; a reader gets the readable form.
#[must_use]
pub fn to_json(prefab: &Prefab) -> JsonValue {
    let voxels = prefab
        .voxels
        .iter()
        .map(|voxel| {
            JsonValue::object([
                ("x", JsonValue::from(u32::from(voxel.x))),
                ("y", JsonValue::from(u32::from(voxel.y))),
                ("z", JsonValue::from(u32::from(voxel.z))),
                ("tile", JsonValue::from(voxel.tile)),
                (
                    "name",
                    JsonValue::from(tilesets::name_of(voxel.tile).unwrap_or("unknown")),
                ),
            ])
        })
        .collect::<Vec<_>>();

    let props = prefab
        .props
        .iter()
        .map(|prop| {
            JsonValue::object([
                ("kind", JsonValue::from(prop.kind.as_str())),
                (
                    "position",
                    JsonValue::array([
                        JsonValue::from(prop.position.x),
                        JsonValue::from(prop.position.y),
                        JsonValue::from(prop.position.z),
                    ]),
                ),
                ("yaw", JsonValue::from(prop.yaw)),
                ("scale", JsonValue::from(prop.scale)),
            ])
        })
        .collect::<Vec<_>>();

    let spawns = prefab
        .spawns
        .iter()
        .map(|spawn| {
            JsonValue::object([
                ("name", JsonValue::from(spawn.name.as_str())),
                (
                    "position",
                    JsonValue::array([
                        JsonValue::from(spawn.position.x),
                        JsonValue::from(spawn.position.y),
                        JsonValue::from(spawn.position.z),
                    ]),
                ),
                ("yaw", JsonValue::from(spawn.yaw)),
            ])
        })
        .collect::<Vec<_>>();

    let occluders = prefab
        .occluders
        .iter()
        .map(|bounds| {
            JsonValue::array(
                bounds
                    .iter()
                    .map(|value| JsonValue::from(u32::from(*value)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();

    JsonValue::object([
        ("name", JsonValue::from(prefab.name.as_str())),
        (
            "size",
            JsonValue::array(
                prefab
                    .size
                    .iter()
                    .map(|value| JsonValue::from(u32::from(*value)))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "tags",
            JsonValue::Array(
                prefab
                    .tags
                    .iter()
                    .map(|tag| JsonValue::from(tag.as_str()))
                    .collect(),
            ),
        ),
        ("voxels", JsonValue::Array(voxels)),
        ("props", JsonValue::Array(props)),
        ("spawns", JsonValue::Array(spawns)),
        ("occluders", JsonValue::Array(occluders)),
    ])
}
