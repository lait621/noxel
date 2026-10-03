# World generation

`noxel-world` turns a seed into terrain, biomes, roads, towns, props and the
collision data the rest of the engine consumes.

**The one rule:** a chunk is a pure function of `(config.seed, chunk_pos)`
(`docs/adr/0006-deterministic-generation.md`). Nothing in the generation path
holds a mutable RNG — each stage draws from its own addressable stream
(`RngStream::for_chunk(seed, label, x, y)`) — so chunk `(900, -400)` is
byte-identical whether it was generated first, last, or on another machine, and
chunks can be generated in parallel and without their neighbours.

## Terrain

`WorldGenerator::sample_height(x, z)` composes three noise fields (constants at
the top of `crates/noxel-world/src/gen.rs`):

| Stage | Noise | Frequency | Contribution |
|---|---|---|---|
| Continent mask | `fbm_2d`, 4 octaves | 0.00085 (~1200 m) | ocean/coast/inland, via `smoothstep(-0.30, 0.35, continent)` |
| Relief | `warped_fbm_2d`, 5 octaves | 0.0035 (~285 m) | bays and peninsulas; scaled by `height_scale` and by how much land there is |
| Mountains | `ridged_2d`, 5 octaves | 0.0018 (~550 m) | ranges, gated by `smoothstep(0.55, 0.92, ridge * (0.55 + 0.45 * land))` |

The land term is a signed offset (`(land - 0.5) * scale * 0.9`), which puts real
ocean *below* the waterline rather than merely low. Two stages then modify the
field: **road grading** (`carve_roads` — flat across the paved width, a two-tile
shoulder, never below `sea_level + 0.15 * tile_size`, so a road crossing a lake
becomes a causeway) and **town flattening** (`flatten_towns` — the inner 55% of a
town's disc pulled flat towards its plaza with `smoothstep(radius * 0.55, radius,
distance)`). The plaza height uses the base field plus the road carve and never
the flattening, so there is no recursion.

`sample_slope(x, z)` is a central difference of `sample_height` over one tile, so
`atan(slope)` is the ground angle. `Chunk::heights`/`slopes` agree with both to
within rounding (`chunk_heights_match_sample_height_at_tile_centres`). Thresholds:
`ROCK_SLOPE` 0.55 (~29°, above which a tile is rock) and `MAX_PROP_SLOPE` 0.84
(~40°, above which no prop is placed).

## Biomes

`biome_for` selects from the fixed eight-entry table in `biome.rs`, in this order:

| Test | Result |
|---|---|
| `height < sea_level` | `LAKE` |
| `height - sea_level > 1.15 * height_scale` | `MOUNTAINS` |
| `temperature < 0.30` | `TUNDRA` |
| `height - sea_level > 0.5 * height_scale` | `HILLS` |
| `moisture > 0.66 && temperature > 0.45` | `SWAMP` |
| `temperature > 0.70 && moisture < 0.42` | `DESERT` |
| `moisture > 0.52` | `FOREST` |
| otherwise | `PLAINS` |

Temperature is low-frequency fBm plus a gradient (`+Z` colder):
`smoothstep(-1, 1, 1.7 * noise - z * 0.00002)`; moisture is
`smoothstep(-1, 1, 1.5 * noise)`. Ids are stable — `PLAINS` is always `0`, `LAKE`
always `7` — so store the id in save files; every `Biome` field is public, so a
game can ship a patched table. `Biome::height_bias` is metadata for tools only:
feeding it back into the height field would make height depend on a biome that
depends on height.

| Biome | Base tile | Accent | Tree density | Rock density |
|---|---|---|---|---|
| `PLAINS` | `grass` | `grass_tuft` | 0.02 | 0.004 |
| `FOREST` | `grass_dark` | `moss` | 0.14 | 0.006 |
| `DESERT` | `sand` | `sand_rock` | 0.002 | 0.02 |
| `TUNDRA` | `snow` | `ice` | 0.012 | 0.03 |
| `SWAMP` | `marsh` | `reeds` | 0.07 | 0.002 |
| `HILLS` | `grass_rocky` | `stone` | 0.04 | 0.08 |
| `MOUNTAINS` | `stone` | `scree` | 0.004 | 0.12 |
| `LAKE` | `water` | `water_deep` | 0.0 | 0.0 |

## Tiles, and the fallback order

`tile_for` picks one tile per tile centre in this priority: water, road, building
floor, rock, beach sand, accent, biome base. A road therefore survives a
shoreline, and a building's interior floor only appears where nothing more
important already is.

Every tile *name* is resolved **once**, in `WorldGenerator::new`, through a
documented fallback chain — which is why an empty tile set still generates a
valid world instead of panicking:

1. the named tile or one of its aliases (`water`, `deep_water`, `lake`, `ocean`,
   `sea` for the water slot);
2. the first tile carrying the matching flag (`water`, `road`, `walkable`,
   `buildable`, `blocks_sight`);
3. one of `grass`, `dirt`, `floor`, `stone`, `sand`, `water`;
4. the tile set's first tile, then tile id `0`.

A completely empty tile set collapses every slot to `0`. The accent tile is
chosen by two `value_2d` scales (0.09 and 0.45), so accents clump into meadows
rather than speckling uniformly.

## The macro road lattice

A road runs along every `road_grid_chunks`-th chunk boundary (8 chunks = 256 m),
offset by a jitter drawn from `RngStream::for_chunk(seed, "road/jitter", index,
axis)`. The lattice is **connected by construction**: every vertical road crosses
every horizontal one, so the network is a single graph with no path-finding at
generation time. `RoadNetwork::generate` materialises the lines a region touches,
extended one cell beyond it so the region is crossed by complete roads.

The same lattice is available in closed form, which is what lets the terrain
carve a road without building the segment list first: `macro_line_x`,
`macro_line_z`, `macro_spacing`, `nearest_line_index` (clamped so a huge
coordinate cannot overflow `i32`) and `road_jitter`.

`RoadSegment` is the shared currency; `clip_to_aabb` hands each chunk only the
piece that crosses it, so neighbours never report the same pavement twice.
`RoadNetwork::MAX_SEGMENTS` (200 000) caps a continent-sized request.

## Towns

A town exists at every `town_spacing_chunks`-th chunk-lattice position (10 by
default), with a built-up radius of `town_radius_chunks` (3). The lattice is a
pure function of position, so the height field, the tile pass and the plan agree
about where the streets are:

```rust
use noxel_world::WorldConfig;
use noxel_world::town::{nearest_town_cell, town_centre};

let config = WorldConfig::new(4);
let cell = nearest_town_cell(&config, 340.0, -12.0);
let centre = town_centre(&config, cell); // y is 0; fill it from the height field
```

`TownStyle` is drawn with fixed weights (hamlet 0.30, village 0.35, town 0.20,
crossroads 0.15) and scales the piece count, streets and plot spacing:

| Style | `building_scale` | `side_streets` | Plot gap (tiles) |
|---|---|---|---|
| `Hamlet` | 0.4 | 0 | 1.60 |
| `Village` | 0.7 | 1 | 0.83 |
| `Town` | 1.0 | 3 | 0.40 |
| `Crossroads` | 0.85 | 1 | 0.61 |

`TownPlan::generate` lays out a main cross through the plaza, `side_streets`
streets per axis (trimmed to the disc by `sqrt(r² - offset²)`), a connector
towards the nearest macro road, and plots along both sides of every street. A
plot is rejected if it leaves the disc, overlaps the plaza (`plaza_radius` is
0.35 chunks), intersects a street's AABB, or comes within a quarter tile of an
existing plot. Plots are over-provisioned by a quarter: `buildings_per_town`
(18) scaled by the style is the number **built**, and the rest are reserved with
`taken == false` for the game (`TownPlan::free_plots()`). `buildings_per_town =
0` still yields eight reserved plots.

`TownPlan::instantiate` turns each taken plot into a `BuildingInstance`: a prefab
when one fits (preferring tags `building`/`house`, then the largest footprint that
fits inside the plot minus a one-tile margin after rotation onto the plot's
facing), and otherwise the procedural fallback — a solid box with a gable roof,
two to four storeys tall, named `PROCEDURAL_PREFAB` (`"procedural_house"`). An
untagged prefab with volume < 4 is never used as a building; an empty prefab
library fills every plot with the fallback, so a town is never an empty field of
plots. A building stands on the *highest* ground sample over its plot, so it is
never buried in a slope.

## Props

Trees and rocks are scattered one roll per tile from
`RngStream::for_chunk(seed, "chunk/props", x, y)`. The per-tile probability is
`prop_density * biome_density * 10.0` clamped to `[0, 1]`, using the biome's tree
density for trees and its rock density for rocks. A prop is rejected when **any**
of these holds (asserted by `props_respect_their_rejection_rules`):

| Rule | Why |
|---|---|
| `height < sea_level` | no trees standing in water |
| `slope > MAX_PROP_SLOPE` (0.84) | nothing grows on a cliff face |
| the tile is the road slot, or `is_road_at` | a road stays clear |
| the tile is the water slot | a lake with a non-water biome still has no props |
| inside a building footprint | no trees in walls |
| the 2 m spacing cell is used | a forest must not become a wall of canopies |

Survivors get a random yaw in `[0, TAU)` and a uniform scale in `[0.85, 1.2]`.

## Collision and occluder emission

A chunk carries two `(Aabb, u64)` lists: `Chunk::colliders` (building volumes
clipped to the chunk, plus each prop's solid box) and `Chunk::occluder_boxes()`
(the same building volumes plus props whose `PropKind::is_occluder()` is true —
trees only).

The unit is **one merged box per building run, not one per voxel**, and that is
the most important thing to understand here. `merge_voxel_runs` collapses a
prefab's voxels greedily: each `(x, z)` column becomes a vertical run, runs are
grouped by height span, then grown along `+X` and `+Z` while the neighbours
agree. A solid house becomes one box, an L-shaped one two or three, and the
procedural fallback exactly two (wall run plus roof volume).

Why it matters: `noxel-visibility` ray-tests these volumes **every frame**, and
the fade is per object (fading half a roof is meaningless). A town of 400
buildings is 400 boxes and a ray costs a couple of microseconds; one box per
voxel would be tens of thousands and would make the fade unaffordable. Props are
the same: a tree is a thin solid trunk (half width 0.28 m, height 1.8 m) plus a
wide canopy occluder (half width 1.5 m, base 1.5 m, height 3.6 m).

Ids are stable across regeneration — `BuildingInstance::id` hashes the prefab
name, origin and yaw; `PropInstance::id` hashes the name and the position
quantised to 1/16 m — so a collider id survives eviction and regeneration.

## Chunk data layout

`Chunk` is dense and flat: every array is `tiles * tiles` long, row-major as
`y * tiles + x` — the order the renderer uploads and `chunk_to_string` prints.

| Field | Type | Contents |
|---|---|---|
| `pos` | `ChunkPos` | where it sits; `y` is the **Z** axis |
| `tiles` | `Vec<u32>` | tile ids |
| `heights` | `Vec<f32>` | terrain height per tile, metres |
| `slopes` | `Vec<f32>` | slope magnitude per tile |
| `biome` | `BiomeId` | the biome at the chunk's centre |
| `colliders` | `Vec<(Aabb, u64)>` | static collision, as above |
| `props` | `Vec<PropInstance>` | trees, rocks, anything else placed |
| `buildings` | `Vec<BuildingInstance>` | buildings intersecting this chunk |
| `roads` | `Vec<RoadSegment>` | road centre lines clipped to this chunk |
| `has_town` | `bool` | any part of a town is here |
| `generated_ms` | `f32` | generation time, `0` when read from cache |
| `tile_size` | `f32` | metres per tile, copied from the config |

Accessors never panic: `tile`/`height`/`slope` return `0` out of range, `index`
returns `None`. `is_walkable` treats out-of-range as not walkable;
`blocks_sight` treats it as blocking — an actor must not walk off the edge of the
loaded world, and visibility must not see through it.

## Streaming

`WorldStreamer` owns a `WorldGenerator` and an LRU cache of chunks.

| Knob | Default | Effect |
|---|---|---|
| `view_distance_chunks` | 6 | chunks generated around the focus: `(2v+1)² = 169` |
| `keep_distance_chunks` | 8 | anything further (Chebyshev) is evicted |
| `max_cached_chunks` | 256 | hard cap; furthest, least-recently-touched first |

`update(focus)` is cheap when nothing changed: if the focus is still in the same
chunk and the view distance is unchanged it returns immediately, reporting
`generated_this_update == 0`. Lifetime counters (`cache_hits`, `cache_misses`)
and per-update counters (`generated_this_update`, `evicted_this_update`) are
separate. Moving one chunk generates one new row or column (13 chunks) and
evicts nothing until the keep ring is crossed.

## Queries

`WorldGenerator` answers from the same fields the chunks are built from, at any
coordinate, without generating anything; `WorldStreamer` prefers resident chunks
and falls back to the generator.

| Query | Generator | Streamer | Notes |
|---|---|---|---|
| `sample_height(x, z)` | ✔ | `height_at(Vec3)` | exact inside a loaded chunk |
| `sample_slope(x, z)` | ✔ | — | rise over run |
| `biome_at(x, z)` | ✔ | — | |
| `is_road_at(x, z)` | ✔ | — | lattice plus town streets |
| `town_at(ChunkPos)` | ✔ | — | `Option<TownPlan>` |
| `tile_at(Vec3)` | — | ✔ | `None` when not loaded |
| `is_walkable(Vec3)` | — | ✔ | falls back to height and slope |
| `blocks_sight(Vec3)` | — | ✔ | reports `true` outside |
| `colliders_in` / `occluders_in` / `props_in` | — | ✔ | fill a caller's `Vec` |

Non-finite input never panics: `sample_height`/`sample_slope` return `sea_level`
and `0`, `biome_at` returns `PLAINS`, `is_road_at` returns `false`.

## Configuration

Every `WorldConfig` field and the value `WorldConfig::new(seed)` uses (`default()`
is the same with seed `0`):

| Field | Default | Notes |
|---|---|---|
| `seed` | `0` | everything generated descends from it |
| `tile_size` | `1.0` | metres per tile |
| `tiles_per_chunk` | `32` | clamped to `1..=MAX_TILES_PER_CHUNK` (1024) |
| `chunk_world_size` | `32.0` | metres; call `sync_sizes()` after editing the two above |
| `view_distance_chunks` | `6` | |
| `keep_distance_chunks` | `8` | `view + 2`, never below `view` |
| `max_cached_chunks` | `256` | at least 1 |
| `sea_level` | `0.0` | the waterline |
| `height_scale` | `8.0` | metres of relief |
| `road_grid_chunks` | `8` | 256 m lattice |
| `road_jitter` | `0.25` | fraction of a chunk a line may wander |
| `road_width_tiles` | `3` | paved width |
| `town_spacing_chunks` | `10` | a town every 10th lattice position |
| `town_radius_chunks` | `3` | built-up radius |
| `buildings_per_town` | `18` | before the style scale; `0` reserves plots |
| `generate_props` | `true` | master switch |
| `prop_density` | `0.02` | scaled by the biome's own density |

The config is sanitised rather than trusted: `chunk_world_size()` and
`tile_size()` derive fallbacks from a zero or non-finite field,
`safe_tiles_per_chunk()` clamps, `road_spacing()` and `town_spacing()` never
return zero, and `sync_sizes()` re-derives the chunk size.

## Determinism guarantees

- `two_generation_orders_agree`: generate a chunk, visit four unrelated chunks,
  regenerate the target — the text dumps are equal. Collider ids and bounds also
  survive regeneration (`colliders_have_stable_ids_and_finite_bounds`).
- `chunk_to_string` is byte-stable, which makes it a golden-world format; heights
  print in decimetres so a diff shows metre changes, not float noise.
- The field is continuous: a 0.1 m step moves the surface by under 0.5 m, and
  halving the step roughly halves the largest change.

The one thing that breaks determinism is editing the config after chunks are
cached: `config_mut()` takes effect immediately, but an existing streamer must be
`clear()`ed.

## Authoring a prefab

A prefab is a small voxel model (`noxel_asset::format::Prefab`). Author it as
JSON and list it in the asset manifest:

```json
{
  "name": "house_small",
  "size": [5, 3, 4],
  "tags": ["building", "residential"],
  "voxels": [{ "x": 0, "y": 0, "z": 0, "tile": 12 }],
  "spawns": [{ "name": "door", "position": [2.0, 0.0, 3.5], "yaw": 0.0 }],
  "occluders": [[0, 0, 0, 4, 2, 3]]
}
```

| Key | Required | Meaning |
|---|---|---|
| `name` | yes | used by `BuildingInstance::prefab` and `AssetDb::prefab(path)` |
| `size` | yes | `[x, y, z]` in tiles; `size[1] == 0` makes it unusable as a building |
| `tags` | no | `"building"`/`"house"` makes it preferred for town plots |
| `voxels` | no | filled cells; an absent cell is empty space |
| `props` | no | `kind`, `position`, optional `yaw`/`scale` |
| `spawns` | no | named markers, e.g. a door |
| `occluders` | no | `[x0,y0,z0,x1,y1,z1]` voxel bounds that hide the player |

Rules that decide whether your prefab is used: a plot offers `plot_size - 1`
tiles after rotation onto its facing (east/west swaps X and Z), so a `5x4` prefab
needs a `6x5` plot; `choose_footprint` ignores prefabs over 12 tiles on either
axis; `size[1]` becomes `size[1] * tile_size` metres tall. With `occluders`
empty the generator merges `voxels` — author them explicitly when the greedy
merge would cover a courtyard, because a box over empty space fades and occludes
things that are visible. Voxel `(0, 0, 0)` is the minimum corner, and the
building is inset half a tile into its plot so the reserved margin stays a
visible gap.

Voxel `tile` values are ids in the same `TileSet` the terrain uses. The renderer
does not draw prefab voxels directly — they feed bounds, collision and occlusion
— so a voxel with no tile still collides.

## Common mistakes

- **"I changed the seed and the world did not change."** The streamer's cache
  still holds old chunks. Call `WorldStreamer::clear()` after changing `seed`,
  `tile_size` or `tiles_per_chunk`.
- **"`Chunk::tiles()` returns 0."** You replaced `chunk.tiles` with a non-square
  length by hand; every accessor now degrades to its out-of-range value.
- **"Every tile is id 0."** The tile set is empty or missing every name the
  generator asks for. Check `generator.tile_set().tile_by_name("grass")`: if the
  manifest did not load, `App` falls back to an empty `TileSet`.
- **"Props are missing from a forest."** `generate_props` is off, `prop_density`
  is 0, or the tile is a road or water tile.
- **"`town_at` returns a plan for wilderness."** A town has a 3-chunk radius, so
  up to seven chunks in each direction report `has_town`. Test a position with
  `TownPlan::contains_world`, not a chunk.
- **"The camera fade is expensive in a town."** You are adding unmerged volumes
  to the visibility scene; the generator's own occluders are merged for exactly
  this reason.
- **"`sample_height` disagrees with a chunk tile."** Chunk heights are sampled at
  tile *centres*: `origin + (tile + 0.5) * tile_size`, not tile corners.
