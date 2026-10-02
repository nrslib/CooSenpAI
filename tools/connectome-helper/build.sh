#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
source_dir="$repository_dir/third_party/habitua"
target_triple=${1:-$(rustc -vV | sed -n 's/^host: //p')}
target_dir=${CONNECTOME_CARGO_TARGET_DIR:-$repository_dir/target/connectome-cargo}
output_dir="$repository_dir/target/helpers"
mkdir -p "$output_dir"
cargo build --locked --release --manifest-path "$source_dir/Cargo.toml" \
  --target "$target_triple" --target-dir "$target_dir" \
  -p habitua-connectome --bin connectome-helper
source_binary="$target_dir/$target_triple/release/connectome-helper"
destination="$output_dir/coosenpai-connectome-$target_triple"
temporary="$destination.$$"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$source_binary" "$temporary"
chmod 755 "$temporary"
mv -f "$temporary" "$destination"
trap - EXIT HUP INT TERM
printf '%s\n' "$destination"
