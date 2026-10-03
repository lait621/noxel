# Noxel Valley · 星野农场

A playable top-down farming game built on the Noxel engine, and the reference
project for the engine's UI layer.

```bash
cargo run -p noxel-valley --features window -- --window      # play it
cargo run -p noxel-valley -- --frames 300 --dump frames       # render frames to PNG
```

If you only want to *play*, do not build anything: the repository root has a
built bundle. See **[Playing it](#playing-it)** below.

---

## What it is

A farm, a house, a shop and a shipping bin, on a 44x34 tile map. You hoe soil,
plant seeds, water them, wait, harvest, and sell. The seasons turn every 28 days
and take your crops with them.

```text
  06:00   wake up. Energy restored. Today's weather was decided last night.
    ...   hoe -> plant -> water -> harvest -> carry to the bin -> sleep
  02:00   forced to bed
          |
          v
      1. the shipping bin is sold and the gold arrives
      2. rain waters every tilled tile
      3. watered plants gain a day; out-of-season plants die
      4. today's watering is cleared
      5. tomorrow's weather is rolled
      6. the calendar advances
```

The order is the design. Rain before growth is what makes a rainy day free for
the player; growth before clearing is what makes yesterday's watering count;
clearing last is what makes today start dry.

### What is in it

| System | What it does |
|---|---|
| **Farming** | Hoe, plant, water, harvest. Six crops, five growth stages each, two seasons of them, two that regrow after harvest. A crop left in the ground when its season ends dies. |
| **Time** | A 20-hour day (06:00 to 02:00) at two in-game minutes per real second, so a day is about ten minutes. Four 28-day seasons, a year counter, and a day-end report. |
| **Weather** | Sunny, cloudy, rain, storm, snow, with seasonal probabilities and a three-day forecast. Rain and storms water the whole farm; snow does not, because nothing grows in winter. |
| **Economy** | 500 gold, 24 slots, a seed shop that stocks only the current season, and a shipping bin that pays out the next morning. Seeds sell for half what they cost, so buying and reselling is a loss. |
| **Player** | Walking and running, energy that tools and walking both spend, a six-slot hotbar, and a tool that acts on the tile you are facing. |
| **UI** | A clock and weather panel, a purse, an energy bar, a hotbar, a 24-slot bag, the shop, the shipping bin, a morning report and a control reference — every one drawn with `noxel-ui`. |

---

## Playing it

### From the prebuilt bundle

`dist/noxel-valley/` is self-contained: the binary, the assets and a `README.txt`.
It needs no Rust toolchain, no source tree and no network.

```bash
cd dist/noxel-valley && ./noxel-valley --window
```

On macOS, `dist/Noxel Valley.app` is a double-clickable bundle — **开箱即玩**.

### From the source tree

```bash
cargo run -p noxel-valley --features window -- --window
```

The `window` feature is off by default so that the engine's own `cargo build`
still downloads nothing (`docs/adr/0002-no-dependencies.md`). Turning it on pulls
in `winit` and `softbuffer`, which `noxel-window` boxes in behind the same
feature.

### Controls

| Key | Action |
|---|---|
| `WASD` / arrows | Walk |
| `Shift` | Run |
| `Space` / left mouse | Use the tool in hand |
| `1`–`6` / scroll | Select a hotbar slot |
| `Tab` | Bag |
| `E` | Shop, shipping bin, or sleep — whichever you are standing next to |
| `?` | Controls |
| `Esc` | Close a screen, or quit |

### Command line

```text
--window        Play in a window. Needs `--features window`.
--frames N      Run N frames and stop.
--seed N        World and weather seed (decimal or 0x-prefixed).
--fast          Run the clock 24x, so a season passes in minutes.
--stats         Print one frame of statistics and exit.
--dump DIR      Write every frame as a PNG into DIR.
--assets DIR    Asset root. Found automatically by default.
--screen NAME   Open a screen at startup: inventory, shop, bin, summary.
```

`--screen` exists so a screenshot of the shop does not need someone to walk to
the shop first, which is how the figures in this document were made.

---

## Directory layout

```text
games/noxel-valley/
  Cargo.toml
  README.md              this file
  README.zh-CN.md        中文说明
  src/
    lib.rs               the game's rules, as a library
    main.rs              argument parsing, asset loading, the frame loop
    config.rs            every number and table the game is tuned by
    world.rs             the tile grid, the farm layout, the mesh built from it
    player.rs            movement, facing, the tool swing, screen-to-tile
    sim.rs               the clock, the weather, the bag, the economy
    assets.rs            finding and loading the atlases and the font
    skin.rs              the UI theme, built from the atlas
    ui.rs                every screen the player sees
  assets/                GENERATED — see below
    farm/                terrain, crops, props, characters, ui — PNG + JSON
    fonts/               the UI font atlas and its metrics
```

The assets are **generated**, not hand-drawn, and they are separated from the
code on purpose: `src/` holds no pixel data at all, and `assets/` holds no code.

### Regenerating the assets

```bash
# the sprites: 141 regions across five atlases
cargo run -p noxel-gen -- farm --out games/noxel-valley/assets

# the font: a hand-authored 5x7 Latin face plus the GB2312 common characters
python3 tools/fontgen/fontgen.py --out games/noxel-valley/assets/fonts

# a contact sheet of every sprite, for looking at
cargo run -p noxel-gen -- farm --out /tmp/farmart
open /tmp/farmart/farm/preview.png
```

Both generators are deterministic: running one twice changes no bytes, so a diff
in `assets/` means something real changed.

The game does **not** need them to run. With no asset tree it falls back to flat
colours and the engine's default theme, which is what lets `cargo test` build a
working game in a millisecond with no files on disk.

---

## How it works

### The frame

The game is one `noxel_app::Plugin`, which buys the whole loop:

```text
  App::step(dt)
    fixed_update    ValleyPlugin::update
                      player movement, tool use, the clock, the day rollover
    frame_update    the camera follows app.context.focus, which update() set
    render          one mesh per atlas
    draw_overlays   ValleyPlugin::draw   <- the entire UI
    resolve         linear HDR -> sRGB, once
```

Drawing the UI in `draw` rather than after `resolve` is the important part: the
interface is composited in the same linear space as the world, one tone curve
applies to both, and a colour authored as `#FF0000` resolves to exactly
`#FF0000`.

### Crispness, which is three decisions

The art is pixel art and it stays sharp because of three things that have to
agree:

1. **The camera** looks straight down an orthographic axis with an orthographic
   height of `270 / 16 = 16.875`, so **one world unit is exactly 16 screen
   pixels** in both directions. A tile is never resampled. A tilted camera would
   foreshorten the ground and scale every upright sprite by `cos(pitch)`, which
   is precisely the softness the art avoids.
2. **The renderer** samples textures with a half-texel-inset nearest neighbour
   (`Texture::sample_pixel_art`), so adjacent atlas cells cannot bleed into each
   other.
3. **The window** upscales by whole numbers only and letterboxes the remainder,
   and the same derivation maps the cursor back down — so a click lands on the
   pixel the player aimed at at every window size.

### Depth without a depth sort

Everything in the world is a flat quad on the ground plane. Two objects whose
sprites overlap have to be drawn back to front, and with a straight-down camera
world `z` *is* screen depth — so each quad gets a small `y` offset proportional
to its `z`, and the depth buffer does the sorting. That is the painter's
algorithm expressed in a way the rasterizer already implements.

### The three meshes

Ground, crops and props are separate meshes because a material carries exactly
one texture and they come from three atlases. The split also lets the ground
material carry the season's tint while the crops keep their true colours — a
pumpkin should not turn orange-er in autumn because the grass did.

The farm mesh is rebuilt whenever the map's revision changes, which is a few
times a second while the player is hoeing. At 44x34 tiles that is about six
thousand vertices; an incremental update would be more code for a cost nobody
can measure.

---

## Testing

```bash
cargo test -p noxel-valley            # 85 tests
```

The rules are tested as rules, without a renderer:

- every crop pays for itself within one season, and none returns more than eight
  times what it cost
- an unwatered crop does not grow; a watered one gains exactly one day
- rain waters every tilled tile exactly once
- a crop left in the ground when its season ends dies
- weather is deterministic for a seed and the forecast slides rather than
  re-rolling
- a full inventory reports what did not fit rather than dropping it
- the player never ends up inside a solid tile, walking in any direction
- the starting position is walkable

and the rendering contract is tested against the real assets:

- the shipped atlases load, the farm reaches the scene, and it **draws pixels** —
  culling accepting an instance is not the same as the rasterizer drawing it, and
  a mesh wound the wrong way passes every cull and is then rejected triangle by
  triangle.

---

## Known weak points

Honest list, in the order a player would notice:

- **Crop stages 1 and 2 are small blobs.** Stage 4 reads clearly for all six
  crops; the two before it are generic sprouts.
- **There is one villager and no dialogue.** The farm looks inhabited; it does
  not yet feel like a town.
- **No save file.** Quitting loses the season.
- **Winter is fallow by design** — nothing can be planted — and there is nothing
  to do instead yet.
- **The walk cycle is a one-to-two pixel leg swing.** It reads as walking in
  motion and the four frames look similar as a static row.
- **`log`, `crate` and `sprinkler` are the plainest props**, and the six seed
  packets differ only by the colour of a band.
