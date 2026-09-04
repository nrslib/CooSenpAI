#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
output_dir="$repository_dir/target/helpers"
module_cache_dir="$output_dir/module-cache"
target_triple=${1:-}
case "$target_triple" in
  aarch64-apple-darwin) swift_target=arm64-apple-macosx13.0 ;;
  x86_64-apple-darwin) swift_target=x86_64-apple-macosx13.0 ;;
  *) swift_target=$target_triple ;;
esac
if [ -n "$target_triple" ]; then
  output_path="$output_dir/coosenpai-speech-$target_triple"
else
  output_path="$output_dir/coosenpai-speech"
fi
temporary_path="$output_dir/.$(basename "$output_path").$$"

mkdir -p "$module_cache_dir"
trap 'rm -f "$temporary_path"' EXIT HUP INT TERM
if [ -n "$target_triple" ]; then
  swiftc -target "$swift_target" -O -module-cache-path "$module_cache_dir" \
    "$script_dir/Sources/main.swift" \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
    -o "$temporary_path"
else
  swiftc -O -module-cache-path "$module_cache_dir" \
    "$script_dir/Sources/main.swift" \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
    -o "$temporary_path"
fi
chmod 755 "$temporary_path"
mv -f "$temporary_path" "$output_path"
trap - EXIT HUP INT TERM
printf '%s\n' "$output_path"
