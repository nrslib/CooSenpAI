#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  printf '使い方: %s OUTPUT_WAV\n' "$0" >&2
  exit 2
fi

output_path=$1
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
output_dir=$(dirname -- "$output_path")
mkdir -p "$output_dir"
for command in say ffmpeg python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Issue #125 fixture: 必須コマンドがありません: %s\n' "$command" >&2
    exit 2
  fi
done

temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/coosenpai-issue-125-wav.XXXXXX")
cleanup() {
  rm -rf "$temporary_dir"
}
trap cleanup EXIT HUP INT TERM

say -v Kyoko -r 100 -o "$temporary_dir/first.aiff" 'テステスマイクテス'
say -v Kyoko -r 100 -o "$temporary_dir/second.aiff" 'テステスマイクテス'
ffmpeg -nostdin -hide_banner -loglevel error \
  -i "$temporary_dir/first.aiff" -af 'volume=8,aresample=48000' \
  -ar 48000 -ac 1 -f f32le -c:a pcm_f32le -y "$temporary_dir/first.f32"
ffmpeg -nostdin -hide_banner -loglevel error \
  -i "$temporary_dir/second.aiff" -af 'volume=8,aresample=48000' \
  -ar 48000 -ac 1 -f f32le -c:a pcm_f32le -y "$temporary_dir/second.f32"
python3 "$script_dir/prepare-issue-125-pcm.py" \
  "$temporary_dir/first.f32" "$temporary_dir/second.f32" "$temporary_dir/combined.f32"
ffmpeg -nostdin -hide_banner -loglevel error \
  -f f32le -ar 48000 -ac 1 -i "$temporary_dir/combined.f32" \
  -c:a pcm_f32le -y "$output_path"
printf '%s\n' "$output_path"
