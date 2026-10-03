#!/usr/bin/env python3
"""Draws the Noxel mark, and the macOS icon set that ships with it.

    python3 tools/logo/logo.py --out dist/logo

The mark is a cube whose top face is a 3x3 grid of pixels with two missing —
a voxel, and a picture of one. It is drawn on a **32x32 logical grid** with
integer coordinates only, then scaled by whole numbers to 32, 64, ... 1024.
That is the same rule the engine's art follows: a mark that is resampled is a
mark that is soft, and an icon is the one image a user sees at every size.

Colours are the engine's own: `--accent` defaults to the cyan the debug overlay
uses, so the mark and the tooling agree.
"""

import argparse
import pathlib
import shutil
import subprocess
import sys

from PIL import Image, ImageDraw

GRID = 32

BACKDROP = (18, 21, 28, 255)
BACKDROP_EDGE = (44, 50, 62, 255)
INK = {
    "top": (126, 231, 239, 255),
    "left": (58, 168, 200, 255),
    "right": (38, 108, 144, 255),
}

# The cube, as the three faces of an isometric projection on the 32-grid.
# Whole numbers throughout: every edge lands on a pixel boundary.
TOP = (16, 4)
LEFT = (4, 16)
FRONT = (16, 28)
RIGHT = (28, 16)
CENTRE = (16, 16)

# The mark is a solid cube: three faces, three tones, one silhouette.
#
# An earlier draft cut two cells out of the roof to say "voxel" — a volume made
# of pixels. It read as damage. At icon sizes the top face is a dozen pixels
# across, so a gap in it is not a grid, it is a chip out of the shape. The
# hard-edged isometric geometry already says pixel, and says it at 16 pixels as
# clearly as at 1024.


def cube() -> Image.Image:
    """The mark on a 32x32 grid, hard-edged, no antialiasing."""
    image = Image.new("RGBA", (GRID, GRID), (0, 0, 0, 0))

    def quad(points, colour):
        # A polygon drawn without antialiasing: `ImageDraw` at this size with
        # integer vertices lands on exact pixels, which is the whole point.
        ImageDraw.Draw(image).polygon(points, fill=colour)

    quad([TOP, RIGHT, CENTRE, LEFT], INK["top"])
    quad([LEFT, CENTRE, FRONT], INK["left"])
    quad([CENTRE, RIGHT, FRONT], INK["right"])

    return image


def rounded_backdrop(size: int) -> Image.Image:
    """The tile the mark sits on: a dark rounded square, four pixels of radius."""
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    radius = max(2, size // 8)
    draw.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=BACKDROP)
    draw.rounded_rectangle(
        [0, 0, size - 1, size - 1], radius=radius, outline=BACKDROP_EDGE, width=max(1, size // 64)
    )
    return image


def render(size: int) -> Image.Image:
    """One icon at exactly `size`, scaled from the 32-grid by a whole number."""
    canvas = rounded_backdrop(size)
    if size % GRID == 0:
        mark = cube().resize((size, size), Image.NEAREST)
    else:
        # Sizes below the grid (16) get a box filter, which at that scale is a
        # faithful reduction rather than a resample.
        mark = cube().resize((size, size), Image.BOX)
    canvas.alpha_composite(mark)
    return canvas


def wordmark(height: int = 128) -> Image.Image:
    """The mark beside the name, for a README header."""
    mark = render(height)
    text = Image.new("RGBA", (height * 5, height), (0, 0, 0, 0))
    text.alpha_composite(mark, (0, 0))
    try:
        from PIL import ImageFont

        font = ImageFont.load_default(size=int(height * 0.55))
    except Exception:
        font = None
    if font is not None:
        ImageDraw.Draw(text).text(
            (int(height * 1.16), height // 2), "noxel", fill=(226, 232, 240, 255), font=font, anchor="lm"
        )
    return text


def iconset(out: pathlib.Path) -> pathlib.Path:
    """A macOS `.iconset`, then the `.icns` built from it."""
    iconset_dir = out / "Noxel.iconset"
    if iconset_dir.exists():
        shutil.rmtree(iconset_dir)
    iconset_dir.mkdir(parents=True)
    for size in (16, 32, 64, 128, 256, 512, 1024):
        image = render(size)
        image.save(iconset_dir / f"icon_{size}x{size}.png")
        # Retina variants: the same image at twice the nominal size.
        if size * 2 <= 1024 and size >= 16:
            image.save(iconset_dir / f"icon_{size}x{size}@2x.png")
    icns = out / "Noxel.icns"
    if shutil.which("iconutil"):
        subprocess.run(["iconutil", "-c", "icns", str(iconset_dir), "-o", str(icns)], check=True)
    return icns


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    for size in (16, 32, 48, 64, 128, 256, 512, 1024):
        render(size).save(args.out / f"noxel-{size}.png")
    wordmark().save(args.out / "noxel-wordmark.png")
    icns = iconset(args.out)
    print(f"logo: 8 sizes + wordmark + {icns.name} -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
