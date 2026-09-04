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
  output_path="$output_dir/coosenpai-hearing-$target_triple"
else
  output_path="$output_dir/coosenpai-hearing"
fi
temporary_path="$output_dir/.$(basename "$output_path").$$"
temporary_object_path="$output_dir/.$(basename "$output_path").$$.o"

mkdir -p "$module_cache_dir"
trap 'rm -f "$temporary_path" "$temporary_object_path"' EXIT HUP INT TERM
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
if [ -n "$target_triple" ]; then
  clang -target "$swift_target" -isysroot "$sdk_path" -fobjc-arc -c \
    "$script_dir/Sources/audio_tap_installer.m" -o "$temporary_object_path"
  swiftc -target "$swift_target" -O -module-cache-path "$module_cache_dir" \
    -import-objc-header "$script_dir/Sources/audio_tap_installer.h" \
    "$script_dir/Sources/audio_stats.swift" \
    "$script_dir/Sources/audio_scaling.swift" \
    "$script_dir/Sources/audio_buffer_copy.swift" \
    "$script_dir/Sources/audio_conversion.swift" \
    "$script_dir/Sources/recognition_state.swift" \
    "$script_dir/Sources/segment_controller.swift" \
    "$script_dir/Sources/voice_activity.swift" \
    "$script_dir/Sources/wav_input.swift" \
    "$script_dir/Sources/appended_audio_dump.swift" \
    "$script_dir/Sources/main.swift" \
    "$temporary_object_path" \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
    -o "$temporary_path"
else
  clang -isysroot "$sdk_path" -fobjc-arc -c \
    "$script_dir/Sources/audio_tap_installer.m" -o "$temporary_object_path"
  swiftc -O -module-cache-path "$module_cache_dir" \
    -import-objc-header "$script_dir/Sources/audio_tap_installer.h" \
    "$script_dir/Sources/audio_stats.swift" \
    "$script_dir/Sources/audio_scaling.swift" \
    "$script_dir/Sources/audio_buffer_copy.swift" \
    "$script_dir/Sources/audio_conversion.swift" \
    "$script_dir/Sources/recognition_state.swift" \
    "$script_dir/Sources/segment_controller.swift" \
    "$script_dir/Sources/voice_activity.swift" \
    "$script_dir/Sources/wav_input.swift" \
    "$script_dir/Sources/appended_audio_dump.swift" \
    "$script_dir/Sources/main.swift" \
    "$temporary_object_path" \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
    -o "$temporary_path"
fi
chmod 755 "$temporary_path"
mv -f "$temporary_path" "$output_path"
trap - EXIT HUP INT TERM
printf '%s\n' "$output_path"
