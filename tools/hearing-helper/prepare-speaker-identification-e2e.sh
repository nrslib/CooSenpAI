#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  printf '使い方: %s OUTPUT_DIRECTORY\n' "$0" >&2
  exit 2
fi

output_dir=$1
fixture_dir=${COOSENPAI_SPEAKER_FIXTURE_DIR:-"$HOME/work/data/models/speaker-id/fixtures"}

for command in python3 sw_vers; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf '話者 ID E2E: 必須コマンドがありません: %s\n' "$command" >&2
    exit 2
  fi
done

if [ ! -d "$fixture_dir" ] || [ ! -f "$fixture_dir/speaker-a.wav" ] || [ ! -f "$fixture_dir/speaker-b.wav" ]; then
  printf '話者 ID E2E: fixture がありません: %s/speaker-a.wav, %s/speaker-b.wav\n' \
    "$fixture_dir" "$fixture_dir" >&2
  exit 2
fi

mkdir -p "$output_dir"
python3 - "$output_dir" "$fixture_dir" <<'PY'
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import wave

output_dir = Path(sys.argv[1])
fixture_dir = Path(sys.argv[2])
rate = 16_000
clip_seconds = 6


def read_mono(path):
    with wave.open(str(path), "rb") as stream:
        params = stream.getparams()
        if (
            params.nchannels,
            params.sampwidth,
            params.framerate,
            params.comptype,
        ) != (1, 2, rate, "NONE"):
            raise SystemExit(f"fixture WAV の形式が不正です: {path}")
        data = stream.readframes(params.nframes)
        if params.nframes < rate * 10:
            raise SystemExit(f"fixture WAV が10秒未満です: {path}")
        return list(struct.unpack("<" + "h" * params.nframes, data))


source_a = read_mono(fixture_dir / "speaker-a.wav")
source_b = read_mono(fixture_dir / "speaker-b.wav")
clips = {
    "A": source_a[: rate * clip_seconds],
    "B": source_b[: rate * clip_seconds],
}
short = clips["A"][: int(rate * 1.5)]
silence = [0] * int(rate * 1.5)


def append_segment(sequence, manifest, samples, label):
    start = sum(len(part["samples"]) for part in sequence)
    sequence.append({"name": label, "label": label, "samples": samples})
    end = start + len(samples)
    manifest.append(
        {
            "name": label,
            "label": label,
            "startMs": round(start * 1000 / rate),
            "endMs": round(end * 1000 / rate),
            "sampleCount": len(samples),
        }
    )


def add_gap(sequence):
    sequence.append({"name": "silence", "label": "silence", "samples": silence})


def make_first_sequence():
    sequence = []
    manifest = []
    append_segment(sequence, manifest, short, "short")
    add_gap(sequence)
    append_segment(sequence, manifest, clips["A"], "registration-A")
    add_gap(sequence)
    append_segment(sequence, manifest, clips["B"], "registration-B")
    add_gap(sequence)
    for index, label in enumerate(("A", "B", "A", "B", "A", "B"), start=1):
        append_segment(sequence, manifest, clips[label], f"evaluation-{index}-{label}")
        add_gap(sequence)
    append_segment(sequence, manifest, clips["A"] + clips["B"], "mixed")
    add_gap(sequence)
    return sequence, manifest


def make_restart_sequence():
    sequence = []
    manifest = []
    for index, label in enumerate(("A", "B", "A", "B", "A", "B"), start=1):
        append_segment(sequence, manifest, clips[label], f"restart-{index}-{label}")
        add_gap(sequence)
    return sequence, manifest


def write_stereo(path, sequence):
    samples = [sample for part in sequence for sample in part["samples"]]
    with wave.open(str(path), "wb") as output:
        output.setnchannels(2)
        output.setsampwidth(2)
        output.setframerate(rate)
        output.writeframes(
            b"".join(struct.pack("<hh", sample, sample) for sample in samples)
        )


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


first_sequence, first_segments = make_first_sequence()
restart_sequence, restart_segments = make_restart_sequence()
first_wav = output_dir / "speaker-identification.wav"
restart_wav = output_dir / "speaker-identification-restart.wav"
write_stereo(first_wav, first_sequence)
write_stereo(restart_wav, restart_sequence)

manifest = {
    "schemaVersion": 2,
    "fixture": "external-real-speaker-wav",
    "os": subprocess.check_output(["sw_vers", "-productVersion"], text=True).strip(),
    "generationCommand": "prepare-speaker-identification-e2e.sh が公開実声 fixture を読み、無音区間を挿入して stereo WAV を生成",
    "minimumRegistrationSeconds": 4.0,
    "minimumEvaluationSeconds": 2.0,
    "evaluationOrder": ["A", "B", "A", "B", "A", "B"],
    "shortBeforeRegistrationStatus": "unknown",
    "mixedAllowedStatuses": ["unknown", "mixed"],
    "restartRequiresSameRegistry": True,
    "leakageSentinel": "COOSENPAI_SPEAKER_ID_INTERNAL_SENTINEL",
    "files": {
        "speaker-identification.wav": {
            "sha256": sha256(first_wav),
            "channels": 2,
            "sampleRate": rate,
        },
        "speaker-identification-restart.wav": {
            "sha256": sha256(restart_wav),
            "channels": 2,
            "sampleRate": rate,
        },
    },
    "segments": first_segments,
    "restartSegments": restart_segments,
}
(output_dir / "speaker-identification-manifest.json").write_text(
    json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)
print(first_wav)
PY
