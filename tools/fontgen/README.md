# fontgen — the UI font bake

Produces the bitmap font that `noxel-ui` draws with. One atlas, two faces.

```bash
python3 tools/fontgen/fontgen.py --out <game>/assets/fonts
```

Requires Pillow (`pip install Pillow`). This is a **development-time** tool: it is
not part of the engine's build, and the engine's zero-third-party rule is intact
because the *output* is a PNG and a JSON file loaded through `noxel-asset` like
any other asset.

## Why bake a font at all

An antialiased glyph edge at a 480x270 internal resolution, upscaled four times to
a 1080p panel, is a grey blur four pixels wide. Every edge in the atlas is
therefore cut to **one bit** at an exact pixel size, so a glyph lands on whole
pixels and stays sharp at every integer upscale.

The bake asserts that: `assert_one_bit` fails the run rather than write an atlas
with a single grey level in it. `noxel-gen farm` makes the same assertion for the
sprite atlases.

## Supersampling: the part that decides whether Chinese is legible

Cutting to one bit is easy. Cutting *well* is the whole problem, and the obvious
implementation — draw the font at 12 px and threshold it — is the one that fails.

A hinted rasteriser at 12 px snaps every stem to a position. A stem that lands on
a half pixel is drawn at roughly 40% coverage, which is below any threshold that
keeps the rest of the glyph in one piece. The stems disappear, the counters close
up, and dense characters collapse: 「翻」 and 「爆」 become solid blocks, and
「日」 and 「曰」 come out **identical**.

So the bake supersamples:

```text
  1. draw the glyph at size x supersample, with antialiasing
  2. box-downsample to the target size          <- exact area averaging
  3. threshold the coverage to one bit
```

Step 2 is the point. Averaging *first* measures what the outline actually
covers: a stem occupying 60% of a target pixel arrives as grey 153 and survives a
threshold of 85, where the same stem rendered directly at 12 px was snapped to
40% and died. Supersampling does not invent detail that was never in the outline
— it stops the rasteriser's hint-time rounding from deciding which strokes are
real.

Two details that matter more than they look:

- The hi-res canvas is an **exact integer multiple** of the target in both
  dimensions, so the box filter is exact area averaging with no resampling phase
  error.
- The baseline is placed from the **target** font's metrics, never from the
  supersized font's own ascent. The two agree to within a fraction of a pixel,
  and a fraction of a pixel is exactly what becomes a one-pixel baseline shift
  applied inconsistently glyph by glyph.

### Measured: supersample factor

| Factor | Result |
|---|---|
| 1 | the defect. 翻/爆/睡 are blobs, 日 and 曰 are identical |
| 4 | strokes separate, 日 and 曰 differ — **the default** |
| 8 | indistinguishable from 4, for four times the bake time |

### Measured: threshold

**Raising the threshold erodes the glyph.** That is the counter-intuitive part:
on a direct render a higher cut removes antialiasing fringe, but on a
supersampled render it removes *ink*, because averaging spreads coverage across
pixel boundaries. Cutting at 140 shatters characters that survived 140 on the old
pipeline.

At the default size of 12 with supersample 4, on 翻 爆 睡 踢 日 曰 目 自 圃 疆 繁:

| Threshold | Result |
|---|---|
| 70 | too heavy — strokes merge, 圃's counters begin to fill |
| **85** | distinct strokes, 日 and 曰 differ, 翻 keeps its 羽 — **the default** |
| 100 | workable, but the densest characters start to fragment |
| 140 | shatters: 日 reduces to a box, 翻 loses whole radicals |

The optimum moves with the size, because a larger cell gives each stroke more
pixels to survive in. A bake at 14 or 16 wants roughly 100.

### Measured: target size

| Size | Result |
|---|---|
| 12 | **the default.** Fixes the blur on its own |
| 14 | visibly better — at 7x, 第 年 春 separate cleanly where 12 still merges the 竹 radical |
| 16 | best legibility, and too wide to fit the game's panels |

The default is 12 for a reason that is not about legibility. The CJK advance
equals the size, so every Chinese string is 17% wider at 14 than at 12, and three
of the game's panels have a hard-coded width:

```text
help panel   (300 wide, inner 290)   widest line  288 at 12  →  336 at 14   OVERFLOWS
shop panel   (240 wide, inner 230)   widest line  216 at 12  →  252 at 14   OVERFLOWS
bin panel    (220 wide, inner 210)   widest  192 at 12  →  224 at 14   OVERFLOWS
```

At the default 12 the metrics are **byte-identical** to the pre-supersampling
bake, so switching the pipeline changes no layout anywhere. `--cjk-size 14` is
there for whoever widens those panels; it is a strictly better-looking font.

## The two faces

### `pixel` — the Latin face

Hand-authored in [`pixelfont.py`](pixelfont.py) as ASCII art: 95 glyphs on a
5-wide by 9-tall grid, cap height 7, baseline on row 6, with two descender rows
so `g j p q y , ;` have somewhere to go. Nothing rasterises it — it is bitmap
data drawn on the grid, which is why it is exactly as crisp as that grid.

Adding a character means drawing it, and the parser rejects a row of the wrong
width rather than silently shifting every glyph after it.

### `cjk` — the Chinese face

Supersampled from a system font and thresholded, as above. Decoding GB2312 rather
than shipping a character list keeps the tool honest: level 1 *is* the standard
"characters a Chinese reader meets in ordinary text" set, and every one of them
is in the encoding by definition.

```text
rows 1-9    CJK punctuation, fullwidth forms
rows 16-55  3755 level-1 hanzi
rows 56-87  level 2 — deliberately excluded
```

Level 2 is left out because a 12-pixel cell cannot draw those characters
legibly; they would triple the atlas for no readable gain. The result is 4437
glyphs in a 114 KB PNG and a 265 KB JSON.

## Metrics, and the one rule that matters

Each glyph is stored as a tight ink box plus an offset, and the offset is
measured **from the baseline**, upwards being negative, for every face.

The line box is then *derived* rather than declared: a string that uses both faces
sets

```text
baseline    = max(face.baseline)     over the faces it actually used
line_height = max(face.line_height)
```

and the renderer shifts each glyph by `line_baseline - face_baseline`. That is
what lets 9-pixel Latin and 14-pixel Chinese sit on one baseline in one line of
text.

Measuring from a common origin is the whole trick, and it was got wrong once: the
Latin face was baked with its own baseline folded into the offset, so the shift
was applied twice and Latin text sat six pixels below the Chinese it was supposed
to align with. If you change the bake, check that rule first.

Changing the supersample factor or the threshold cannot break it — neither touches
`line_height`, `baseline` or `advance`. Changing `--cjk-size` does, and it will
move every Chinese glyph relative to the Latin beside it.

## Output

| File | What it is |
|---|---|
| `ui_font.png` | the atlas: white RGB with coverage in alpha, so the painter can tint it to any colour |
| `ui_font.json` | the metrics, compact and key-sorted so a diff is stable |

The JSON schema is consumed by `noxel-ui::FontSet::from_json` and is deliberately
not something this tool may change. The `source` block records the font file,
size, threshold and supersample factor, because two machines with different fonts
installed — or with different defaults — would otherwise produce two different
atlases from the same command.

## Options

```text
--out DIR          where to write (required)
--cjk-font PATH    a CJK .ttf/.ttc; searched for by default
--cjk-size N       pixel size for the CJK face (default 12)
--threshold N      the 1-bit cut, 0-255 (default 85; lower keeps thin strokes)
--atlas-width N    atlas width (default 1024)
--supersample N    subpixels per axis before averaging down (default 4)
--threshold-before-downsample
                   cut to one bit at high resolution instead of averaging
                   greyscale first. Kept because it is the obvious thing to try,
                   and it had to be measured to be rejected: cutting early throws
                   away the sub-pixel coverage the averaging step needs, so it is
                   strictly worse.
```

## What is still bad

- **The densest characters are still approximate.** 翻 爆 疆 繁 carry more strokes
  than a 12-pixel cell can hold, so at 1x they are recognisable in context rather
  than readable in isolation. Context is what a UI has; isolation is what a
  specimen sheet has.
- **Some near-identical pairs stay close.** 日/曰 and 目/自 are now *distinguishable*
  — they differ by a pixel — but they are not comfortable at 12 px. No bake fixes
  that; only a larger cell does.
- **The Latin face is unchanged and now looks small.** It was already 9 px against
  14 px Chinese; the size mismatch is the same as it always was. The supersampling
  does not touch it, because a hand-drawn bitmap has no antialiasing to remove.
- **`--threshold-before-downsample` breaks 日 down to a bare box at threshold 140.**
  It is in the tool as a documented dead end, not as an option to use.
