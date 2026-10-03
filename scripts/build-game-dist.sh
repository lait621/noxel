#!/usr/bin/env bash
#
# Builds the playable game: a relocatable `dist/noxel-valley/` directory and, on
# macOS, a double-clickable `dist/Noxel Valley.app`.
#
#   ./scripts/build-game-dist.sh            # release, into dist/
#   ./scripts/build-game-dist.sh --debug    # faster to build, slower to run
#
# Both outputs are self-contained. They need no Rust toolchain, no source tree
# and no network: copy either anywhere and it runs.
#
# The `window` feature is what makes the game a game rather than a frame
# renderer. It is off by default so the engine's own `cargo build` still
# downloads nothing (docs/adr/0002-no-dependencies.md), which is why this script
# turns it on explicitly.
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

TITLE="星野农场 · Noxel Valley"

printf '\n== building (%s, window feature on)\n' "$profile"
cargo build $flag -p noxel-valley --features window -p noxel-gen

bin="$root/target/$profile/noxel-valley"
out="$root/dist/noxel-valley"
assets="$root/games/noxel-valley/assets"

if [ ! -f "$bin" ]; then
    printf 'build produced no binary at %s\n' "$bin" >&2
    exit 1
fi
if [ ! -d "$assets/farm" ]; then
    printf '\n== generating the game art (not built yet)\n'
    cargo run $flag -q -p noxel-gen -- farm --out "$assets"
fi

# ---------------------------------------------------------------------------
# dist/noxel-valley — the relocatable directory
# ---------------------------------------------------------------------------
printf '\n== assembling %s\n' "$out"
rm -rf "$out"
mkdir -p "$out"
cp "$bin" "$out/noxel-valley"
# The asset generator comes along so the bundle can regenerate its own art.
# It reads nothing and needs no toolchain.
if [ -x "$root/target/$profile/noxel-gen" ]; then
    cp "$root/target/$profile/noxel-gen" "$out/noxel-gen"
fi
# The assets sit beside the binary, which is the first place the game looks.
# That is what makes the directory relocatable.
cp -R "$assets" "$out/assets"

cat > "$out/README.txt" <<'TXT'
Noxel Valley — 星野农场
========================

This directory is self-contained. Nothing here needs Rust, the source tree, or
a network connection.

Play it:

    ./noxel-valley              # opens a window, with no arguments

Render frames instead (headless, no window):

    ./noxel-valley --frames 300 --dump frames
    ./noxel-valley --stats            # one frame, the full statistics report
    ./noxel-valley --help             # every option

Controls:

    WASD / arrows   walk                 Shift   run
    Space / LMB     use the held tool    1-6     select a hotbar slot
    Tab             bag                  E       shop, shipping bin, or sleep
    ?               controls             Esc     close / quit

The loop: hoe the soil, plant a seed, water it every day, harvest when it is
ripe, and carry the crop to the shipping bin. It sells overnight. Rain waters
the whole farm for you. Crops die when their season ends.

Regenerate the art in ./assets from code:

    ./noxel-gen farm --out assets         # writes ./assets/farm/*.png + *.json
TXT

# ---------------------------------------------------------------------------
# dist/Noxel Valley.app — the double-clickable macOS bundle
# ---------------------------------------------------------------------------
if [ "$(uname -s)" = "Darwin" ]; then
    app="$root/dist/Noxel Valley.app"
    printf '\n== assembling %s\n' "$app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

    cp "$bin" "$app/Contents/MacOS/noxel-valley"
    # `find_root` looks in `Contents/Resources/assets`, which is where a macOS
    # bundle is supposed to keep things it is not allowed to put beside the
    # executable.
    cp -R "$assets" "$app/Contents/Resources/assets"

    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Noxel Valley</string>
    <key>CFBundleDisplayName</key>
    <string>$TITLE</string>
    <key>CFBundleIdentifier</key>
    <string>dev.noxel.valley</string>
    <key>CFBundleExecutable</key>
    <string>noxel-valley</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

    # An unsigned bundle built by a script carries no quarantine attribute, but
    # clearing it anyway costs nothing and saves a "damaged app" dialog if the
    # directory is ever copied through something that adds one.
    xattr -cr "$app" 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# Verify both actually run, from somewhere unrelated
# ---------------------------------------------------------------------------
printf '\n== verifying\n'
( cd "$out" && ./noxel-valley --frames 3 --dump /tmp/noxel-valley-smoke >/dev/null \
    && echo "  runs from inside dist/noxel-valley/" )
work="$(mktemp -d)"
( cd "$work" && "$out/noxel-valley" --frames 3 --dump "$work/frames" >/dev/null \
    && echo "  runs from an unrelated directory" )
rm -rf "$work" /tmp/noxel-valley-smoke

printf '\n== done\n'
ls -lh "$out" | tail -n +2 | awk '{printf "   %-16s %s\n", $9, $5}'
du -sh "$out/assets" | awk '{printf "   assets           %s\n", $1}'
if [ "$(uname -s)" = "Darwin" ]; then
    printf '   %s\n' "$root/dist/Noxel Valley.app  (double-click it)"
fi
