//! The demo world configuration and its schema documentation.
//!
//! `world/demo.json` is *not* an engine format: it is the hand-authored input
//! the `town-demo` example reads before it asks the world generator for
//! anything. Keeping it as plain JSON with a documented schema means the demo
//! can grow without the asset crate having to learn about world building.
//!
//! The `--seed` given to `noxel-gen` is this file's world seed, and it also
//! drives the small layout jitter applied to the points of interest, so a
//! different seed really does describe a different town.

use noxel_asset::json::JsonValue;
use noxel_core::rng::Pcg32;

use crate::prefabs;
use crate::tilesets;

/// The size of the demo world, in tiles.
pub const WORLD_TILES: u32 = 192;

/// The demo world as JSON.
///
/// Everything here is derived from `seed`: the same seed always produces the
/// same file, and the jitter comes from a [`Pcg32`] rather than from any system
/// source.
#[must_use]
pub fn demo(seed: u64) -> JsonValue {
    let mut rng = Pcg32::new(seed);
    let center = (WORLD_TILES / 2) as f32;

    // Points of interest, each pinned to a town district and then nudged by
    // the seeded generator so no two seeds lay the town out identically.
    let pois: [(&str, &str, &str, f32, f32); 8] = [
        ("The Old Well", "well", "well", -6.0, -4.0),
        ("Rivermouth Market", "market", "market_stall", 5.0, -2.0),
        ("The Gilded Goose", "inn", "inn", -12.0, 6.0),
        ("Blacksmith's Forge", "smithy", "shop", 9.0, 7.0),
        ("Chapel of the Dawn", "chapel", "house_large", 0.0, 16.0),
        ("North Gate", "gate", "fence_segment", -3.0, -20.0),
        ("Miller's Barn", "farm", "barn", 18.0, 11.0),
        ("The Standing Stones", "ruin", "rock_cluster", -19.0, -12.0),
    ];
    let poi_json = pois
        .iter()
        .map(|(name, kind, prefab, dx, dy)| {
            let jitter_x = rng.range_f32(-1.5, 1.5);
            let jitter_z = rng.range_f32(-1.5, 1.5);
            JsonValue::object([
                ("name", JsonValue::from(*name)),
                ("kind", JsonValue::from(*kind)),
                ("prefab", JsonValue::from(*prefab)),
                (
                    "position",
                    JsonValue::array([
                        JsonValue::from(center + dx + jitter_x),
                        JsonValue::from(0.0),
                        JsonValue::from(center + dy + jitter_z),
                    ]),
                ),
            ])
        })
        .collect::<Vec<_>>();

    let biomes = vec![
        biome("meadow", "grass", "grass_flowers", 0.34, 0.30, 0.75),
        biome("woodland", "grass_dark", "dirt", 0.26, 0.55, 1.00),
        biome("riverbank", "sand", "road_edge", 0.16, 0.75, 1.00),
        biome("marsh", "dirt_dark", "water_shallow", 0.14, 0.80, 1.00),
        biome("highland", "rock", "snow", 0.10, 0.00, 0.35),
    ];

    let towns = vec![
        town(
            "Rivermouth",
            center,
            center,
            26.0,
            "plaster",
            &[
                "house_small",
                "house_large",
                "shop",
                "inn",
                "well",
                "market_stall",
                "lamp",
                "fence_segment",
            ],
        ),
        town(
            "Ashford",
            44.0,
            60.0,
            12.0,
            "wood",
            &["house_small", "barn", "well", "fence_segment", "tree_oak"],
        ),
        town(
            "Bram Hollow",
            150.0,
            132.0,
            10.0,
            "stone",
            &["house_small", "barn", "well", "tree_pine"],
        ),
    ];

    let roads = vec![
        JsonValue::object([
            ("name", JsonValue::from("high street")),
            ("tile", JsonValue::from("road")),
            (
                "points",
                JsonValue::Array(vec![
                    point(center - 30.0, center),
                    point(center, center - 2.0),
                    point(center + 30.0, center + 4.0),
                ]),
            ),
        ]),
        JsonValue::object([
            ("name", JsonValue::from("north road")),
            ("tile", JsonValue::from("road")),
            (
                "points",
                JsonValue::Array(vec![
                    point(center - 3.0, 8.0),
                    point(center - 2.0, center - 6.0),
                    point(center + 1.0, center + 40.0),
                ]),
            ),
        ]),
    ];

    JsonValue::object([
        ("name", JsonValue::from("town-demo")),
        ("schema", JsonValue::from(1u32)),
        ("seed", JsonValue::from(seed as f64)),
        ("tile_size", JsonValue::from(1.0f32)),
        (
            "bounds",
            JsonValue::object([
                ("tiles_x", JsonValue::from(WORLD_TILES)),
                ("tiles_z", JsonValue::from(WORLD_TILES)),
            ]),
        ),
        (
            "player_start",
            JsonValue::array([
                JsonValue::from(center),
                JsonValue::from(0.0),
                JsonValue::from(center + 14.0),
            ]),
        ),
        ("biomes", JsonValue::Array(biomes)),
        ("towns", JsonValue::Array(towns)),
        ("roads", JsonValue::Array(roads)),
        ("pois", JsonValue::Array(poi_json)),
    ])
}

/// A `[x, y, z]` point in tiles.
fn point(x: f32, z: f32) -> JsonValue {
    JsonValue::array([JsonValue::from(x), JsonValue::from(0.0), JsonValue::from(z)])
}

/// One biome hint: which ground tile to lay down and where the biome belongs.
fn biome(
    name: &str,
    ground: &str,
    accent: &str,
    weight: f32,
    moisture_min: f32,
    moisture_max: f32,
) -> JsonValue {
    JsonValue::object([
        ("name", JsonValue::from(name)),
        ("ground", JsonValue::from(ground)),
        ("accent", JsonValue::from(accent)),
        ("weight", JsonValue::from(weight)),
        (
            "moisture",
            JsonValue::array([JsonValue::from(moisture_min), JsonValue::from(moisture_max)]),
        ),
    ])
}

/// One town: where it is, how big, and which prefabs it may place.
fn town(name: &str, x: f32, z: f32, radius: f32, style: &str, allowed: &[&str]) -> JsonValue {
    JsonValue::object([
        ("name", JsonValue::from(name)),
        ("position", point(x, z)),
        ("radius", JsonValue::from(radius)),
        ("style", JsonValue::from(style)),
        (
            "prefabs",
            JsonValue::Array(
                allowed
                    .iter()
                    .map(|prefab| JsonValue::from(*prefab))
                    .collect(),
            ),
        ),
    ])
}

/// The schema documentation written to `world/README.md`.
#[must_use]
pub fn readme() -> String {
    let mut text = String::new();
    text.push_str(
        "# World configuration\n\n\
         `demo.json` is the input the `town-demo` example reads before it touches the\n\
         world generator. It is plain JSON with no engine type behind it, so this file\n\
         is the schema: every key below is required unless it says otherwise.\n\n\
         ## Keys\n\n\
         | Key | Type | Meaning |\n\
         |---|---|---|\n\
         | `name` | string | World name, for logs and save files. |\n\
         | `schema` | integer | Schema version. Bump it when a key changes meaning. |\n\
         | `seed` | integer | World seed. Passed to `RngStream`/`Pcg32`; every stream in the world derives from it. |\n\
         | `tile_size` | number | Metres per tile. The engine's convention is 1.0. |\n\
         | `bounds.tiles_x`, `bounds.tiles_z` | integer | Size of the demo world in tiles. |\n\
         | `player_start` | `[x, y, z]` | Where the player spawns, in tile units (`y` is height). |\n\
         | `biomes` | array | Biome hints, most specific last. |\n\
         | `towns` | array | Settlements and the prefabs each may place. |\n\
         | `roads` | array | Polylines the road painter follows. |\n\
         | `pois` | array | Named points of interest shown on the map and in dialogue. |\n\n\
         ### `biomes[]`\n\n\
         | Field | Meaning |\n\
         |---|---|\n\
         | `name` | Biome id, matched by the world generator. |\n\
         | `ground` | Tile **name** used for the biome floor (see `../tilesets/terrain.json`). |\n\
         | `accent` | Tile name scattered through the biome. |\n\
         | `weight` | Relative share of the map; the weights of a file should sum to 1. |\n\
         | `moisture` | `[min, max]` moisture band, `0..1`, that selects this biome. |\n\n\
         ### `towns[]`\n\n\
         | Field | Meaning |\n\
         |---|---|\n\
         | `name` | Settlement name. |\n\
         | `position` | `[x, y, z]` centre in tile units. |\n\
         | `radius` | Build radius in tiles. |\n\
         | `style` | Wall material hint: `plaster`, `wood` or `stone`. |\n\
         | `prefabs` | Prefab **names** this town may place (see `../prefabs/`). |\n\n\
         ### `roads[]`\n\n\
         `name`, the `tile` to paint, and `points`: a polyline of `[x, y, z]` tile\n\
         positions. The painter connects consecutive points with straight runs.\n\n\
         ### `pois[]`\n\n\
         `name` (shown to the player), `kind` (a stable id used by dialogue and the\n\
         map legend), `prefab` (what to place), and `position` in tile units.\n\n\
         ## Units\n\n\
         One tile is one world unit is one metre. Positions are tile centres: the\n\
         tile at column `i` spans `[i, i + 1)`, so `x = 12.5` is the middle of column\n\
         12. Heights (`y`) are metres above the ground plane.\n\n\
         ## Regenerating\n\n\
         This file is written by `noxel-gen`; edit the generator, not the file.\n\n\
         ```sh\n\
         cargo run -p noxel-gen -- generate --force\n\
         ```\n",
    );
    text
}

/// Checks a parsed world configuration against the documented schema.
///
/// This is the round-trip half of generation: `assets` parses what it just
/// wrote and runs this, so a malformed world cannot ship just because the
/// generator was the one that produced it.
pub fn validate(json: &JsonValue) -> std::result::Result<(), String> {
    if !json.is_object() {
        return Err("the world file must be an object".to_string());
    }
    for key in [
        "name",
        "schema",
        "seed",
        "tile_size",
        "bounds",
        "player_start",
    ] {
        if !json.has(key) {
            return Err(format!("missing required key `{key}`"));
        }
    }

    let tiles_x = json
        .get("bounds")
        .and_then(|bounds| bounds.get("tiles_x"))
        .and_then(JsonValue::as_u32)
        .ok_or_else(|| "`bounds.tiles_x` must be a positive integer".to_string())?;
    let tiles_z = json
        .get("bounds")
        .and_then(|bounds| bounds.get("tiles_z"))
        .and_then(JsonValue::as_u32)
        .ok_or_else(|| "`bounds.tiles_z` must be a positive integer".to_string())?;
    if tiles_x == 0 || tiles_z == 0 {
        return Err("`bounds` must be non-zero on both axes".to_string());
    }

    let tiles = known_tiles();
    let prefab_names = known_prefabs();

    let biomes = json
        .get("biomes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "`biomes` must be an array".to_string())?;
    if biomes.is_empty() {
        return Err("`biomes` must not be empty".to_string());
    }
    for (index, biome) in biomes.iter().enumerate() {
        for key in ["name", "ground", "accent", "weight", "moisture"] {
            if !biome.has(key) {
                return Err(format!("`biomes[{index}]` is missing `{key}`"));
            }
        }
        for key in ["ground", "accent"] {
            let tile = biome
                .get_str(key)
                .ok_or_else(|| format!("`biomes[{index}].{key}` must be a string"))?;
            if !tiles.contains(&tile) {
                return Err(format!(
                    "`biomes[{index}].{key}` names unknown tile `{tile}`"
                ));
            }
        }
    }

    let towns = json
        .get("towns")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "`towns` must be an array".to_string())?;
    for (index, town) in towns.iter().enumerate() {
        if !town.has("name") || !town.has("position") || !town.has("radius") {
            return Err(format!(
                "`towns[{index}]` needs `name`, `position` and `radius`"
            ));
        }
        let listed = town
            .get("prefabs")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| format!("`towns[{index}].prefabs` must be an array"))?;
        for prefab in listed {
            let name = prefab
                .as_str()
                .ok_or_else(|| format!("`towns[{index}].prefabs` must hold strings"))?;
            if !prefab_names.contains(&name) {
                return Err(format!("`towns[{index}]` names unknown prefab `{name}`"));
            }
        }
    }

    let pois = json
        .get("pois")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "`pois` must be an array".to_string())?;
    if pois.is_empty() {
        return Err("`pois` must not be empty".to_string());
    }
    let mut names: Vec<&str> = Vec::with_capacity(pois.len());
    for (index, poi) in pois.iter().enumerate() {
        let name = poi
            .get_str("name")
            .ok_or_else(|| format!("`pois[{index}].name` must be a string"))?;
        if names.contains(&name) {
            return Err(format!("two points of interest are called `{name}`"));
        }
        names.push(name);
        let prefab = poi
            .get_str("prefab")
            .ok_or_else(|| format!("`pois[{index}].prefab` must be a string"))?;
        if !prefab_names.contains(&prefab) {
            return Err(format!("`pois[{index}]` names unknown prefab `{prefab}`"));
        }
        let position = poi
            .get("position")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| format!("`pois[{index}].position` must be an array"))?;
        if position.len() != 3 {
            return Err(format!("`pois[{index}].position` must have three numbers"));
        }
        let x = position[0]
            .as_f32()
            .ok_or_else(|| format!("`pois[{index}].position[0]` must be a number"))?;
        let z = position[2]
            .as_f32()
            .ok_or_else(|| format!("`pois[{index}].position[2]` must be a number"))?;
        if x < 0.0 || z < 0.0 || x > tiles_x as f32 || z > tiles_z as f32 {
            return Err(format!(
                "`pois[{index}]` at ({x}, {z}) is outside the {tiles_x}x{tiles_z} world"
            ));
        }
    }

    Ok(())
}

/// The prefab names a town may place, for the tests and the README.
#[must_use]
pub fn known_prefabs() -> Vec<&'static str> {
    prefabs::NAMES.to_vec()
}

/// The tile names the schema may reference.
#[must_use]
pub fn known_tiles() -> Vec<&'static str> {
    tilesets::all().into_iter().map(|tile| tile.name).collect()
}
