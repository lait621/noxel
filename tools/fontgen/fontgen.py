#!/usr/bin/env python3
"""Bakes Noxel's UI font atlas.

Two faces go into one texture:

* **`pixel`** — the hand-authored 5x7 Latin font in `pixelfont.py`. Pure bitmap
  data: no rasteriser, no hinting, no antialiasing, so it is exactly as crisp as
  the grid it was drawn on and stays crisp at every integer upscale.
* **`cjk`** — Chinese glyphs rasterised from a system font, supersampled,
  thresholded to one bit.

Why one bit, and how it is produced
-----------------------------------

The engine needs **one-bit** glyphs. An antialiased grey edge at a 480x270
internal resolution, upscaled four times to a 1080p panel, is a grey blur four
pixels wide; a hard cut on the pixel grid is what makes the text read as pixel
art instead.

Getting that cut right is the whole problem, and the obvious implementation —
draw the font at 12 px and threshold it — is the one that fails. A hinted
rasteriser at 12 px snaps each stem to a position, and a stem that lands on a
half pixel is drawn at ~40% coverage, which is below any threshold that keeps
the rest of the glyph in one piece. The stems vanish, counters close up, and
dense characters collapse into blobs. 「日」 and 「曰」 come out identical.

So the bake supersamples:

```text
   1. draw the glyph at size * supersample, with antialiasing
   2. box-downsample to the target size   <- exact area averaging
   3. threshold the coverage to one bit
```

The middle step is the point. Downsampling *first* preserves what the outline
actually covers: a stem occupying 60% of a target pixel arrives as grey 153 and
survives a threshold of 100, where the same stem rendered directly at 12 px was
snapped to 40% and died. Supersampling does not add detail that was never in the
outline — it measures the coverage honestly instead of letting the rasteriser's
hint-time rounding decide which strokes are real.

The hi-res canvas is an **exact integer multiple** of the target in both
dimensions, so the box filter is exact area averaging with no resampling phase
error, and the baseline is placed from the *target* font's metrics rather than
from the supersized font's own ascent — the two agree to within a fraction of a
pixel, and a fraction of a pixel is what turns into a one-pixel baseline shift.

The output is a PNG plus a JSON sidecar of metrics. The engine loads both at
runtime through `noxel-ui`; nothing about the bake is needed to play the game.

Usage:

```text
python3 tools/fontgen/fontgen.py --out <game>/assets/fonts
python3 tools/fontgen/fontgen.py --out DIR --cjk-size 14 --threshold 100 --supersample 4
```

The bake is deterministic: the same font file, size, threshold and supersample
factor produce the same bytes, so the atlas is safe to commit and a diff means
something changed. All four are recorded in the JSON's `source` block so two
machines cannot silently produce different atlases from the same command.

Requires Pillow (`pip install Pillow`). This is a development-time tool; it is
not part of the engine's zero-dependency build.

Metrics
-------

Every glyph is stored as a tight ink box plus an offset. The offset is measured
from the **baseline**, upwards negative, for every face. The renderer shifts each
glyph by `line_baseline - face_baseline`, and that single rule is what lets 9px
Latin and its much taller Chinese neighbours sit on one baseline in one line of
text. Folding a face's own baseline into the glyph offset — which this file did
once — applies the shift twice and sinks Latin text six pixels below the Chinese
beside it.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pixelfont

#: Candidate system fonts for the CJK face, best first. The bake records which
#: one it used, because two machines with different fonts installed will
#: otherwise produce two different atlases from the same command.
CJK_FONT_CANDIDATES = [
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
]

#: Padding between glyphs in the atlas. One pixel is enough: the sampler is
#: nearest-neighbour and never interpolates across a cell boundary, and the
#: padding only exists so a future bilinear filter cannot bleed.
PADDING = 1

#: How many subpixels per axis the CJK face is rasterised at before being
#: averaged down. Four is where the returns flatten: 2 still loses the thinnest
#: stems, and 8 costs four times the bake time to change a handful of glyphs by
#: one pixel. See `bake_cjk_face` for why this exists at all.
DEFAULT_SUPERSAMPLE = 4

#: The 1-bit cut, applied to the *averaged-down* coverage.
#:
#: Lower than the 100 this file used to default to, and the direction is the
#: surprising part: supersampling spreads ink across pixel boundaries, so
#: raising the threshold now **erodes** thin strokes instead of cleaning them
#: up. Cutting at 140 on a supersampled bake shatters characters that survived
#: it on a direct one.
#:
#: Measured at the default size of 12, supersample 4, on the discriminating set
#: 翻 爆 睡 踢 日 曰 目 自 圃 疆 繁:
#:
#:   70   too heavy: strokes merge and 圃's counters begin to fill
#:   85   distinct strokes, 日 and 曰 finally differ, 翻 keeps its 羽
#:   100  workable, but the densest characters start to fragment
#:
#: The optimum moves with the size — a larger cell gives each stroke more pixels
#: and tolerates more erosion — so this default is tuned for the default 12 and
#: a bake at 14 or 16 wants roughly 100.
DEFAULT_THRESHOLD = 85

#: The CJK pixel size, and therefore the advance of every Chinese glyph.
#:
#: Deliberately the same 12 that the game's layout was built against. 14 is
#: visibly more legible — at 7x, 第 年 春 separate cleanly where 12 still merges
#: the 竹 radical — but a Chinese advance of 14 rather than 12 widens every
#: string by 17%, and three of the game's panels have a hard-coded width. With
#: the default left at 12 the metrics are byte-identical to the previous bake
#: and this change is purely a rasterisation fix; `--cjk-size 14` is there for
#: whoever widens those panels.
DEFAULT_CJK_SIZE = 12


def gb2312_level1() -> str:
    """The hanzi and symbols a Chinese reader actually meets, from the encoding.

    Decoding GB2312 rather than shipping a character list keeps this file
    honest: level 1 *is* the standard "common characters" set, and every one of
    them is in the encoding by definition. Rows 1-9 add the CJK punctuation and
    fullwidth forms that a UI needs; rows 16-55 are the 3755 level-1 hanzi.
    Level 2 is deliberately left out — those characters are rare enough that a
    small cell cannot draw them legibly, so they would triple the atlas for no
    readable gain.
    """
    chars: list[str] = []
    for high in list(range(0xA1, 0xAA)) + list(range(0xB0, 0xD8)):
        for low in range(0xA1, 0xFF):
            try:
                char = bytes((high, low)).decode("gb2312")
            except UnicodeDecodeError:
                continue
            if char not in chars:
                chars.append(char)
    return "".join(chars)


class Glyph:
    """One baked glyph: an ink box, where to put it, and how far the pen moves."""

    __slots__ = ("bitmap", "dx", "dy", "advance")

    def __init__(self, bitmap: Image.Image, dx: int, dy: int, advance: int) -> None:
        self.bitmap = bitmap
        self.dx = dx
        self.dy = dy
        self.advance = advance


def bake_pixel_face() -> tuple[dict[str, Glyph], int, int]:
    """Turns the art table into glyphs relative to the line top.

    Proportional spacing falls out of the ink crop: `i` is three columns wide
    and `m` is five, so a line of text is not padded out to a monospace grid it
    does not need. The right side bearing is one pixel, which is what keeps
    letters from touching.
    """
    glyphs: dict[str, Glyph] = {}
    for char, rows in pixelfont.GLYPHS.items():
        bounds = pixelfont.ink_bounds(rows)
        if bounds is None:
            # Blank, but still advances: this is the space character.
            glyphs[char] = Glyph(Image.new("L", (1, 1), 0), 0, 0, 3)
            continue
        left, right = bounds
        bitmap = Image.new("L", (right - left + 1, pixelfont.HEIGHT), 0)
        pixels = bitmap.load()
        for y, row in enumerate(rows):
            for x in range(left, right + 1):
                if row[x] == "#":
                    pixels[x - left, y] = 255
        # `dy` is measured from the **baseline**, upwards being negative, for
        # every face. The cell top is `BASELINE` pixels above the baseline, so
        # the whole-cell bitmap starts at `-BASELINE`.
        #
        # Measuring from a common origin is what lets the renderer align a Latin
        # glyph to a Chinese one: it knows each face's baseline and can shift a
        # glyph by `line_baseline - face_baseline`. Baking the face's baseline
        # into `dy` instead would make a two-face line double-count it.
        glyphs[char] = Glyph(
            bitmap, left, -pixelfont.BASELINE, left + bitmap.width + pixelfont.TRACKING
        )
    return glyphs, pixelfont.HEIGHT, pixelfont.BASELINE


def bake_cjk_face(
    font_path: str,
    size: int,
    threshold: int,
    chars: str,
    supersample: int = DEFAULT_SUPERSAMPLE,
    threshold_before_downsample: bool = False,
):
    """Rasterises `chars` at `size`, supersampled and thresholded to one bit.

    See the module docstring for why the supersample step exists. The short
    version: thresholding a 12 px render loses the stems the rasteriser rounded
    onto half pixels, and averaging down from 4x measures their real coverage
    instead.

    `threshold_before_downsample` is the naive variant — cut to one bit at high
    resolution, then average the 0/1 coverage down and cut again. It is kept
    because it is the obvious thing to try and it had to be measured to be
    rejected: cutting early throws away the sub-pixel coverage the averaging
    step needs, so it is strictly worse than averaging the greyscale.
    """
    ss = max(1, supersample)

    # Metrics come from the *target* size, never from the supersized font. The
    # two agree to within a fraction of a pixel, and a fraction of a pixel is
    # exactly what becomes a one-pixel baseline shift.
    try:
        target = ImageFont.truetype(font_path, size)
    except OSError as error:
        raise SystemExit(f"fontgen: cannot open {font_path}: {error}") from error
    ascent, descent = target.getmetrics()

    padding = max(2, size // 4)
    cell_w = size * 2 + padding * 2
    cell_h = ascent + descent + padding * 2
    baseline_abs = padding + ascent

    if ss == 1:
        draw_font = target
        origin = (padding, padding)
    else:
        draw_font = ImageFont.truetype(font_path, size * ss)
        ascent_hi, _ = draw_font.getmetrics()
        # Where the hi-res font would put its own ascender, versus where the
        # target metrics say the ascender belongs. The difference is the
        # correction; dropping it shifts every glyph by a subpixel, which the
        # threshold then rounds inconsistently glyph by glyph.
        origin = (padding * ss, baseline_abs * ss - ascent_hi)

    glyphs: dict[str, Glyph] = {}
    for char in chars:
        canvas = Image.new("L", (cell_w * ss, cell_h * ss), 0)
        ImageDraw.Draw(canvas).text(origin, char, font=draw_font, fill=255)

        if ss > 1 and threshold_before_downsample:
            canvas = canvas.point(lambda p: 255 if p >= threshold else 0, mode="L")
        if ss > 1:
            # BOX is exact area averaging for an integer factor. LANCZOS would
            # ring, and ringing around a 1-pixel stem is a grey halo that the
            # threshold turns into a broken stroke.
            canvas = canvas.resize((cell_w, cell_h), Image.BOX)

        binary = canvas.point(lambda p: 255 if p >= threshold else 0, mode="1").convert("L")
        box = binary.getbbox()
        if box is None:
            # Whitespace, or a glyph this font genuinely has no ink for.
            glyphs[char] = Glyph(Image.new("L", (1, 1), 0), 0, 0, size)
            continue
        left, top, _right, _bottom = box
        # `top - baseline_abs` is the offset above the baseline, which is
        # negative. Converting to the line top happens in `build`.
        glyphs[char] = Glyph(binary.crop(box), left - padding, top - baseline_abs, size)
    return glyphs, ascent, descent


def assert_one_bit(atlas: Image.Image) -> None:
    """Refuses to let a partially covered pixel reach the atlas.

    The window upscales by whole numbers and the renderer samples the atlas
    nearest-neighbour, so a grey pixel is not "softer" at runtime — it is a grey
    block four pixels wide on a 1080p panel. That is the exact defect this file
    exists to avoid, so it fails the bake rather than shipping. The sprite
    generator (`noxel-gen farm`) has the same assertion for its own atlases.
    """
    histogram = atlas.histogram()
    levels = [level for level, count in enumerate(histogram) if count]
    if levels != [0, 255] and levels != [255] and levels != [0]:
        raise SystemExit(
            f"fontgen: refusing to write an atlas with {len(levels)} grey levels "
            f"({levels[:8]}{'...' if len(levels) > 8 else ''}); "
            "every pixel must be fully transparent or fully opaque"
        )


class Shelf:
    """A trivial shelf packer. Every atlas here is a handful of rows of glyphs
    of near-identical height, so a real bin packer would be complexity without a
    payoff."""

    def __init__(self, width: int) -> None:
        self.width = width
        self.x = 0
        self.y = 0
        self.row_height = 0
        self.height = 0

    def place(self, w: int, h: int) -> tuple[int, int]:
        if self.x + w > self.width:
            self.x = 0
            self.y += self.row_height + PADDING
            self.row_height = 0
        x, y = self.x, self.y
        self.x += w + PADDING
        self.row_height = max(self.row_height, h)
        self.height = max(self.height, y + h)
        return x, y


def build(
    out_dir: Path,
    cjk_font: str | None,
    cjk_size: int,
    threshold: int,
    atlas_width: int,
    supersample: int = DEFAULT_SUPERSAMPLE,
    threshold_before_downsample: bool = False,
):
    out_dir.mkdir(parents=True, exist_ok=True)

    resolved_font = cjk_font
    if resolved_font is None:
        for candidate in CJK_FONT_CANDIDATES:
            if Path(candidate).exists():
                resolved_font = candidate
                break
    if resolved_font is None:
        raise SystemExit(
            "fontgen: no CJK font found; pass --cjk-font /path/to/font.ttc\n"
            "         (the Latin pixel face needs no system font)"
        )

    latin, latin_line, latin_baseline = bake_pixel_face()
    hanzi, _, _ = bake_cjk_face(
        resolved_font, cjk_size, threshold, gb2312_level1(), supersample, threshold_before_downsample
    )

    # The CJK line box is the font's own em box plus the two pixels of
    # descender a Latin `g` needs under a Chinese character.
    cjk_line = cjk_size + 2
    cjk_baseline = cjk_size

    # ---- pack -------------------------------------------------------------
    shelves = Shelf(atlas_width)
    entries: list[tuple[str, str, Glyph, int, int]] = []

    for char, glyph in sorted(latin.items()):
        x, y = shelves.place(glyph.bitmap.width, glyph.bitmap.height)
        entries.append(("pixel", char, glyph, x, y))
    for char, glyph in sorted(hanzi.items()):
        x, y = shelves.place(glyph.bitmap.width, glyph.bitmap.height)
        entries.append(("cjk", char, glyph, x, y))

    height = max(1, shelves.height)
    atlas = Image.new("L", (atlas_width, height), 0)
    for _face, _char, glyph, x, y in entries:
        atlas.paste(glyph.bitmap, (x, y))

    assert_one_bit(atlas)

    # Ink is white with coverage in alpha so the renderer can tint it: the
    # painter draws `texture.a` as coverage and substitutes the colour it was
    # asked for. One atlas therefore draws white, gold and grey text.
    rgba = Image.merge(
        "RGBA",
        (
            Image.new("L", atlas.size, 255),
            Image.new("L", atlas.size, 255),
            Image.new("L", atlas.size, 255),
            atlas,
        ),
    )

    # ---- metrics ----------------------------------------------------------
    def face(name: str, line_height: int, baseline: int) -> dict:
        return {"name": name, "line_height": line_height, "baseline": baseline, "glyphs": {}}

    faces = {
        "pixel": face("pixel", latin_line, latin_baseline),
        "cjk": face("cjk", cjk_line, cjk_baseline),
    }

    for face_name, char, glyph, x, y in entries:
        # `dy` is measured from the baseline; the renderer wants it from the top
        # of the face's own line box, so shift it down by the face's baseline.
        # Doing this here rather than in the renderer keeps the renderer's
        # arithmetic identical for every face.
        entry = faces[face_name]
        blank = (
            glyph.bitmap.width == 1
            and glyph.bitmap.height == 1
            and glyph.bitmap.getpixel((0, 0)) == 0
        )
        entry["glyphs"][char] = {
            "rect": [0, 0, 0, 0] if blank else [x, y, glyph.bitmap.width, glyph.bitmap.height],
            "offset": [glyph.dx, entry["baseline"] + glyph.dy],
            "advance": glyph.advance,
        }

    document = {
        "atlas": {"width": atlas_width, "height": height},
        "faces": [faces["pixel"], faces["cjk"]],
        "fallback": ["pixel", "cjk"],
        "source": {
            "cjk_font": resolved_font,
            "cjk_size": cjk_size,
            "threshold": threshold,
            # Recorded so two machines cannot silently bake different atlases
            # from the same command line, and so a reader can tell which
            # pipeline produced the file they are looking at.
            "supersample": supersample,
            "threshold_before_downsample": threshold_before_downsample,
        },
    }

    png_path = out_dir / "ui_font.png"
    json_path = out_dir / "ui_font.json"
    rgba.save(png_path, optimize=True)
    # Compact rather than indented: 4400 glyphs pretty-printed is a 650 KB file
    # in version control, and `sort_keys` is what keeps diffs stable, not the
    # whitespace.
    json_path.write_text(
        json.dumps(document, sort_keys=True, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )

    print(f"fontgen: {len(latin)} latin + {len(hanzi)} CJK glyphs")
    print(
        f"fontgen: size {cjk_size}px  supersample {supersample}x  threshold {threshold}"
        + ("  (threshold before downsample)" if threshold_before_downsample else "")
    )
    print(f"fontgen: {png_path}  {atlas_width}x{height}  {png_path.stat().st_size} bytes")
    print(f"fontgen: {json_path}  {json_path.stat().st_size} bytes")
    return png_path, json_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path, help="asset directory to write into")
    parser.add_argument("--cjk-font", default=None, help="path to a CJK .ttf/.ttc")
    parser.add_argument(
        "--cjk-size",
        type=int,
        default=DEFAULT_CJK_SIZE,
        help=(
            f"CJK pixel size (default {DEFAULT_CJK_SIZE}). Larger is more legible "
            "and wider: every Chinese advance grows, so panels that are sized for "
            "12 will overflow"
        ),
    )
    parser.add_argument(
        "--threshold",
        type=int,
        default=DEFAULT_THRESHOLD,
        help=f"1-bit cut, 0-255 (default {DEFAULT_THRESHOLD}; lower keeps thin strokes)",
    )
    parser.add_argument("--atlas-width", type=int, default=1024, help="atlas width (default 1024)")
    parser.add_argument(
        "--supersample",
        type=int,
        default=DEFAULT_SUPERSAMPLE,
        help=(
            "subpixels per axis before averaging down (default "
            f"{DEFAULT_SUPERSAMPLE}; 1 disables, which is the blurry setting)"
        ),
    )
    parser.add_argument(
        "--threshold-before-downsample",
        action="store_true",
        help="cut to one bit at high resolution instead of averaging greyscale first (worse)",
    )
    args = parser.parse_args()
    build(
        args.out,
        args.cjk_font,
        args.cjk_size,
        args.threshold,
        args.atlas_width,
        args.supersample,
        args.threshold_before_downsample,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
