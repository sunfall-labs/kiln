#!/usr/bin/env bash
# Fetch (not build, where avoidable) the competitor toolchains for bench/realworld.
#
# Layout follows the repo's durable-cache rule (AGENTS.md): nothing under /tmp.
# porffor installs from npm into bench/realworld/.toolchains (gitignored);
# Hermes clones its static_h branch into ~/.cache/kiln and builds hermesc+hvm
# there, which takes a while -- the suite skips hermes gracefully until then.
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TOOLCHAINS="$HERE/.toolchains"
HERMES_DIR="${KILN_HERMES_CHECKOUT:-$HOME/.cache/kiln/hermes-static_h}"

mkdir -p "$TOOLCHAINS"
if [[ ! -x "$TOOLCHAINS/node_modules/.bin/porf" ]] || [[ ! -x "$TOOLCHAINS/node_modules/.bin/esbuild" ]]; then
  echo "installing porffor + esbuild into $TOOLCHAINS"
  npm i --prefix "$TOOLCHAINS" porffor@0.0.0-alpha.7 esbuild
else
  echo "porffor already present at $TOOLCHAINS"
fi
export PATH="$TOOLCHAINS/node_modules/.bin:$PATH"
command -v porf >/dev/null && porf --version 2>/dev/null || true

if [[ -x "$HERMES_DIR/build/bin/hermesc" ]]; then
  echo "hermes already built at $HERMES_DIR/build"
  echo "export HERMESC_BIN=$HERMES_DIR/build/bin/hermesc HVM_BIN=$HERMES_DIR/build/bin/hvm"
  exit 0
fi
if [[ ! -d "$HERMES_DIR" ]]; then
  echo "cloning hermes static_h branch into $HERMES_DIR"
  git clone --depth 1 --branch static_h https://github.com/facebook/hermes.git "$HERMES_DIR"
fi
# cmake comes from mise (global config) or PATH; the repo's .mise.toml is not
# touched for it. Ninja is optional -- Makefiles work, just slower.
if ! command -v cmake >/dev/null; then
  for candidate in "$HOME"/.local/share/mise/installs/cmake/*/cmake-*/bin/cmake; do
    if [[ -x $candidate ]]; then
      PATH="$(dirname -- "$candidate"):$PATH"
      break
    fi
  done
fi
command -v cmake >/dev/null || { echo "fetch-competitors: cmake not found" >&2; exit 1; }
GENERATOR="Unix Makefiles"
command -v ninja >/dev/null && GENERATOR="Ninja"
echo "building hermesc + hvm with $GENERATOR (this takes a while;" \
  "the suite skips hermes until done)"
cmake -S "$HERMES_DIR" -B "$HERMES_DIR/build" -G "$GENERATOR" -DCMAKE_BUILD_TYPE=Release
cmake --build "$HERMES_DIR/build" --target hermesc hvm -j "$(nproc)"
echo "export HERMESC_BIN=$HERMES_DIR/build/bin/hermesc HVM_BIN=$HERMES_DIR/build/bin/hvm"
