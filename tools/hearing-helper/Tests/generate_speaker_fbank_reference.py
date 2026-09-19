#!/usr/bin/env python3
"""WeSpeaker の公式 fbank 条件で固定 WAV の参照値を生成する。"""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import wave

import numpy as np
import onnxruntime as ort
import torch
import torchaudio
import torchaudio.compliance.kaldi as kaldi


TARGET_SAMPLE_RATE = 16_000
WINDOW_DURATION_SECONDS = 2.015
WINDOW_SAMPLE_COUNT = 32_240
PREPROCESSING_VERSION = "wespeaker-kaldi-fbank-snip-edges-true-200fr-2015ms-resample-trim-v4"


def make_pcm16(sample_count: int) -> list[int]:
    state = 0x1234_5678
    samples = []
    for _ in range(sample_count):
        state = (1_664_525 * state + 1_013_904_223) & 0xFFFF_FFFF
        value = ((state / 4_294_967_296.0) * 2.0 - 1.0) * 0.2 * 32_768.0
        samples.append(int(round(value)))
    return samples


def write_wav(path: Path, samples: list[int], sample_rate: int) -> None:
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.setcomptype("NONE", "not compressed")
        output.writeframes(struct.pack("<" + "h" * len(samples), *samples))


def load_wav(path: Path) -> tuple[torch.Tensor, int]:
    with wave.open(str(path), "rb") as input_file:
        if (
            input_file.getnchannels(),
            input_file.getsampwidth(),
            input_file.getcomptype(),
        ) != (1, 2, "NONE"):
            raise SystemExit("参照 WAV の形式が不正です")
        samples = struct.unpack(
            "<" + "h" * input_file.getnframes(), input_file.readframes(input_file.getnframes())
        )
        waveform = torch.tensor(samples, dtype=torch.float32).reshape(1, -1) / (1 << 15)
        return waveform, input_file.getframerate()


def make_reference(
    wav_path: Path,
    onnx_path: Path,
    samples: list[int],
    source_sample_rate: int,
) -> dict:
    # torchaudio の WAV 読み込みと同じ int16 -> [-1, 1) の尺度を、
    # 環境ごとの音声バックエンドに依存せず固定する。
    waveform, sample_rate = load_wav(wav_path)
    if sample_rate != source_sample_rate:
        raise SystemExit(f"参照 WAV のサンプルレートが不正です: {sample_rate}")
    waveform = waveform * (1 << 15)
    if sample_rate != TARGET_SAMPLE_RATE:
        waveform = torchaudio.transforms.Resample(
            sample_rate,
            TARGET_SAMPLE_RATE,
        )(waveform)
    if waveform.shape[-1] < WINDOW_SAMPLE_COUNT:
        raise SystemExit(
            f"リサンプル後の参照 WAV が短すぎます: {waveform.shape[-1]} samples"
        )
    # Swift helper と同じく、比率の丸めで増えた末尾の1サンプルを使わない。
    waveform = waveform[..., :WINDOW_SAMPLE_COUNT]
    features = kaldi.fbank(
        waveform,
        num_mel_bins=80,
        frame_length=25,
        frame_shift=10,
        dither=0.0,
        sample_frequency=TARGET_SAMPLE_RATE,
        snip_edges=True,
        window_type="hamming",
        use_energy=False,
    )
    features = features - torch.mean(features, dim=0)
    if tuple(features.shape) != (200, 80):
        raise SystemExit(f"参照 fbank の形状が不正です: {tuple(features.shape)}")

    session = ort.InferenceSession(
        str(onnx_path),
        sess_options=ort.SessionOptions(),
        providers=["CPUExecutionProvider"],
    )
    output = session.run(
        output_names=["embs"],
        input_feed={"feats": features.unsqueeze(0).numpy()},
    )[0]
    embedding = np.asarray(output[0], dtype=np.float32)
    norm = float(np.linalg.norm(embedding))
    if not np.isfinite(norm) or norm <= 0:
        raise SystemExit("参照埋め込みのノルムが不正です")
    embedding = embedding / norm
    wav_bytes = wav_path.read_bytes()
    return {
        "schemaVersion": 2,
        "preprocessingVersion": PREPROCESSING_VERSION,
        "sampleRate": source_sample_rate,
        "channels": 1,
        "sampleCount": len(samples),
        "wavSha256": hashlib.sha256(wav_bytes).hexdigest(),
        "pcm16": samples,
        "features": features.numpy().tolist(),
        "normalizedEmbedding": embedding.tolist(),
        "conditions": {
            "frameLengthMs": 25,
            "frameShiftMs": 10,
            "snipEdges": True,
            "dither": 0.0,
            "removeDcOffset": True,
            "preemphasisCoefficient": 0.97,
            "roundToPowerOfTwo": True,
            "windowType": "hamming",
            "numMelBins": 80,
            "cmn": "mean per mel bin",
            "targetSampleRate": TARGET_SAMPLE_RATE,
            "inputWindowSampleCount": WINDOW_SAMPLE_COUNT,
            "trimExtraResampledSamples": True,
            "resample": "torchaudio.transforms.Resample default sinc_interp_hann",
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--onnx", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wav-output", type=Path)
    parser.add_argument("--sample-rate", type=int, default=TARGET_SAMPLE_RATE)
    args = parser.parse_args()
    if args.sample_rate <= 0:
        raise SystemExit("サンプルレートは正数で指定してください")
    sample_count = round(WINDOW_DURATION_SECONDS * args.sample_rate)
    samples = make_pcm16(sample_count)
    with tempfile.TemporaryDirectory(prefix="coosenpai-speaker-fbank-") as temporary:
        temporary_wav = Path(temporary) / "golden.wav"
        write_wav(temporary_wav, samples, args.sample_rate)
        if args.wav_output is not None:
            args.wav_output.parent.mkdir(parents=True, exist_ok=True)
            args.wav_output.write_bytes(temporary_wav.read_bytes())
        reference = make_reference(
            temporary_wav,
            args.onnx,
            samples,
            args.sample_rate,
        )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(reference, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(args.output)


if __name__ == "__main__":
    main()
