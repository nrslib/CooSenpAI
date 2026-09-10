"""A real process tap must become ready without playback and close cleanly on cancel."""
import json
import shutil
from pathlib import Path
import subprocess
import sys
import time

root = Path(sys.argv[1])
scenario = sys.argv[2] if len(sys.argv) > 2 else "normal"
# Initial interactive permission grants are separate from the normal 8-second startup check.
ready_timeout = 60 if sys.argv[3:] == ["1"] else 8


def events():
    path = root / "stdout"
    if not path.exists():
        return []
    # The helper flushes complete NDJSON lines; ignore an in-progress final line.
    lines = path.read_text().splitlines(keepends=True)
    result = [json.loads(line) for line in lines if line.endswith("\n")]
    errors = [event for event in result if event.get("event") == "error"]
    if errors and scenario in ("normal", "recovered"):
        raise SystemExit(f"Process tap E2E error: {errors}")
    return result


def wait_until(condition, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.02)
    for name in ("stdout", "stderr"):
        if (root / name).exists():
            shutil.copyfile(root / name, root.parent / f"failed-{scenario}-{name}")
    raise SystemExit(f"Process tap E2E timeout: {description}; artifacts: {root.parent}")


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
        with (root / "stdin").open("w") as stream:
            stream.write('{"op":"cancel"}\n')
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
if "audio-tap format=" not in log:
    raise SystemExit("Process tap E2E requires macOS 14.2+; legacy capture is not a passing result")
# Playback is a separate process, so the helper's self-exclusion must not exclude it.
subprocess.run(["afplay", str(root / "input-two-stereo.wav")], check=True, timeout=35)
wait_until(lambda: any(e.get("event") == "final" and e.get("source") == "speaker"
                       and e.get("text", "").strip() for e in events()), 15, "speaker transcription")
with (root / "stdin").open("w") as stream:
    stream.write('{"op":"cancel"}\n')
wait_until(lambda: (root / "exit").exists(), 5, "cancel and exit")
result = events()
names = [event.get("event") for event in result]
if names.count("ready") != 1 or names.count("closed") != 1 or names[-1] != "closed":
    raise SystemExit(f"Process tap E2E invalid lifecycle: {names}")
if (root / "exit").read_text().strip() != "0":
    raise SystemExit("Process tap E2E helper exit was nonzero")
log = (root / "stderr").read_text()
if "audio-tap-cleanup" in log or "no-buffers-after" in log:
    raise SystemExit("Process tap E2E detected capture/cleanup failure")
if "source=speaker stage=first-sample" not in log:
    raise SystemExit("Process tap E2E has no first-buffer diagnostic")

if scenario == "recovered":
    assert "TEST-INJECTION speaker cleanup=complete" in log
    for name in ("stdout", "stderr", "exit"):
        shutil.copyfile(root / name, root.parent / f"speaker-{scenario}-{name}")
    print("Speaker recovered: ready, speaker final, cleanup, closed PASS")
