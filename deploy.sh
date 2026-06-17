#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

wasm-pack build crates/pilegen --release --target web

rsync -avzR \
  index.html \
  crates/pilegen/pkg/pilegen.js \
  crates/pilegen/pkg/pilegen_bg.wasm \
  bo@bur.io:/usr/local/share/site/static/pilegen/

# dd-goldfish frontend. `--no-default-features` drops the native `cli` set
# (clap/textplots/ureq) that doesn't build for wasm. `-R` preserves the
# crates/dd-goldfish/pkg path so goldfish.html's `./crates/.../dd_goldfish.js`
# import resolves both locally and on the server.
wasm-pack build crates/dd-goldfish --release --target web --no-default-features

rsync -avzR \
  dd-goldfish.html \
  crates/dd-goldfish/pkg/dd_goldfish.js \
  crates/dd-goldfish/pkg/dd_goldfish_bg.wasm \
  bo@bur.io:/usr/local/share/site/static/pilegen/
