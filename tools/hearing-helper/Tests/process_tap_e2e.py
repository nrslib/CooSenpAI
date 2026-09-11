"""Verify real speaker capture, segmentation, and completed cleanup for each backend."""
import json
import atexit
import os
import shutil
from pathlib import Path
import subprocess
import sys
import time

root = Path(sys.argv[1])
scenario = sys.argv[2] if len(sys.argv) > 2 else "normal"
# Initial interactive permission grants are separate from the normal 8-second startup check.
ready_timeout = 60 if len(sys.argv) > 3 and sys.argv[3] == "1" else 8
backend = sys.argv[4] if len(sys.argv) > 4 else "process-tap"
cycle = sys.argv[5] if len(sys.argv) > 5 else "1"
assert backend in ("process-tap", "screen-capture-kit"), backend


def preserve_artifacts():
    destination = root.parent / f"{backend}-{scenario}-{cycle}"
    destination.mkdir(exist_ok=True)
    for name in ("stdout", "stderr", "exit", "input-two-stereo.wav"):
        if (root / name).exists():
            shutil.copyfile(root / name, destination / name)
    if (root / "dump").exists():
        shutil.copytree(root / "dump", destination / "dump", dirs_exist_ok=True)


atexit.register(preserve_artifacts)


def send_cancel():
    descriptor = os.open(root / "stdin", os.O_WRONLY | os.O_NONBLOCK)
    try:
        os.write(descriptor, b'{"op":"cancel"}\n')
    finally:
        os.close(descriptor)


def events():
    path = root / "stdout"
    if not path.exists():
        return []
    # The helper flushes complete NDJSON lines; ignore an in-progress final line.
    lines = path.read_text().splitlines(keepends=True)
    result = [json.loads(line) for line in lines if line.endswith("\n")]
    errors = [event for event in result if event.get("event") == "error"]
    if errors and scenario in ("normal", "recovered"):
        raise SystemExit(f"Speaker E2E backend={backend} error: {errors}")
    return result


def wait_until(condition, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.02)
    for name in ("stdout", "stderr"):
        if (root / name).exists():
            shutil.copyfile(root / name, root.parent / f"failed-{backend}-{scenario}-{name}")
    raise SystemExit(f"Speaker E2E backend={backend} timeout: {description}; artifacts: {root.parent}")


if scenario not in ("normal", "recovered"):
    wait_until(lambda: any(e.get("event") == "ready" for e in events()), ready_timeout, "microphone ready")
    wait_until(lambda: any(e.get("event") == "final" and e.get("source") == "microphone"
                          for e in events()), 35, "microphone final after speaker failure")
    result = events()
    expected = {"permission": "system-audio-permission", "no-output": "system-audio-device"}[scenario]
    errors = [e for e in result if e.get("event") == "error"]
    assert [e.get("kind") for e in errors] == [expected], errors
    error_index = next(i for i, e in enumerate(result) if e.get("event") == "error")
    assert any(e.get("event") == "final" and e.get("source") == "microphone"
               for e in result[error_index + 1:]), result
    assert "TEST-INJECTION speaker cleanup=complete" in (root / "stderr").read_text()
    if not (root / "exit").exists():
        send_cancel()
    wait_until(lambda: (root / "exit").exists(), 5, "cancel and cleanup")
    names = [e.get("event") for e in events()]
    assert names.count("ready") == 1 and names.count("closed") == 1 and names[-1] == "closed", names
    assert (root / "exit").read_text().strip() == "0"
    assert "audio-tap-cleanup" not in (root / "stderr").read_text()
    for name in ("stdout", "stderr", "exit"):
        shutil.copyfile(root / name, root.parent / f"speaker-{scenario}-{name}")
    print(f"Speaker {scenario}: ready, microphone final, cleanup, closed PASS")
    sys.exit(0)

wait_until(lambda: any(e.get("event") == "ready" for e in events()), ready_timeout, "ready before playback")
log = (root / "stderr").read_text()
assert f"speaker-capture backend={backend} event=start" in log, log
if backend == "process-tap":
    assert "audio-tap format=" in log, "process tap のフォーマット通知がありません"
else:
    assert "stage=shareable-content phase=end" in log, "SCShareableContent が完了していません"
    assert "stage=start-capture phase=end" in log, "SCStream が開始していません"
    assert "audio-format speaker capture=" in log, "SCStream のフォーマット通知がありません"
    assert "audio-tap format=" not in log, "ScreenCaptureKit の強制指定が無視されました"
# Playback is a separate process, so the helper's self-exclusion must not exclude it.
subprocess.run(["afplay", str(root / "input-two-stereo.wav")], check=True, timeout=35)
wait_until(lambda: len([e for e in events() if e.get("event") == "final" and e.get("source") == "speaker"
                       and e.get("text", "").strip()]) >= 2, 15, "two speaker segments")
send_cancel()
wait_until(lambda: (root / "exit").exists(), 5, "cancel and exit")
result = events()
names = [event.get("event") for event in result]
if names.count("ready") != 1 or names.count("closed") != 1 or names[-1] != "closed":
    raise SystemExit(f"Speaker E2E backend={backend} invalid lifecycle: {names}")
if (root / "exit").read_text().strip() != "0":
    raise SystemExit(f"Speaker E2E backend={backend} helper exit was nonzero")
log = (root / "stderr").read_text()
if "audio-tap-cleanup" in log or "no-buffers-after" in log:
    raise SystemExit(f"Speaker E2E backend={backend} detected capture/cleanup failure")
if "source=speaker stage=first-sample" not in log:
    raise SystemExit(f"Speaker E2E backend={backend} has no first-buffer diagnostic")
assert log.count(f"speaker-capture backend={backend} event=stopped") == 1, "停止完了が一度だけ通知されていません"
assert "screen-capture stop failed:" not in log, "SCStream の停止に失敗しました"
finals = [e for e in result if e.get("event") == "final" and e.get("source") == "speaker"]
assert len(finals) == 2, finals
assert len({e["generation"] for e in finals}) == 2, finals
assert all(e["sequence"] > 0 and e["text"].strip() for e in finals), finals
assert "アルファ" in finals[0]["text"] and finals[0]["text"] != finals[1]["text"], finals
assert all(any(e.get("event") == "recognizing" and e.get("source") == "speaker"
               and e.get("generation") == final["generation"] for e in result) for final in finals), result
for name in ("stdout", "stderr", "exit"):
    shutil.copyfile(root / name, root.parent / f"{backend}-{scenario}-{cycle}-{name}")
print(f"Speaker backend={backend}: ready, two segments, stopped, closed PASS")

if scenario == "recovered":
    assert "TEST-INJECTION speaker cleanup=complete" in log
    for name in ("stdout", "stderr", "exit"):
        shutil.copyfile(root / name, root.parent / f"speaker-{scenario}-{name}")
    print("Speaker recovered: ready, speaker final, cleanup, closed PASS")
