#!/usr/bin/env python3
"""Bakes Noxel's UI font atlas.

Two faces go into one texture:

* **`pixel`** — the hand-authored 5x7 Latin font in `pixelfont.py`. Pure bitmap
  data: no rasteriser, no hinting, no antialiasing, so it is exactly as crisp as
  the grid it was drawn on and stays crisp at every integer upscale.
* **`cjk`** — Chinese glyphs rasterised from a system font and *thresholded to
  one bit*. The threshold is the whole trick: an antialiased grey edge is what
  makes small text look blurry once the framebuffer is upscaled to the window,
  and a hard cut on the pixel grid is what makes it read as pixel art instead.

The output is a PNG plus a JSON sidecar of metrics. The engine loads both at
runtime through `noxel-ui`; nothing about the bake is needed to play the game.

Usage:

```text
python3 tools/fontgen/fontgen.py --out games/noxel-valley/assets/fonts
python3 tools/fontgen/fontgen.py --out DIR --cjk-size 12 --threshold 100
```

The bake is deterministic: the same font file, size and threshold produce the
same bytes, so the atlas is safe to commit and a diff means something changed.

Requires Pillow (`pip install Pillow`). This is a development-time tool; it is
not part of the engine's zero-dependency build.

Metrics
-------

Every glyph is stored as a tight ink box plus an offset. The offset is measured
from the **pen origin at the line top**, which is `(pen_x, line_top)` in the
renderer, so drawing a glyph is `blit(rect, pen_x + dx, line_top + dy)` with no
further arithmetic.

The line box is derived, not fixed: a string that uses both faces sets
`baseline = max(face.baseline)` and `line_height = max(face.line_height)` over
the faces it actually used. That is what lets 9px Latin and 14px Chinese sit on
one baseline in one line of text instead of two.
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


def gb2312_level1() -> str:
    """The hanzi and symbols a Chinese reader actually meets, from the encoding.

    Decoding GB2312 rather than shipping a character list keeps this file
    honest: level 1 *is* the standard "common characters" set, and every one of
    them is in the encoding by definition. Rows 1-9 add the CJK punctuation and
    fullwidth forms that a UI needs; rows 16-55 are the 3755 level-1 hanzi.
    Level 2 is deliberately left out — those characters are rare enough that a
    12px cell cannot draw them legibly, so they would triple the atlas for no
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
        # into `dy` instead would make a two-face line double-count it, which is
        # exactly what happened before this comment existed — Latin text sat six
        # pixels below the Chinese it was supposed to share a baseline with.
        glyphs[char] = Glyph(
            bitmap, left, -pixelfont.BASELINE, left + bitmap.width + pixelfont.TRACKING
        )
    return glyphs, pixelfont.HEIGHT, pixelfont.BASELINE


def bake_cjk_face(font_path: str, size: int, threshold: int, chars: str):
    """Thresholds `chars` to 1-bit at `size`, cropped and baseline-anchored.

    Each glyph is drawn on its own canvas and cropped to its ink box, so the
    atlas wastes nothing and the metrics stay exact. The crop is what makes a
    thresholded font sit on the pixel grid instead of on a fractional origin.
    """
    try:
        font = ImageFont.truetype(font_path, size)
    except OSError as error:
        raise SystemExit(f"fontgen: cannot open {font_path}: {error}") from error

    ascent, descent = font.getmetrics()
    padding = max(2, size // 4)
    baseline_abs = padding + ascent

    glyphs: dict[str, Glyph] = {}
    for char in chars:
        # A generous canvas: some glyphs overflow their nominal metrics, and a
        # clipped stroke is a wrong glyph rather than a small one.
        canvas = Image.new("L", (size * 2 + padding * 2, ascent + descent + padding * 2), 0)
        ImageDraw.Draw(canvas).text((padding, padding), char, font=font, fill=255)
        binary = canvas.point(lambda p: 255 if p >= threshold else 0, mode="1").convert("L")
        box = binary.getbbox()
        if box is None:
            glyphs[char] = Glyph(Image.new("L", (1, 1), 0), 0, 0, size)
            continue
        left, top, _right, _bottom = box
        # `top - baseline_abs` is the offset above the baseline, which is
        # negative. Converting to the line top happens in `build`.
        glyphs[char] = Glyph(binary.crop(box), left - padding, top - baseline_abs, size)
    return glyphs, ascent, descent


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


def build(out_dir: Path, cjk_font: str | None, cjk_size: int, threshold: int, atlas_width: int):
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
    hanzi, _, _ = bake_cjk_face(resolved_font, cjk_size, threshold, gb2312_level1())

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
        blank = glyph.bitmap.width == 1 and glyph.bitmap.height == 1 and glyph.bitmap.getpixel((0, 0)) == 0
        entry["glyphs"][char] = {
            "rect": [0, 0, 0, 0] if blank else [x, y, glyph.bitmap.width, glyph.bitmap.height],
            "offset": [glyph.dx, entry["baseline"] + glyph.dy],
            "advance": glyph.advance,
        }

    document = {
        "atlas": {"width": atlas_width, "height": height},
        "faces": [faces["pixel"], faces["cjk"]],
        "fallback": ["pixel", "cjk"],
        "source": {"cjk_font": resolved_font, "cjk_size": cjk_size, "threshold": threshold},
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
    print(f"fontgen: {png_path}  {atlas_width}x{height}")
    print(f"fontgen: {json_path}")
    return png_path, json_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path, help="asset directory to write into")
    parser.add_argument("--cjk-font", default=None, help="path to a CJK .ttf/.ttc")
    parser.add_argument("--cjk-size", type=int, default=12, help="CJK pixel size (default 12)")
    parser.add_argument("--threshold", type=int, default=100, help="1-bit cut, 0-255 (default 100)")
    parser.add_argument("--atlas-width", type=int, default=1024, help="atlas width (default 1024)")
    args = parser.parse_args()
    build(args.out, args.cjk_font, args.cjk_size, args.threshold, args.atlas_width)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
