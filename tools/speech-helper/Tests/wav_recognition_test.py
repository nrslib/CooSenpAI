#!/usr/bin/env python3
"""実 helper の WAV 入力・stdin finish・final を通して全文を検証する。"""

import argparse
from contextlib import redirect_stderr, redirect_stdout
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time
import traceback
import unicodedata


EXPECTED = "最初の文をここで話しますそして続きの文をここで話します"
SF_EXPECTED_FRAGMENT = "続きの文"


def normalize(text):
    return "".join(
        character for character in text
        if not character.isspace() and not unicodedata.category(character).startswith("P")
    )


def recognize(helper, wav, directory, duration, expected_frames, engine):
    directory.mkdir()
    process = subprocess.Popen(
        [str(helper), "--locale", "ja-JP", "--input-device", "default", "--debug-input-wav", str(wav), "--engine", engine],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True,
    )
    started = time.monotonic()
    ready_time = None
    finish_time = None
    final_time = None
    appended_frames = 0
    events = []
    buffers = {"stdout": b"", "stderr": b""}
    try:
        with selectors.DefaultSelector() as selector, \
                (directory / "stdout.jsonl").open("wb") as stdout, \
                (directory / "stderr.log").open("wb") as stderr, \
                (directory / "timeline.jsonl").open("w", encoding="utf-8") as timeline:
            outputs = {"stdout": stdout, "stderr": stderr}
            selector.register(process.stdout, selectors.EVENT_READ, "stdout")
            selector.register(process.stderr, selectors.EVENT_READ, "stderr")
            while selector.get_map():
                now = time.monotonic()
                deadline = finish_time + 32 if finish_time is not None else started + duration + 30
                if now >= deadline:
                    raise AssertionError("WAV 入力または finish 後の結果待ちが期限を超えました")
                for key, _ in selector.select(timeout=min(1, deadline - now)):
                    channel = key.data
                    chunk = os.read(key.fd, 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        if buffers[channel]:
                            raise AssertionError(f"{channel} が行の途中で終了しました")
                        continue
                    outputs[channel].write(chunk)
                    outputs[channel].flush()
                    buffers[channel] += chunk
                    while b"\n" in buffers[channel]:
                        line, buffers[channel] = buffers[channel].split(b"\n", 1)
                        observed = time.monotonic()
                        timeline.write(json.dumps({
                            "seconds": round(observed - started, 6),
                            "channel": channel, "line": line.decode("utf-8"),
                        }, ensure_ascii=False) + "\n")
                        timeline.flush()
                        if channel == "stderr":
                            if line.startswith(b"speech event=analysis-input-ended "):
                                fields = dict(field.split("=", 1) for field in line.decode("utf-8").split()[1:])
                                appended_frames += int(fields["frames"])
                            continue
                        event = json.loads(line)
                        events.append(event)
                        if event["event"] == "ready":
                            if ready_time is not None or event.get("input") != "wav":
                                raise AssertionError("WAV 入力の ready が一度だけ必要です")
                            expected_engine = "SFSpeechRecognizer" if engine == "sf" else "SpeechAnalyzer"
                            if event.get("engine") != expected_engine:
                                raise AssertionError(f"engine が一致しません: {event.get('engine')}")
                            ready_time = observed
                        elif event["event"] == "debug-input-ended":
                            if ready_time is None or finish_time is not None:
                                raise AssertionError("WAV 終端が重複しました")
                            if observed - ready_time < duration - 0.1:
                                raise AssertionError("WAV が実時間より速く投入されました")
                            if observed - ready_time > duration + 0.5:
                                raise AssertionError("WAV の投入に0.5秒を超える遅延が累積しました")
                            finish_time = time.monotonic()
                            process.stdin.write(b'{"op":"finish"}\n')
                            process.stdin.flush()
                            timeline.write(json.dumps({
                                "seconds": round(finish_time - started, 6),
                                "channel": "stdin", "line": '{"op":"finish"}',
                            }) + "\n")
                            timeline.flush()
                        elif event["event"] == "final":
                            if finish_time is None:
                                raise AssertionError("finish 前に final が送信されました")
                            final_time = observed
            process.wait(timeout=5)
        finals = [event["text"] for event in events if event["event"] == "final"]
        errors = [event for event in events if event["event"] == "error"]
        if process.returncode != 0 or errors:
            raise AssertionError(f"helper が失敗しました: exit={process.returncode}, errors={errors}")
        if sum(event["event"] == "ready" for event in events) != 1:
            raise AssertionError("ready は一度だけ必要です")
        if sum(event["event"] == "closed" for event in events) != 1:
            raise AssertionError("closed は一度だけ必要です")
        if appended_frames != expected_frames:
            raise AssertionError(f"認識器へ渡した PCM が一致しません: {appended_frames}/{expected_frames} frames")
        if len(finals) != 1:
            raise AssertionError(f"final は一度だけ必要です: {finals!r}")
        recognized = normalize(finals[0])
        if engine == "analyzer" and recognized != EXPECTED:
            raise AssertionError(f"全文が一致しません: final={finals!r}, PCM={appended_frames}/{expected_frames} frames")
        if engine == "sf" and SF_EXPECTED_FRAGMENT not in recognized:
            raise AssertionError(f"SFSpeechRecognizer が後半の発話を認識していません: {finals!r}")
        if final_time - finish_time > 30:
            raise AssertionError("final が finish から30秒を超えました")
        return {
            "engine": engine, "prefix_lost": "最初の文" not in recognized,
            "final": finals[0], "input_seconds": round(finish_time - ready_time, 3),
            "finish_to_final_ms": round((final_time - finish_time) * 1000, 1),
            "appended_frames": appended_frames,
        }
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()


def parse_arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--wav", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, required=True)
    parser.add_argument("--engine", choices=["analyzer", "sf"], required=True)
    arguments = parser.parse_args()
    if arguments.runs < 1:
        parser.error("--runs は1以上で指定してください")
    return arguments


def main(arguments):
    probe = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries",
        "stream=codec_name,sample_rate,channels,duration_ts,time_base", "-of", "json", str(arguments.wav),
    ]))["streams"][0]
    sample_rate = int(probe["sample_rate"])
    duration = float(probe["duration_ts"] * Fraction(probe["time_base"]))
    metadata = {
        "frames": round(duration * sample_rate), "sample_rate": sample_rate,
        "channels": probe["channels"], "codec": probe["codec_name"], "duration_seconds": duration,
        "sha256": hashlib.sha256(arguments.wav.read_bytes()).hexdigest(),
        "expected": EXPECTED if arguments.engine == "analyzer" else SF_EXPECTED_FRAGMENT,
        "runs": arguments.runs, "engine": arguments.engine,
    }
    (arguments.output / "input.json").write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    results = []
    print(f"WAV 実認識: {duration:.3f} 秒、{arguments.runs} 回連続、ログ: {arguments.output}", flush=True)
    for index in range(1, arguments.runs + 1):
        directory = arguments.output / f"run-{index}"
        try:
            result = recognize(arguments.helper.resolve(), arguments.wav.resolve(), directory, duration, metadata["frames"], arguments.engine)
        except (AssertionError, OSError, ValueError, subprocess.TimeoutExpired) as error:
            print(f"FAIL WAV {index}/{arguments.runs}: {error}\nログ: {directory}", flush=True)
            return 1
        results.append(result)
        print(f"PASS WAV {index}/{arguments.runs}: {json.dumps(result, ensure_ascii=False)}", flush=True)
    (arguments.output / "results.json").write_text(
        json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return 0


def run():
    arguments = parse_arguments()
    output = arguments.output
    output.mkdir(parents=True, exist_ok=True)
    (output / "runner.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    status = 1
    with (output / "runner.log").open("w", encoding="utf-8", buffering=1) as log, \
            redirect_stdout(log), redirect_stderr(log):
        try:
            status = main(arguments)
        except SystemExit as error:
            status = error.code
        except Exception:
            traceback.print_exc()
        finally:
            (output / "exit-status").write_text(f"{status}\n", encoding="utf-8")
            (output / "runner.pid").unlink()
    return status


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda _signum, _frame: sys.exit(143))
    signal.signal(signal.SIGINT, lambda _signum, _frame: sys.exit(130))
    raise SystemExit(run())
