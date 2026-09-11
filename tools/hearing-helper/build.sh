#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
output_dir="$repository_dir/target/helpers"
module_cache_dir="$output_dir/module-cache"
deployment_target=13.0
target_triple=${1:-}
case "$target_triple" in
  aarch64-apple-darwin) swift_target="arm64-apple-macosx$deployment_target" ;;
  x86_64-apple-darwin) swift_target="x86_64-apple-macosx$deployment_target" ;;
  '') swift_target="$(uname -m)-apple-macosx$deployment_target" ;;
  *) swift_target=$target_triple ;;
esac
if [ -n "$target_triple" ]; then
  output_path="$output_dir/coosenpai-hearing-$target_triple"
else
  output_path="$output_dir/coosenpai-hearing"
fi
entrypoint="$script_dir/Sources/entrypoint.swift"
if [ "${2:-}" = "--test-speaker-failure" ]; then
  entrypoint="$script_dir/Tests/speaker_failure_entrypoint.swift"
  output_path="$output_path-failure-test"
elif [ -n "${2:-}" ]; then
  printf 'Unknown build option: %s\n' "$2" >&2
  exit 2
fi
temporary_path="$output_dir/.$(basename "$output_path").$$"
temporary_object_path="$output_dir/.$(basename "$output_path").$$.o"
temporary_ring_object_path="$output_dir/.$(basename "$output_path").$$.ring.o"

mkdir -p "$module_cache_dir"
trap 'rm -f "$temporary_path" "$temporary_object_path" "$temporary_ring_object_path"' EXIT HUP INT TERM
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
clang -target "$swift_target" -isysroot "$sdk_path" -fobjc-arc -c \
  "$script_dir/Sources/audio_tap_installer.m" -o "$temporary_object_path"
clang -target "$swift_target" -isysroot "$sdk_path" -std=c11 -O2 -c \
  "$script_dir/Sources/speaker_audio_ring.c" -o "$temporary_ring_object_path"
swiftc -target "$swift_target" -O -parse-as-library -module-cache-path "$module_cache_dir" \
  -import-objc-header "$script_dir/Sources/audio_tap_installer.h" \
  "$script_dir/Sources/speaker_audio_tap.swift" \
  "$script_dir/Sources/speaker_backend.swift" \
  "$script_dir/Sources/speaker_screen_capture.swift" \
  "$script_dir/Sources/speaker_screen_device.swift" \
  "$script_dir/Sources/speaker_audio_device.swift" \
  "$script_dir/Sources/audio_stats.swift" \
  "$script_dir/Sources/audio_scaling.swift" \
  "$script_dir/Sources/audio_buffer_copy.swift" \
  "$script_dir/Sources/audio_conversion.swift" \
  "$script_dir/Sources/audio_input_processing.swift" \
  "$script_dir/Sources/music_gate.swift" \
  "$script_dir/Sources/microphone_input_recovery.swift" \
  "$script_dir/Sources/recognition_state.swift" \
  "$script_dir/Sources/segment_controller.swift" \
  "$script_dir/Sources/voice_activity.swift" \
  "$script_dir/Sources/wav_input.swift" \
  "$script_dir/Sources/appended_audio_dump.swift" \
  "$script_dir/Sources/main.swift" \
  "$entrypoint" \
  "$temporary_object_path" "$temporary_ring_object_path" \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
  -o "$temporary_path"
build_version=$(xcrun vtool -show-build "$temporary_path")
actual_deployment_target=$(printf '%s\n' "$build_version" | awk '$1 == "minos" { print $2 }')
if [ "$actual_deployment_target" != "$deployment_target" ]; then
  printf 'helper の最低 OS が一致しません: expected=%s actual=%s\n' \
    "$deployment_target" "$actual_deployment_target" >&2
  exit 1
fi
chmod 755 "$temporary_path"
mv -f "$temporary_path" "$output_path"
trap - EXIT HUP INT TERM
printf '%s\n' "$output_path"
