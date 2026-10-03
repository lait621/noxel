//! The farm: a tile grid, the mesh built from it, and the rules about what may
//! stand where.
//!
//! # Why the whole farm is one mesh
//!
//! The camera looks straight down an orthographic axis, so every object in the
//! world is a flat quad lying on the ground plane and the depth buffer orders
//! them by a depth offset baked into the vertex. That means the entire farm —
//! ground, crops, props and buildings — is one mesh with one instance, and
//! "draw order" is not a thing the game has to think about.
//!
//! It is rebuilt whenever the map's revision changes, which is a few times a
//! second while the player is hoeing. At 44x34 tiles the rebuild is about six
//! thousand vertices and costs far less than a frame; an incremental update
//! would be more code for a cost nobody can measure.
//!
//! # Why depth, not sort order
//!
//! Two objects whose sprites overlap must be drawn back-to-front: a tree at the
//! top of the screen is behind the player standing below it. With a straight-down
//! camera, world `z` *is* screen depth, so the mesh gives each quad a tiny `y`
//! offset proportional to its `z`. Larger `y` is nearer the camera, so objects
//! lower on screen win the depth test — which is exactly the painter's algorithm,
//! expressed in a way the rasterizer already implements.

use noxel_asset::atlas::Atlas;
use noxel_core::math::{Vec2, Vec3};
use noxel_render::mesh::{Mesh, Vertex};

use crate::config::{CROP_STAGES, Crop, FARM_HEIGHT, FARM_WIDTH, Season, TILE};

/// World units between two adjacent objects' depth offsets.
///
/// One thousandth of a tile: far smaller than the world, far larger than the
/// depth buffer's precision at these distances, so adjacent objects never
/// z-fight and the ordering is exact.
const DEPTH_STEP: f32 = 0.001;

// ---------------------------------------------------------------------------
// Tiles
// ---------------------------------------------------------------------------

/// What the ground of a tile is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ground {
    /// Ordinary grass.
    Grass,
    /// Grass in shadow, for texture.
    GrassDark,
    /// Grass with a flower.
    GrassFlower,
    /// Packed earth, walkable, not plantable until hoed.
    Dirt,
    /// Hoed soil, ready to plant.
    Tilled,
    /// A path.
    Path,
    /// Water: blocks walking.
    Water,
    /// Stone flooring.
    Stone,
    /// Sand at the water's edge.
    Sand,
}

impl Ground {
    /// The region name in the terrain atlas.
    #[must_use]
    pub const fn sprite(self, watered: bool) -> &'static str {
        match self {
            Self::Grass => "grass",
            Self::GrassDark => "grass_dark",
            Self::GrassFlower => "grass_flower",
            Self::Dirt => "dirt",
            // A watered tile is the same furrows, darker. The player has to be
            // able to tell at a glance which tiles they have already done, and
            // a separate sprite is the only honest way to show it.
            Self::Tilled => {
                if watered {
                    "watered"
                } else {
                    "tilled"
                }
            }
            Self::Path => "path",
            Self::Water => "water",
            Self::Stone => "stone",
            Self::Sand => "sand",
        }
    }

    /// Whether the player can walk here.
    #[must_use]
    pub const fn walkable(self) -> bool {
        !matches!(self, Self::Water)
    }

    /// Whether a tool can turn this into soil.
    #[must_use]
    pub const fn is_hoeable(self) -> bool {
        matches!(
            self,
            Self::Grass | Self::GrassDark | Self::GrassFlower | Self::Dirt
        )
    }
}

/// A plant growing in a tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plant {
    /// Which crop.
    pub crop: &'static Crop,
    /// Days of growth accumulated while watered.
    pub days: u32,
    /// Whether it was watered today.
    pub watered: bool,
    /// Whether the season ended while it was still in the ground.
    pub dead: bool,
}

impl Plant {
    /// A freshly planted seed.
    #[must_use]
    pub fn new(crop: &'static Crop) -> Self {
        Self {
            crop,
            days: 0,
            watered: false,
            dead: false,
        }
    }

    /// The growth stage, `0..CROP_STAGES`.
    ///
    /// Derived from `days` rather than stored, so a save file cannot disagree
    /// with itself about how grown a plant is.
    #[must_use]
    pub fn stage(&self) -> u32 {
        if self.dead {
            return 0;
        }
        let growth = self.crop.growth_days.max(1);
        let stage = (self.days as f32 / growth as f32 * (CROP_STAGES - 1) as f32) as u32;
        stage.min(CROP_STAGES - 1)
    }

    /// Whether it is ready to harvest.
    #[must_use]
    pub fn is_ripe(&self) -> bool {
        !self.dead && self.days >= self.crop.growth_days
    }

    /// The region name in the crop atlas.
    #[must_use]
    pub fn sprite(&self) -> String {
        format!("{}_{}", self.crop.key, self.stage())
    }
}

/// One map cell.
#[derive(Clone, Copy, Debug)]
pub struct Tile {
    /// The ground.
    pub ground: Ground,
    /// Whether the soil was watered today.
    pub watered: bool,
    /// What is growing, if anything.
    pub plant: Option<Plant>,
    /// A static object standing here, as an atlas region name.
    pub prop: Option<&'static str>,
    /// Whether the player has walked on it, which compacts dirt into a path.
    pub trodden: bool,
}

impl Tile {
    fn new(ground: Ground) -> Self {
        Self {
            ground,
            watered: false,
            plant: None,
            prop: None,
            trodden: false,
        }
    }

    /// Whether the player can stand here.
    #[must_use]
    pub fn walkable(&self) -> bool {
        // A plant never blocks movement; a prop always does. That asymmetry is
        // deliberate: crops you cannot walk through turn a dense field into a
        // maze, and the player's own farm should never trap them.
        self.ground.walkable() && self.prop.is_none()
    }
}

// ---------------------------------------------------------------------------
// The map
// ---------------------------------------------------------------------------

/// Where a placed object is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Bottom-centre of the sprite sits on the tile's centre.
    BottomCenter,
    /// The sprite exactly covers its tile (or tile block).
    Tile,
}

/// A static object drawn on the map.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// Atlas region.
    pub sprite: &'static str,
    /// Position in tiles; the anchor decides which point of the tile.
    pub tile: (u32, u32),
    /// The anchor.
    pub anchor: Anchor,
}

/// The farm.
#[derive(Clone, Debug)]
pub struct FarmMap {
    width: u32,
    height: u32,
    tiles: Vec<Tile>,
    /// Bumped by every mutation, so the renderer knows when to rebuild.
    revision: u64,
    /// Objects that are not a single tile: buildings, trees, the shipping bin.
    placements: Vec<Placement>,
}

impl FarmMap {
    /// The map's width in tiles.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The map's height in tiles.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// A counter that changes whenever the map does.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The objects placed on the map.
    #[must_use]
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y * self.width + x) as usize)
    }

    /// A tile, or `None` outside the map.
    #[must_use]
    pub fn get(&self, x: i32, y: i32) -> Option<&Tile> {
        if x < 0 || y < 0 {
            return None;
        }
        self.index(x as u32, y as u32).map(|i| &self.tiles[i])
    }

    /// A tile to mutate.
    pub fn get_mut(&mut self, x: i32, y: i32) -> Option<&mut Tile> {
        if x < 0 || y < 0 {
            return None;
        }
        let index = self.index(x as u32, y as u32)?;
        self.revision += 1;
        self.tiles.get_mut(index)
    }

    /// Marks the map changed without touching a tile.
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// Whether a tile blocks movement. Off-map is not walkable.
    #[must_use]
    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        self.get(x, y).is_some_and(Tile::walkable)
    }

    /// Clears yesterday's watering and grows every plant that was watered.
    ///
    /// Returns how many plants died of old age at the season's end. The order
    /// matters and is the heart of the farming loop:
    ///
    /// 1. A plant that was watered yesterday advances one day.
    /// 2. Rain waters everything *before* this runs, so a rainy day counts.
    /// 3. Watering is then cleared, so today starts dry.
    pub fn advance_day(&mut self, season: Season) -> DayReport {
        let mut report = DayReport::default();
        for tile in &mut self.tiles {
            tile.watered = false;

            let Some(plant) = tile.plant.as_mut() else {
                continue;
            };
            let was_watered = plant.watered;
            plant.watered = false;

            if plant.dead {
                continue;
            }
            // A crop out of its season dies rather than merely stopping: a
            // pumpkin left in the ground through winter should be a loss the
            // player sees, not a plant that quietly waits.
            if plant.crop.season != season {
                plant.dead = true;
                report.died += 1;
                continue;
            }
            if was_watered {
                plant.days += 1;
                if plant.days == plant.crop.growth_days {
                    report.ripened += 1;
                }
            }
        }
        self.revision += 1;
        report
    }

    /// Waters every tilled tile, for a rainy day.
    ///
    /// Returns how many tiles were watered. Rain reaching unplanted soil matters
    /// as much as rain reaching a crop: the player can plant on a wet day
    /// without spending the energy.
    pub fn water_all(&mut self) -> u32 {
        let mut count = 0;
        for tile in &mut self.tiles {
            if tile.ground == Ground::Tilled && !tile.watered {
                tile.watered = true;
                if let Some(plant) = tile.plant.as_mut() {
                    plant.watered = true;
                }
                count += 1;
            }
        }
        if count > 0 {
            self.revision += 1;
        }
        count
    }

    /// The number of tiles with a ripe crop.
    #[must_use]
    pub fn ripe_count(&self) -> u32 {
        self.tiles
            .iter()
            .filter(|t| t.plant.is_some_and(|p| p.is_ripe()))
            .count() as u32
    }

    /// A short description, for the log and the day summary.
    #[must_use]
    pub fn summary(&self) -> String {
        let planted = self.tiles.iter().filter(|t| t.plant.is_some()).count();
        let tilled = self
            .tiles
            .iter()
            .filter(|t| t.ground == Ground::Tilled)
            .count();
        format!(
            "{tilled} tilled, {planted} planted, {} ripe",
            self.ripe_count()
        )
    }
}

/// What happened overnight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DayReport {
    /// Plants that became ready to harvest.
    pub ripened: u32,
    /// Plants that died because their season ended.
    pub died: u32,
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A tiny deterministic hash, so the farm scatters identically on every machine
/// and in every run.
///
/// The engine's rule (`docs/adr/0006-deterministic-generation.md`) is that world
/// content is a pure function of an address, never of a shared RNG. This is that
/// function in four lines.
fn hash2(x: u32, y: u32, salt: u32) -> u32 {
    let mut h = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(salt.wrapping_mul(0xC2B2_AE35));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    h
}

/// A value in `0..100` from an address.
fn chance(x: u32, y: u32, salt: u32) -> u32 {
    hash2(x, y, salt) % 100
}

/// Builds the starting farm.
///
/// Hand-placed zones rather than pure noise: a farm the player cannot read at a
/// glance is a bad first five minutes, and "where is the shop" should be
/// answered by looking, not by walking.
#[must_use]
pub fn build_farm() -> FarmMap {
    let mut tiles = Vec::with_capacity((FARM_WIDTH * FARM_HEIGHT) as usize);
    for y in 0..FARM_HEIGHT {
        for x in 0..FARM_WIDTH {
            tiles.push(Tile::new(grass_at(x, y)));
        }
    }

    let mut map = FarmMap {
        width: FARM_WIDTH,
        height: FARM_HEIGHT,
        tiles,
        revision: 1,
        placements: Vec::new(),
    };

    carve_zones(&mut map);
    place_objects(&mut map);
    scatter_nature(&mut map);
    map
}

/// The base ground for a tile, before zones are carved.
fn grass_at(x: u32, y: u32) -> Ground {
    match chance(x, y, 7) {
        0..=13 => Ground::GrassDark,
        14..=19 => Ground::GrassFlower,
        _ => Ground::Grass,
    }
}

/// Paints the farm's regions.
fn carve_zones(map: &mut FarmMap) {
    // A stone plaza in front of the house, and the paths that run from it.
    for y in 6..12 {
        for x in 3..30 {
            set_ground(map, x, y, Ground::Path);
        }
    }
    // The main path down the middle of the farm to the field.
    for y in 11..FARM_HEIGHT - 2 {
        for x in 15..18 {
            set_ground(map, x, y, Ground::Path);
        }
    }
    // Around the pond, at the bottom left.
    for y in FARM_HEIGHT - 12..FARM_HEIGHT - 4 {
        for x in 2..12 {
            let dx = x as f32 - 7.0;
            let dy = y as f32 - (FARM_HEIGHT as f32 - 8.0);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < 3.4 {
                set_ground(map, x, y, Ground::Water);
            } else if distance < 4.4 {
                set_ground(map, x, y, Ground::Sand);
            }
        }
    }
    // The field: the plot the player will actually farm.
    for y in 14..FARM_HEIGHT - 5 {
        for x in 21..FARM_WIDTH - 3 {
            set_ground(map, x, y, Ground::Dirt);
        }
    }
    // The map's edge is a treeline, which is also what stops the player walking
    // off it.
    for x in 0..FARM_WIDTH {
        set_ground(map, x, 0, Ground::GrassDark);
        set_ground(map, x, FARM_HEIGHT - 1, Ground::GrassDark);
    }
    for y in 0..FARM_HEIGHT {
        set_ground(map, 0, y, Ground::GrassDark);
        set_ground(map, FARM_WIDTH - 1, y, Ground::GrassDark);
    }
}

fn set_ground(map: &mut FarmMap, x: u32, y: u32, ground: Ground) {
    if let Some(index) = map.index(x, y) {
        map.tiles[index].ground = ground;
    }
}

/// Places the buildings and the fixtures the farm starts with.
fn place_objects(map: &mut FarmMap) {
    // The farmhouse, top-left, with the player's bed on its doorstep.
    map.placements.push(Placement {
        sprite: "farmhouse",
        tile: (6, 3),
        anchor: Anchor::Tile,
    });
    map.placements.push(Placement {
        sprite: "shop",
        tile: (26, 3),
        anchor: Anchor::Tile,
    });
    map.placements.push(Placement {
        sprite: "shipping_bin",
        tile: (14, 9),
        anchor: Anchor::BottomCenter,
    });
    map.placements.push(Placement {
        sprite: "well",
        tile: (4, 12),
        anchor: Anchor::BottomCenter,
    });
    map.placements.push(Placement {
        sprite: "scarecrow",
        tile: (30, 16),
        anchor: Anchor::BottomCenter,
    });
    map.placements.push(Placement {
        sprite: "sign",
        tile: (13, 9),
        anchor: Anchor::BottomCenter,
    });

    // The buildings block movement: mark the tiles they stand on.
    for y in 3..7 {
        for x in 6..10 {
            block(map, x, y);
        }
    }
    for y in 3..6 {
        for x in 26..30 {
            block(map, x, y);
        }
    }
}

fn block(map: &mut FarmMap, x: u32, y: u32) {
    if let Some(index) = map.index(x, y) {
        map.tiles[index].prop = Some("__building");
    }
}

/// Scatters trees, rocks, weeds and flowers.
fn scatter_nature(map: &mut FarmMap) {
    for y in 1..FARM_HEIGHT - 1 {
        for x in 1..FARM_WIDTH - 1 {
            let tile = &map.tiles[(y * FARM_WIDTH + x) as usize];
            if tile.ground != Ground::Grass && tile.ground != Ground::GrassDark {
                continue;
            }
            if tile.prop.is_some() {
                continue;
            }
            let roll = chance(x, y, 31);
            let sprite = match roll {
                0..=3 => Some("tree_oak"),
                4..=5 => Some("tree_pine"),
                6..=9 => Some("bush"),
                10..=13 => Some("weed"),
                14..=16 => Some("rock_small"),
                17 => Some("rock_large"),
                18..=20 => Some("flower_red"),
                21..=23 => Some("flower_blue"),
                24..=26 => Some("mushroom"),
                27 => Some("log"),
                _ => None,
            };
            if let Some(sprite) = sprite {
                let index = (y * FARM_WIDTH + x) as usize;
                map.tiles[index].prop = Some(sprite);
            }
        }
    }
    map.touch();
}

// ---------------------------------------------------------------------------
// Mesh
// ---------------------------------------------------------------------------

/// The three meshes the farm is drawn as.
///
/// Three rather than one because a material carries exactly one texture, and the
/// ground, the crops and the props come from three different atlases. Splitting
/// them also means the ground material can carry the season's tint while the
/// crops keep their true colours — a pumpkin should not turn orange-er in autumn
/// because the grass did.
pub struct FarmMeshes {
    /// Ground tiles, drawn with the season's tint on the material.
    pub ground: Mesh,
    /// Growing plants.
    pub crops: Mesh,
    /// Trees, rocks, flowers, buildings.
    pub props: Mesh,
}

/// Builds every mesh for the map.
///
/// Every quad is a flat sprite on the ground plane. Ground tiles sit at `y = 0`
/// and objects are raised by a depth offset derived from their `z`, which is what
/// makes the depth buffer sort them back to front.
#[must_use]
pub fn build_meshes(map: &FarmMap, terrain: &Atlas, crops: &Atlas, props: &Atlas) -> FarmMeshes {
    let mut ground = Mesh::new("farm.ground", Vec::new(), Vec::new());
    let mut crop_mesh = Mesh::new("farm.crops", Vec::new(), Vec::new());
    let mut prop_mesh = Mesh::new("farm.props", Vec::new(), Vec::new());

    for y in 0..map.height {
        for x in 0..map.width {
            let tile = &map.tiles[(y * map.width + x) as usize];

            if let Some(sprite) = terrain.region(tile.ground.sprite(tile.watered)) {
                push_quad(
                    &mut ground,
                    &sprite.uv_min,
                    &sprite.uv_max,
                    x as f32,
                    y as f32,
                    1.0,
                    1.0,
                    0.0,
                );
            }
            if let Some(plant) = tile.plant {
                let name = plant.sprite();
                if let Some(sprite) = crops.region(&name) {
                    push_quad(
                        &mut crop_mesh,
                        &sprite.uv_min,
                        &sprite.uv_max,
                        x as f32,
                        y as f32,
                        1.0,
                        1.0,
                        depth_of(y as f32 + 1.0),
                    );
                }
            }
            let Some(name) = tile.prop else { continue };
            // The sentinel a building uses to block movement has no sprite of
            // its own; the building is drawn from `placements` instead.
            if name == "__building" {
                continue;
            }
            let Some(sprite) = props.region(name) else {
                continue;
            };
            push_sprite(
                &mut prop_mesh,
                &sprite.uv_min,
                &sprite.uv_max,
                x as f32 + 0.5,
                y as f32 + 1.0,
                sprite.width() / TILE as f32,
                sprite.height() / TILE as f32,
                depth_of(y as f32 + 1.0),
            );
        }
    }

    for placement in &map.placements {
        let Some(sprite) = props.region(placement.sprite) else {
            continue;
        };
        let width = sprite.width() / TILE as f32;
        let height = sprite.height() / TILE as f32;
        let (tx, ty) = (placement.tile.0 as f32, placement.tile.1 as f32);
        let (anchor_x, anchor_z) = match placement.anchor {
            Anchor::BottomCenter => (tx + 0.5, ty + 1.0),
            // A building's sprite is its footprint seen from above, so its
            // bottom edge is the bottom of the tile block it occupies.
            Anchor::Tile => (tx + width * 0.5, ty + height),
        };
        push_sprite(
            &mut prop_mesh,
            &sprite.uv_min,
            &sprite.uv_max,
            anchor_x,
            anchor_z,
            width,
            height,
            depth_of(anchor_z),
        );
    }

    ground.recompute_bounds();
    crop_mesh.recompute_bounds();
    prop_mesh.recompute_bounds();
    FarmMeshes {
        ground,
        crops: crop_mesh,
        props: prop_mesh,
    }
}

/// Builds a mesh for one character frame, anchored at its feet.
///
/// The quad runs from `(-width/2, 0)` to `(width/2, -height)` in the ground
/// plane, so translating it to a world position puts the sprite's feet there and
/// grows it up the screen. One mesh per animation frame, swapped when the frame
/// changes — four vertices, so rebuilding is cheaper than the bookkeeping an
/// atlas-swap would need.
#[must_use]
pub fn build_character_mesh(atlas: &Atlas, region: &str) -> Mesh {
    let mut mesh = Mesh::new("character", Vec::new(), Vec::new());
    let Some(sprite) = atlas.region(region) else {
        return mesh;
    };
    push_sprite(
        &mut mesh,
        &sprite.uv_min,
        &sprite.uv_max,
        0.0,
        0.0,
        sprite.width() / TILE as f32,
        sprite.height() / TILE as f32,
        0.0,
    );
    mesh.recompute_bounds();
    mesh
}

/// The depth offset for an object rooted at world `z`.
///
/// Monotonic in `z`, which is the only property that matters: the depth buffer
/// turns it back into "the object lower on screen wins".
#[must_use]
pub fn depth_of(z: f32) -> f32 {
    DEPTH_STEP * (z + 1.0)
}

/// Appends a sprite quad anchored at its **bottom centre**.
///
/// `(anchor_x, anchor_z)` is the point the sprite stands on; the quad grows up
/// the screen from there, which for a straight-down camera means towards smaller
/// `z`.
#[allow(clippy::too_many_arguments)]
fn push_sprite(
    mesh: &mut Mesh,
    uv_min: &Vec2,
    uv_max: &Vec2,
    anchor_x: f32,
    anchor_z: f32,
    width: f32,
    height: f32,
    depth: f32,
) {
    push_quad(
        mesh,
        uv_min,
        uv_max,
        anchor_x - width * 0.5,
        anchor_z - height,
        width,
        height,
        depth,
    );
}

/// Appends one axis-aligned quad covering `width` x `height` tiles from `(x, z)`.
#[allow(clippy::too_many_arguments)]
fn push_quad(
    mesh: &mut Mesh,
    uv_min: &Vec2,
    uv_max: &Vec2,
    x: f32,
    z: f32,
    width: f32,
    height: f32,
    depth: f32,
) {
    let base = mesh.vertex_count() as u32;
    let (x0, x1) = (x, x + width);
    let (z0, z1) = (z, z + height);
    let uv = |u: f32, v: f32| {
        Vec2::new(
            uv_min.x + (uv_max.x - uv_min.x) * u,
            uv_min.y + (uv_max.y - uv_min.y) * v,
        )
    };
    // Wound so the surface normal points **up**, towards the camera. Going
    // (x0,z0) -> (x1,z0) -> (x1,z1) instead gives a normal of -Y, and the
    // rasterizer's backface cull then drops every tile in the world — which
    // looks exactly like "the scene is empty" and is invisible in a test that
    // only checks the mesh has vertices in it.
    for (px, pz, u, v) in [
        (x0, z0, 0.0, 0.0),
        (x0, z1, 0.0, 1.0),
        (x1, z1, 1.0, 1.0),
        (x1, z0, 1.0, 0.0),
    ] {
        mesh.vertices
            .push(Vertex::new(Vec3::new(px, depth, pz), Vec3::Y, uv(u, v)));
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// What the player can do where they are standing.
///
/// A function of the tile rather than of the player, so it has no opinion about
/// who is asking and a test can check the shop's doorstep without building a
/// player.
///
/// The rectangles are the buildings' own footprints from [`place_objects`],
/// widened by one tile on the side the player approaches from. A prompt you have
/// to stand *on* the building to see is a prompt nobody finds.
#[must_use]
pub fn prompt_for(tile: (i32, i32)) -> Option<&'static str> {
    let (x, y) = tile;
    // The shop occupies (26, 3)..(30, 6); its door faces the plaza below it.
    if (25..=30).contains(&x) && (6..=9).contains(&y) {
        return Some("进入种子商店");
    }
    // The shipping bin stands at (14, 9).
    if (12..=16).contains(&x) && (9..=11).contains(&y) {
        return Some("打开出货箱");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::crop_by_key;

    #[test]
    fn the_farm_is_the_configured_size() {
        let map = build_farm();
        assert_eq!(map.width(), FARM_WIDTH);
        assert_eq!(map.height(), FARM_HEIGHT);
        assert_eq!(map.tiles.len(), (FARM_WIDTH * FARM_HEIGHT) as usize);
    }

    #[test]
    fn generation_is_deterministic() {
        // The engine's rule: world content is a pure function of its address.
        // Two builds must be identical or a save file means nothing.
        let a = build_farm();
        let b = build_farm();
        assert_eq!(a.revision(), b.revision());
        for y in 0..FARM_HEIGHT as i32 {
            for x in 0..FARM_WIDTH as i32 {
                assert_eq!(
                    a.get(x, y).unwrap().ground,
                    b.get(x, y).unwrap().ground,
                    "at {x},{y}"
                );
                assert_eq!(
                    a.get(x, y).unwrap().prop,
                    b.get(x, y).unwrap().prop,
                    "at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn the_edges_of_the_map_are_not_walkable() {
        // Otherwise the player walks into the void and the camera has nothing to
        // clamp to.
        let map = build_farm();
        assert!(!map.is_walkable(-1, 5));
        assert!(!map.is_walkable(5, -1));
        assert!(!map.is_walkable(FARM_WIDTH as i32, 5));
        assert!(!map.is_walkable(5, FARM_HEIGHT as i32));
    }

    #[test]
    fn there_is_a_way_into_the_field_that_is_not_blocked() {
        // A farm whose field is walled off by scenery is unplayable, and the
        // scatter is dense enough that this is a real risk.
        let map = build_farm();
        let walkable = (14..FARM_HEIGHT - 5)
            .flat_map(|y| (21..FARM_WIDTH - 3).map(move |x| (x, y)))
            .filter(|(x, y)| map.is_walkable(*x as i32, *y as i32))
            .count();
        let total = ((FARM_HEIGHT - 5 - 14) * (FARM_WIDTH - 3 - 21)) as usize;
        assert!(
            walkable * 2 > total,
            "only {walkable} of {total} field tiles are reachable-looking"
        );
    }

    #[test]
    fn the_pond_is_water_and_the_shore_is_sand() {
        let map = build_farm();
        let water = map
            .tiles
            .iter()
            .filter(|t| t.ground == Ground::Water)
            .count();
        let sand = map
            .tiles
            .iter()
            .filter(|t| t.ground == Ground::Sand)
            .count();
        assert!(
            water > 20,
            "the pond is too small to be worth drawing: {water}"
        );
        assert!(sand > 5, "the pond has no shore: {sand}");
    }

    #[test]
    fn the_island_case_i_the_centre_of_the_pond_is_water() {
        let map = build_farm();
        let centre = (7i32, (FARM_HEIGHT - 8) as i32);
        assert_eq!(map.get(centre.0, centre.1).unwrap().ground, Ground::Water);
        assert!(!map.is_walkable(centre.0, centre.1));
    }

    #[test]
    fn a_planted_crop_advances_one_day_per_watering() {
        let mut map = build_farm();
        let tile = map.get_mut(30, 20).unwrap();
        tile.ground = Ground::Tilled;
        tile.plant = Some(Plant::new(crop_by_key("parsnip").unwrap()));
        let revision = map.revision();

        // Watered yesterday: grows.
        for _ in 0..3 {
            map.get_mut(30, 20).unwrap().plant.as_mut().unwrap().watered = true;
            map.advance_day(Season::Spring);
        }
        assert_eq!(map.get(30, 20).unwrap().plant.unwrap().days, 3);
        assert!(
            map.revision() > revision,
            "advancing a day must dirty the mesh"
        );
    }

    #[test]
    fn an_unwatered_crop_does_not_grow() {
        // The whole farming loop depends on this: if crops grew regardless, the
        // watering can would be decoration.
        let mut map = build_farm();
        map.get_mut(30, 20).unwrap().ground = Ground::Tilled;
        map.get_mut(30, 20).unwrap().plant = Some(Plant::new(crop_by_key("parsnip").unwrap()));
        map.advance_day(Season::Spring);
        map.advance_day(Season::Spring);
        assert_eq!(map.get(30, 20).unwrap().plant.unwrap().days, 0);
    }

    #[test]
    fn rain_waters_every_tilled_tile_once() {
        let mut map = build_farm();
        for x in 24..28 {
            map.get_mut(x, 20).unwrap().ground = Ground::Tilled;
        }
        let first = map.water_all();
        assert!(first >= 4);
        assert_eq!(map.water_all(), 0, "watering twice in a day is a no-op");
    }

    #[test]
    fn a_crop_left_in_the_ground_when_its_season_ends_dies() {
        // A pumpkin through winter is a visible loss, not a plant that quietly
        // waits for next autumn.
        let mut map = build_farm();
        map.get_mut(30, 20).unwrap().ground = Ground::Tilled;
        map.get_mut(30, 20).unwrap().plant = Some(Plant::new(crop_by_key("pumpkin").unwrap()));
        map.get_mut(30, 20).unwrap().plant.as_mut().unwrap().watered = true;

        let report = map.advance_day(Season::Winter);
        assert_eq!(report.died, 1);
        assert!(map.get(30, 20).unwrap().plant.unwrap().dead);
    }

    #[test]
    fn growth_stages_span_the_whole_sprite_range() {
        let crop = crop_by_key("parsnip").unwrap();
        let mut plant = Plant::new(crop);
        assert_eq!(plant.stage(), 0, "a seed is stage 0");
        plant.days = crop.growth_days;
        assert_eq!(
            plant.stage(),
            CROP_STAGES - 1,
            "a ripe crop is the last stage"
        );
        assert!(plant.is_ripe());
        // A stage never leaves the atlas, even if `days` overshoots.
        plant.days = 9999;
        assert_eq!(plant.stage(), CROP_STAGES - 1);
    }

    #[test]
    fn a_dead_plant_is_not_ripe() {
        let crop = crop_by_key("parsnip").unwrap();
        let mut plant = Plant::new(crop);
        plant.days = crop.growth_days;
        plant.dead = true;
        assert!(!plant.is_ripe());
        assert_eq!(
            plant.stage(),
            0,
            "a dead plant falls back to the sprout sprite"
        );
    }

    #[test]
    fn depth_increases_with_z_so_lower_objects_win() {
        // The single property the depth-buffer sort depends on.
        assert!(depth_of(10.0) > depth_of(9.0));
        assert!(
            depth_of(0.0) > 0.0,
            "an object at z=0 must still be in front of the ground"
        );
    }

    #[test]
    fn a_sprite_grows_up_the_screen_from_its_anchor() {
        let mut mesh = Mesh::new("t", Vec::new(), Vec::new());
        push_sprite(
            &mut mesh,
            &Vec2::ZERO,
            &Vec2::new(1.0, 1.0),
            5.0,
            8.0,
            1.0,
            2.0,
            0.5,
        );
        // The bottom edge is the anchor, and the sprite extends towards smaller
        // z, which is up the screen.
        let highest = mesh
            .vertices
            .iter()
            .map(|v| v.position.z)
            .fold(f32::MAX, f32::min);
        let lowest = mesh
            .vertices
            .iter()
            .map(|v| v.position.z)
            .fold(f32::MIN, f32::max);
        assert_eq!(lowest, 8.0);
        assert_eq!(highest, 6.0);
        // Centred horizontally on the anchor.
        let left = mesh
            .vertices
            .iter()
            .map(|v| v.position.x)
            .fold(f32::MAX, f32::min);
        let right = mesh
            .vertices
            .iter()
            .map(|v| v.position.x)
            .fold(f32::MIN, f32::max);
        assert_eq!(left, 4.5);
        assert_eq!(right, 5.5);
    }

    #[test]
    fn a_quad_faces_upwards_so_the_camera_can_see_it() {
        // The camera looks straight down, and the rasterizer culls backfaces. A
        // quad wound the other way is invisible, and "invisible" here means the
        // whole world, with no error anywhere to say why.
        let mut mesh = Mesh::new("q", Vec::new(), Vec::new());
        push_quad(
            &mut mesh,
            &Vec2::ZERO,
            &Vec2::new(1.0, 1.0),
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
        );
        for index in 0..mesh.triangle_count() {
            let normal = mesh.triangle_normal(index);
            assert!(
                normal.y > 0.5,
                "triangle {index} faces {normal:?}, so the camera is looking at its back"
            );
        }
    }

    #[test]
    fn a_quad_winds_so_both_triangles_cover_it() {
        let mut mesh = Mesh::new("q", Vec::new(), Vec::new());
        push_quad(
            &mut mesh,
            &Vec2::ZERO,
            &Vec2::new(1.0, 1.0),
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
        );
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.triangle_count(), 2);
    }
}
