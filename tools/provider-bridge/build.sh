#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$script_dir"
npm ci
npm run build
chmod 755 dist/bridge.js
printf '%s\n' "$script_dir/dist/bridge.js"
