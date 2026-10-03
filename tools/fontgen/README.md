# fontgen — the UI font bake

Produces the bitmap font that `noxel-ui` draws with. One atlas, two faces.

```bash
python3 tools/fontgen/fontgen.py --out games/noxel-valley/assets/fonts
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

That leaves two problems the bake solves differently for each script.

## The two faces

### `pixel` — the Latin face

Hand-authored in [`pixelfont.py`](pixelfont.py) as ASCII art: 95 glyphs on a
5-wide by 9-tall grid, cap height 7, baseline on row 6, with two descender rows
so `g j p q y , ;` have somewhere to go. Nothing rasterises it — it is bitmap
data drawn on the grid, which is why it is exactly as crisp as that grid.

Adding a character means drawing it, and the parser rejects a row of the wrong
width rather than silently shifting every glyph after it.

### `cjk` — the Chinese face

Rasterised from a system font and thresholded. Decoding GB2312 rather than
shipping a character list keeps the tool honest: level 1 *is* the standard
"characters a Chinese reader meets in ordinary text" set, and every one of them
is in the encoding by definition.

```text
rows 1-9    CJK punctuation, fullwidth forms
rows 16-55  3755 level-1 hanzi
rows 56-87  level 2 — deliberately excluded
```

Level 2 is left out because 12 pixels cannot draw those characters legibly; they
would triple the atlas for no readable gain. The result is 4437 glyphs in a
117 KB PNG and a 265 KB JSON.

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

## Output

| File | What it is |
|---|---|
| `ui_font.png` | the atlas: white RGB with coverage in alpha, so the painter can tint it to any colour |
| `ui_font.json` | the metrics, compact and key-sorted so a diff is stable |

Also written is a `source` block recording which font file and threshold produced
the bake, because two machines with different fonts installed would otherwise
produce two different atlases from the same command.

## Options

```text
--out DIR          where to write (required)
--cjk-font PATH    a CJK .ttf/.ttc; searched for by default
--cjk-size N       pixel size for the CJK face (default 12)
--threshold N      the 1-bit cut, 0-255 (default 100)
--atlas-width N    atlas width (default 1024)
```

12 pixels is the default because it is the size at which the common characters
stay legible; 10 loses too much detail and 14 makes Chinese noticeably larger
than the Latin beside it. Lowering the threshold bolds the glyphs, which helps
dense characters at small sizes and starts to close the counters.
