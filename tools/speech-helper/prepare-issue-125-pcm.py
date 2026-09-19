#!/usr/bin/env python3
"""二つの Float32 PCM 発話を末尾無音なしで3秒の無音につなぐ。"""

import argparse
from array import array
import math
from pathlib import Path
import sys


SAMPLE_RATE = 48_000
GAP_FRAMES = 3 * SAMPLE_RATE


def read_trimmed(path):
    data = path.read_bytes()
    if len(data) % 4:
        raise ValueError(f"Float32 PCM の長さが不正です: {path}")
    samples = array("f")
    samples.frombytes(data)
    if sys.byteorder != "little":
        samples.byteswap()
    if not samples or not all(math.isfinite(sample) for sample in samples):
        raise ValueError(f"Float32 PCM のサンプルが不正です: {path}")
    first = next((index for index, sample in enumerate(samples) if sample != 0), None)
    last = next(
        (len(samples) - offset - 1 for offset, sample in enumerate(reversed(samples)) if sample != 0),
        None,
    )
    if first is None or last is None:
        raise ValueError(f"発話が完全な無音です: {path}")
    return samples[first:last + 1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    first = read_trimmed(arguments.first)
    second = read_trimmed(arguments.second)
    silence = array("f", [0]) * GAP_FRAMES
    arguments.output.write_bytes(first.tobytes() + silence.tobytes() + second.tobytes())
    print(f"first_frames={len(first)} gap_frames={GAP_FRAMES} second_frames={len(second)}")


if __name__ == "__main__":
    main()
