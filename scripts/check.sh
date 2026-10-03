#!/usr/bin/env bash
#
# The full local quality gate. CI runs the same steps (.github/workflows/ci.yml).
#
#   ./scripts/check.sh          # format, lint, test, docs, assets
#   ./scripts/check.sh --fast   # skip the release build and the demo run
#
set -euo pipefail

# This machine keeps the toolchain outside the default PATH.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$(dirname "$0")/.."

step() { printf '\n\033[1m== %s\033[0m\n' "$1"; }

step "format"
cargo fmt --all --check

step "clippy (warnings are errors)"
cargo clippy --workspace --all-targets -- -D warnings

step "test"
cargo test --workspace

step "documentation"
cargo doc --workspace --no-deps

step "the window feature still builds"
# Not part of the default build: this is the one place that compiles winit.
cargo check -p town-demo --features window

step "generated assets are up to date"
cargo run --release -q -p noxel-gen -- verify

if [ "${1:-}" != "--fast" ]; then
    step "headless demo"
    out="$(mktemp -d)"
    cargo run --release -q -p town-demo -- --frames 30 --dump "$out" --quiet
    ls "$out" | tail -1
    rm -rf "$out"
fi

printf '\n\033[1;32mall checks passed\033[0m\n'
