#!/usr/bin/env bash
#
# Builds the runnable example: a self-contained `dist/` directory holding the
# compiled binaries and the assets they need, so it can be copied anywhere and
# run with no Rust toolchain, no source tree and no network.
#
#   ./scripts/build-dist.sh            # release binaries into dist/
#   ./scripts/build-dist.sh --debug    # faster to build, slower to run
#
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$(dirname "$0")/.."
root="$(pwd)"
profile="release"
flag="--release"
if [ "${1:-}" = "--debug" ]; then
    profile="debug"
    flag=""
fi

printf '\n== building (%s)\n' "$profile"
cargo build $flag -p town-demo -p noxel-gen

bin="$root/target/$profile"
out="$root/dist"
rm -rf "$out"
mkdir -p "$out"

printf '\n== assembling %s\n' "$out"
cp "$bin/town-demo" "$out/"
cp "$bin/noxel-gen" "$out/"
# The assets sit beside the binary, which is the first place the demo looks.
# That is what makes the directory relocatable.
cp -R "$root/examples/town-demo/assets" "$out/assets"

cat > "$out/README.txt" <<'TXT'
Noxel — town-demo
=================

This directory is self-contained. Nothing here needs Rust, the source tree, or
a network connection.

Run the village:

    ./town-demo                  # 600 frames, one PNG per frame into ./frames
    ./town-demo --help           # every option

Switch renderer:

    ./town-demo --mode hybrid    # raster primary + ray-traced shadows and AO
    ./town-demo --mode raytrace  # full ray tracing (slow, beautiful)

Look at the world rather than render it:

    ./town-demo --world-info     # biomes, heights and towns for the seed
    ./town-demo --stats          # one frame, the full statistics report

Regenerate the assets in ./assets from code:

    ./noxel-gen generate --out assets
    ./noxel-gen verify   --out assets

Frames land in ./frames beside wherever you ran the command. Pass
`--dump DIR` to choose another directory, or `--no-dump` to render without
writing anything.

There is no window: the demo renders to PNG files. `docs/guides/windowing.md`
in the source tree shows how to present the same frames on screen.
TXT

printf '\n== verifying the assembled directory runs\n'
( cd "$out" && ./town-demo --frames 5 --quiet >/dev/null && echo "  runs from inside dist/" )
work="$(mktemp -d)"
( cd "$work" && "$out/town-demo" --frames 5 --quiet --no-dump >/dev/null && echo "  runs from an unrelated directory" )
rm -rf "$work"

printf '\n== done: %s\n' "$out"
ls -lh "$out" | tail -n +2 | awk '{printf "   %-12s %s\n", $9, $5}'
printf '\n   assets: %s files\n' "$(find "$out/assets" -type f | wc -l | tr -d ' ')"
