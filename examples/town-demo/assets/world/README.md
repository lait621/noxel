# World configuration

`demo.json` is the input the `town-demo` example reads before it touches the
world generator. It is plain JSON with no engine type behind it, so this file
is the schema: every key below is required unless it says otherwise.

## Keys

| Key | Type | Meaning |
|---|---|---|
| `name` | string | World name, for logs and save files. |
| `schema` | integer | Schema version. Bump it when a key changes meaning. |
| `seed` | integer | World seed. Passed to `RngStream`/`Pcg32`; every stream in the world derives from it. |
| `tile_size` | number | Metres per tile. The engine's convention is 1.0. |
| `bounds.tiles_x`, `bounds.tiles_z` | integer | Size of the demo world in tiles. |
| `player_start` | `[x, y, z]` | Where the player spawns, in tile units (`y` is height). |
| `biomes` | array | Biome hints, most specific last. |
| `towns` | array | Settlements and the prefabs each may place. |
| `roads` | array | Polylines the road painter follows. |
| `pois` | array | Named points of interest shown on the map and in dialogue. |

### `biomes[]`

| Field | Meaning |
|---|---|
| `name` | Biome id, matched by the world generator. |
| `ground` | Tile **name** used for the biome floor (see `../tilesets/terrain.json`). |
| `accent` | Tile name scattered through the biome. |
| `weight` | Relative share of the map; the weights of a file should sum to 1. |
| `moisture` | `[min, max]` moisture band, `0..1`, that selects this biome. |

### `towns[]`

| Field | Meaning |
|---|---|
| `name` | Settlement name. |
| `position` | `[x, y, z]` centre in tile units. |
| `radius` | Build radius in tiles. |
| `style` | Wall material hint: `plaster`, `wood` or `stone`. |
| `prefabs` | Prefab **names** this town may place (see `../prefabs/`). |

### `roads[]`

`name`, the `tile` to paint, and `points`: a polyline of `[x, y, z]` tile
positions. The painter connects consecutive points with straight runs.

### `pois[]`

`name` (shown to the player), `kind` (a stable id used by dialogue and the
map legend), `prefab` (what to place), and `position` in tile units.

## Units

One tile is one world unit is one metre. Positions are tile centres: the
tile at column `i` spans `[i, i + 1)`, so `x = 12.5` is the middle of column
12. Heights (`y`) are metres above the ground plane.

## Regenerating

This file is written by `noxel-gen`; edit the generator, not the file.

```sh
cargo run -p noxel-gen -- generate --force
```
