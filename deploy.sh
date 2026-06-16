#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

wasm-pack build crates/pilegen --release --target web

rsync -avzR \
  index.html \
  crates/pilegen/pkg/pilegen.js \
  crates/pilegen/pkg/pilegen_bg.wasm \
  bo@bur.io:/usr/local/share/site/static/pilegen/
