#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  printf '使い方: %s OUTPUT_DIRECTORY\n' "$0" >&2
  exit 2
fi

output_dir=$1
mkdir -p "$output_dir"
for command in say ffmpeg; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf '音声入力 WAV テスト: 必須コマンドがありません: %s\n' "$command" >&2
    exit 2
  fi
done

say -v Kyoko -r 150 -o "$output_dir/first.aiff" '最初の文をここで話します'
say -v Kyoko -r 150 -o "$output_dir/second.aiff" 'そして続きの文をここで話します'
ffmpeg -nostdin -hide_banner -loglevel error \
  -i "$output_dir/first.aiff" -i "$output_dir/second.aiff" \
  -filter_complex '[0:a]aresample=48000,aformat=channel_layouts=mono[first];[1:a]aresample=48000,aformat=channel_layouts=mono[second];anullsrc=r=48000:cl=mono,atrim=duration=3.5[gap];anullsrc=r=48000:cl=mono,atrim=duration=1.5[tail];[first][gap][second][tail]concat=n=4:v=0:a=1[out]' \
  -map '[out]' -c:a pcm_s16le -y "$output_dir/two-utterances.wav"
printf '%s\n' "$output_dir/two-utterances.wav"
