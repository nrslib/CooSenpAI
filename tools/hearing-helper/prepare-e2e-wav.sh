#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  printf '使い方: %s OUTPUT_DIRECTORY\n' "$0" >&2
  exit 2
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
output_dir=$1
mkdir -p "$output_dir"

for command in say afconvert python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'WAV E2E: 必須コマンドがありません: %s\n' "$command" >&2
    exit 2
  fi
done

first_aiff="$output_dir/input.aiff"
second_aiff="$output_dir/input-b.aiff"
first_wav="$output_dir/input.wav"
second_wav="$output_dir/input-b.wav"
output_wav="$output_dir/input-two-stereo.wav"

say -v Kyoko -o "$first_aiff" 'これはアルファの発話です'
say -v Kyoko -o "$second_aiff" 'これはベータの発話です'
afconvert -f WAVE -d LEI16@48000 -c 1 "$first_aiff" "$first_wav"
afconvert -f WAVE -d LEI16@48000 -c 1 "$second_aiff" "$second_wav"
python3 - "$first_wav" "$second_wav" "$output_wav" <<'PY'
import sys
import wave

first_path, second_path, output_path = sys.argv[1:]
with wave.open(first_path, "rb") as first, wave.open(second_path, "rb") as second:
    first_params = first.getparams()
    second_params = second.getparams()
    if (
        first_params.nchannels,
        first_params.sampwidth,
        first_params.framerate,
        first_params.comptype,
    ) != (
        second_params.nchannels,
        second_params.sampwidth,
        second_params.framerate,
        second_params.comptype,
    ):
        raise SystemExit("二連発話 WAV のフォーマットが一致しません")
    if first_params.nchannels != 1:
        raise SystemExit("二連発話 WAV の入力は mono である必要があります")
    first_frames = first.readframes(first_params.nframes)
    second_frames = second.readframes(second_params.nframes)

def stereoize(frames, sample_width):
    return b"".join(
        frames[offset:offset + sample_width] * 2
        for offset in range(0, len(frames), sample_width)
    )

silence_frames = int(first_params.framerate * 1.5)
silence = b"\0" * silence_frames * 2 * first_params.sampwidth
with wave.open(output_path, "wb") as output:
    output.setnchannels(2)
    output.setsampwidth(first_params.sampwidth)
    output.setframerate(first_params.framerate)
    output.writeframes(
        stereoize(first_frames, first_params.sampwidth)
        + silence
        + stereoize(second_frames, first_params.sampwidth)
        + silence
    )
PY

printf '%s\n' "$output_wav"
