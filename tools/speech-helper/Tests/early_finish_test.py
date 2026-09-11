import argparse
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--engine", required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
status = 0
for mode in ("before-start", "during-start", "session"):
    try:
        result = subprocess.run([args.binary, "--real-early-finish", args.engine, mode],
                                capture_output=True, text=True, timeout=12)
        log = result.stdout + result.stderr
        if result.returncode != 0:
            status = 1
        (args.output / f"{mode}.log").write_text(log)
    except subprocess.TimeoutExpired:
        (args.output / f"{mode}.log").write_text("FAIL process timeout\n")
        status = 1
(args.output / "exit-status").write_text(str(status))
raise SystemExit(status)
