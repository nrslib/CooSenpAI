#!/usr/bin/env python3
"""実機採取の mic 区間2・3を、区間1の実ノイズでつないで検証 WAV にする。"""

import argparse
import array
import hashlib
import json
import math
from pathlib import Path
import subprocess


def read_segment(path):
    source = path.read_bytes()
    metadata = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries",
        "stream=codec_name,sample_rate,channels", "-of", "json", "-i", "pipe:0",
    ], input=source))["streams"][0]
    if metadata != {"codec_name": "pcm_f32le", "sample_rate": "48000", "channels": 1}:
        raise ValueError(f"この検証録音は 48 kHz / mono / Float32 が必要です: {path.name}")
    pcm = subprocess.check_output([
        "ffmpeg", "-nostdin", "-v", "error", "-f", "wav", "-i", "pipe:0",
        "-f", "f32le", "-c:a", "pcm_f32le", "pipe:1",
    ], input=source)
    samples = array.array("f", pcm)
    if not samples or not all(math.isfinite(sample) for sample in samples):
        raise ValueError(f"PCM のサンプルが不正です: {path.name}")
    return pcm, {"file": path.name, "sha256": hashlib.sha256(source).hexdigest(), "frames": len(samples)}


def rms_dbfs(pcm):
    samples = array.array("f", pcm)
    rms = math.sqrt(sum(sample * sample for sample in samples) / len(samples))
    if rms == 0:
        raise ValueError("実ノイズの代わりに完全無音が含まれています")
    return 20 * math.log10(rms)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    arguments = parser.parse_args()
    output = arguments.output_directory
    output.mkdir(parents=True, exist_ok=True)
    sources = [read_segment(arguments.source_directory / f"segment-microphone-{index}.wav") for index in (1, 2, 3)]
    sample_rate = 48_000
    bytes_per_frame = 4
    noise_frames = 5 * sample_rate
    if sources[0][1]["frames"] < noise_frames:
        raise ValueError("区間1の環境音が5秒未満です")
    noise = sources[0][0][-noise_frames * bytes_per_frame:]
    gap_frames = int(3.5 * sample_rate)
    gap, tail = noise[:gap_frames * bytes_per_frame], noise[gap_frames * bytes_per_frame:]
    noise_level = rms_dbfs(noise)
    if noise_level >= -40:
        raise ValueError("区間1の末尾5秒を静かな環境音として使用できません")
    # VAD が残した発話の前後も保持し、語尾を含む元の音声を切り落とさない。
    pcm = sources[1][0] + gap + sources[2][0] + tail
    wav = output / "two-utterances-microphone.wav"
    subprocess.run([
        "ffmpeg", "-nostdin", "-v", "error", "-f", "f32le", "-ar", str(sample_rate),
        "-ac", "1", "-i", "pipe:0", "-c:a", "pcm_f32le", "-n", str(wav),
    ], input=pcm, check=True)
    manifest = {
        "sources": [metadata for _, metadata in sources],
        "sample_rate": sample_rate, "channels": 1, "codec": "pcm_f32le",
        "parts": [
            {"source": sources[1][1]["file"], "start_frame": 0, "frames": sources[1][1]["frames"]},
            {"source": sources[0][1]["file"], "start_frame": sources[0][1]["frames"] - noise_frames, "frames": gap_frames},
            {"source": sources[2][1]["file"], "start_frame": 0, "frames": sources[2][1]["frames"]},
            {"source": sources[0][1]["file"], "start_frame": sources[0][1]["frames"] - noise_frames + gap_frames, "frames": noise_frames - gap_frames},
        ],
        "gap_seconds": 3.5, "tail_seconds": 1.5, "noise_rms_dbfs": noise_level,
        "frames": len(pcm) // bytes_per_frame, "duration_seconds": len(pcm) / bytes_per_frame / sample_rate,
        "pcm_sha256": hashlib.sha256(pcm).hexdigest(), "wav_sha256": hashlib.sha256(wav.read_bytes()).hexdigest(),
        "note": "区間2・3は全フレームを保持。元からある前後の環境音に加えて3.5秒・1.5秒を挿入。元の連続録音の時系列復元ではない。",
    }
    (output / "microphone-replay.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(wav.resolve())


if __name__ == "__main__":
    main()
