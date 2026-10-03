#!/usr/bin/env bash
#
# Builds and runs the example as a windowed game.
#
#   ./scripts/run-window.sh                 # play it
#   ./scripts/run-window.sh --mode hybrid   # with ray-traced shadows
#   ./scripts/run-window.sh --frames 300    # close after 300 frames
#
# The `window` feature is what pulls in the platform crates. It is off by
# default, so the engine's own build still downloads nothing.
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$(dirname "$0")/.."
exec cargo run --release -p town-demo --features window -- --window "$@"
