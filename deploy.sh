#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

wasm-pack build pilegen --release --target web

rsync -avzR \
  index.html \
  pilegen/pkg/pilegen.js \
  pilegen/pkg/pilegen_bg.wasm \
  bo@bur.io:/usr/local/share/site/static/pilegen/
