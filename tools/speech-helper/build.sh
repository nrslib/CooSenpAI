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
  output_path="$output_dir/coosenpai-speech-$target_triple"
else
  output_path="$output_dir/coosenpai-speech"
fi
temporary_path="$output_dir/.$(basename "$output_path").$$"

mkdir -p "$module_cache_dir"
trap 'rm -f "$temporary_path"' EXIT HUP INT TERM
swiftc -target "$swift_target" -O -module-cache-path "$module_cache_dir" \
  "$script_dir/../hearing-helper/Sources/audio_buffer_copy.swift" \
  "$script_dir/../hearing-helper/Sources/audio_stats.swift" \
  "$script_dir/../hearing-helper/Sources/wav_input.swift" \
  "$script_dir/Sources/audio_queue.swift" \
  "$script_dir/Sources/wav_dump.swift" \
  "$script_dir/Sources/wav_file.swift" \
  "$script_dir/Sources/transcript_accumulator.swift" \
  "$script_dir/Sources/recognition_session.swift" \
  "$script_dir/Sources/audio_converter.swift" \
  "$script_dir/Sources/speech_analyzer.swift" \
  "$script_dir/Sources/speech_recognizer.swift" \
  "$script_dir/Sources/speech_engine.swift" \
  "$script_dir/Sources/main.swift" \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$script_dir/Info.plist" \
  -o "$temporary_path"
build_version=$(xcrun vtool -show-build "$temporary_path")
actual_deployment_target=$(printf '%s\n' "$build_version" | awk '$1 == "minos" { print $2 }')
if [ "$actual_deployment_target" != "$deployment_target" ]; then
  printf 'speech-helper の最低 OS が一致しません: expected=%s actual=%s\n' \
    "$deployment_target" "$actual_deployment_target" >&2
  exit 1
fi
chmod 755 "$temporary_path"
mv -f "$temporary_path" "$output_path"
trap - EXIT HUP INT TERM
printf '%s\n' "$output_path"
