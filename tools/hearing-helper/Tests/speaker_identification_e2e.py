"""公開実声 fixture で話者 ID、台帳、再起動、漏えい境界を検証する。"""

import json
import os
from pathlib import Path
import subprocess
import sys
import time


root = Path(sys.argv[1])
state_root = Path(os.environ.get("COOSENPAI_SPEAKER_ID_STATE", root))
phase = sys.argv[2] if len(sys.argv) > 2 else "first"
backend = sys.argv[3] if len(sys.argv) > 3 else "screen-capture-kit"
manifest = json.loads((root / "speaker-identification-manifest.json").read_text())
if phase not in ("first", "restart"):
    raise SystemExit(f"unknown phase: {phase}")
if backend not in ("process-tap", "screen-capture-kit"):
    raise SystemExit(f"unknown speaker backend: {backend}")

wav_name = "speaker-identification.wav" if phase == "first" else "speaker-identification-restart.wav"
expected_segments = manifest["segments"] if phase == "first" else manifest["restartSegments"]


def events():
    path = root / "stdout"
    if not path.exists():
        return []
    result = []
    for line in path.read_text().splitlines(keepends=True):
        if not line.endswith("\n"):
            continue
        if line.strip():
            try:
                result.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return result


def wait_until(condition, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        failure = capture_permission_failure()
        if failure:
            raise SystemExit(failure)
        if condition():
            return
        time.sleep(0.1)
    stderr = (root / "stderr").read_text(errors="replace") if (root / "stderr").exists() else ""
    tail = "\n".join(stderr.splitlines()[-20:])
    raise SystemExit(
        f"話者 ID E2E がタイムアウトしました: {description}; backend={backend}\n{tail}"
    )


def capture_permission_failure():
    if backend != "screen-capture-kit" or not (root / "stderr").exists():
        return None
    stderr = (root / "stderr").read_text(errors="replace")
    if "screen-capture permission denied" in stderr or (
        "SCStreamErrorDomain" in stderr and "-3801" in stderr
    ):
        return (
            "画面収録が許可されていません（SCStreamErrorDomain -3801）。"
            "--request-auth を付けて再実行するか、システム設定の"
            "「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で"
            " HearingE2E を許可してから再実行してください。"
        )
    return None


def send_cancel():
    descriptor = os.open(root / "stdin", os.O_WRONLY | os.O_NONBLOCK)
    try:
        os.write(descriptor, b'{"op":"cancel"}\n')
    finally:
        os.close(descriptor)


def stop_player(player):
    if player.poll() is not None:
        return
    player.terminate()
    try:
        player.wait(timeout=5)
    except subprocess.TimeoutExpired:
        player.kill()
        player.wait(timeout=5)


def cancel_after_playback_failure():
    try:
        send_cancel()
    except OSError:
        pass
    try:
        wait_until(lambda: (root / "exit").exists(), 10, "afplay異常終了後のclosed")
    except SystemExit as error:
        return str(error)
    return None


def play_wav():
    try:
        player = subprocess.Popen(["afplay", str(root / wav_name)])
    except OSError as error:
        cleanup_error = cancel_after_playback_failure()
        message = f"afplay を起動できません: {error}"
        if cleanup_error:
            message += f"; {cleanup_error}"
        raise SystemExit(message)
    try:
        return_code = player.wait(timeout=120)
    except subprocess.TimeoutExpired as error:
        stop_player(player)
        cleanup_error = cancel_after_playback_failure()
        message = f"afplay がタイムアウトしました: {error}"
        if cleanup_error:
            message += f"; {cleanup_error}"
        raise SystemExit(message)
    finally:
        stop_player(player)
    if return_code != 0:
        cleanup_error = cancel_after_playback_failure()
        message = f"afplay が異常終了しました: status={return_code}"
        if cleanup_error:
            message += f"; {cleanup_error}"
        raise SystemExit(message)


wait_until(lambda: any(event.get("event") == "ready" for event in events()), 60, "ready")
ready = next(event for event in events() if event.get("event") == "ready")
if ready.get("speakerIdentification") is not True:
    raise SystemExit(f"話者識別が有効になっていません: {ready}")
if capture_permission_failure():
    raise SystemExit(capture_permission_failure())
capture_log = (root / "stderr").read_text(errors="replace")
if f"speaker-capture backend={backend} event=start" not in capture_log:
    raise SystemExit(f"speaker backend={backend} の開始記録がありません")
if backend == "process-tap":
    if "audio-tap format=" not in capture_log:
        raise SystemExit("process-tap のフォーマット通知がありません")
else:
    for marker in (
        "stage=shareable-content phase=end",
        "stage=start-capture phase=end",
        "audio-format speaker capture=",
    ):
        if marker not in capture_log:
            raise SystemExit(f"ScreenCaptureKit の開始記録がありません: {marker}")
    if "audio-tap format=" in capture_log:
        raise SystemExit("ScreenCaptureKit に process-tap のフォーマット通知があります")


def model_preparation_event():
    return next(
        (
            event
            for event in events()
            if event.get("event") == "speaker-identification"
            and event.get("status") in ("ready", "unavailable")
        ),
        None,
    )


# Core ML の未コンパイル package は初回だけ数十秒かかるため、audio ready とは別に待つ。
wait_until(model_preparation_event, 180, "speaker-identification-ready")
preparation = model_preparation_event()
if preparation.get("status") != "ready":
    raise SystemExit(f"話者識別モデルを準備できません: {preparation}")

play_wav()
wait_until(
    lambda: len([
        event for event in events()
        if event.get("event") == "final" and event.get("source") == "speaker"
    ]) >= len(expected_segments),
    45,
    f"{len(expected_segments)} 件の speaker final",
)

if phase == "restart" and "speaker-identification model status=ready cache=hit" not in (
    root / "stderr"
).read_text(errors="replace"):
    raise SystemExit("再起動後にコンパイル済み Core ML モデルのキャッシュを再利用していません")

send_cancel()
wait_until(lambda: (root / "exit").exists(), 10, "closed")
result = events()
names = [event.get("event") for event in result]
if names.count("ready") != 1 or names.count("closed") != 1 or names[-1] != "closed":
    raise SystemExit(f"話者 ID E2E の lifecycle が不正です: {names}")
if (root / "exit").read_text().strip() != "0":
    raise SystemExit("話者 ID E2E helper が異常終了しました")

finals = [
    event for event in result
    if event.get("event") == "final" and event.get("source") == "speaker"
]
if len(finals) != len(expected_segments):
    raise SystemExit(f"公開実声 fixture の区間数と final 数が一致しません: {len(finals)}")

for event in finals:
    required = ("segmentId", "audioStartMs", "audioEndMs", "speakerStatus")
    if any(key not in event for key in required):
        raise SystemExit(f"speaker final の区間情報が不足しています: {event}")
    if event["audioEndMs"] <= event["audioStartMs"]:
        raise SystemExit(f"speaker final の時刻範囲が不正です: {event}")
    if event["speakerStatus"] == "identified":
        if not event.get("speakerId") or not event.get("speakerRegistryId"):
            raise SystemExit(f"identified に ID または registry ID がありません: {event}")
    elif "speakerId" in event or "speakerRegistryId" in event:
        raise SystemExit(f"不確実な区間に ID が付いています: {event}")

if phase == "first":
    short, registration_a, registration_b = finals[:3]
    if short.get("speakerStatus") != manifest["shortBeforeRegistrationStatus"]:
        raise SystemExit(f"短区間が unknown ではありません: {short}")
    if registration_a.get("speakerStatus") != "identified" or registration_b.get("speakerStatus") != "identified":
        raise SystemExit(f"4秒以上の登録区間が identified ではありません: {finals[:3]}")
    speaker_ids = {
        "A": registration_a["speakerId"],
        "B": registration_b["speakerId"],
    }
    if speaker_ids["A"] == speaker_ids["B"]:
        raise SystemExit(f"A/B が同じ話者 ID です: {speaker_ids}")
    registry_ids = {registration_a["speakerRegistryId"], registration_b["speakerRegistryId"]}
    if len(registry_ids) != 1:
        raise SystemExit(f"A/B の registry ID が一致しません: {registry_ids}")
    evaluations = finals[3:9]
    for event, expected_label in zip(evaluations, manifest["evaluationOrder"]):
        if event.get("speakerStatus") != "identified" or event.get("speakerId") != speaker_ids[expected_label]:
            raise SystemExit(f"評価区間 {expected_label} の ID が不一致です: {event}")
    mixed = finals[9]
    if mixed.get("speakerStatus") not in manifest["mixedAllowedStatuses"]:
        raise SystemExit(f"混在区間が unknown/mixed ではありません: {mixed}")
    if "speakerId" in mixed or "speakerRegistryId" in mixed:
        raise SystemExit(f"混在区間に ID が付きました: {mixed}")
    state_root.mkdir(parents=True, exist_ok=True)
    (state_root / f"speaker-identification-observed-{backend}.json").write_text(
        json.dumps({
            "registryId": next(iter(registry_ids)),
            "speakerIds": speaker_ids,
        }, indent=2) + "\n"
    )
else:
    observed = json.loads(
        (state_root / f"speaker-identification-observed-{backend}.json").read_text()
    )
    evaluations = finals
    for event, expected_label in zip(evaluations, manifest["evaluationOrder"]):
        if event.get("speakerStatus") != "identified" or event.get("speakerId") != observed["speakerIds"][expected_label]:
            raise SystemExit(f"再起動後の評価区間 {expected_label} の ID が不一致です: {event}")
        if event.get("speakerRegistryId") != observed["registryId"]:
            raise SystemExit(f"再起動後の registry ID が不一致です: {event}")

for event in result:
    if event.get("source") != "speaker" and any(
        key in event for key in ("speakerId", "speakerRegistryId", "speakerStatus", "segmentId")
    ):
        raise SystemExit(f"speaker 情報が mic または別イベントへ混入しています: {event}")

leakage_sentinel = manifest.get("leakageSentinel", "").strip().lower()
if not leakage_sentinel:
    raise SystemExit("漏えい検査用 sentinel が manifest にありません")


def output_without_final_text(path):
    if path.name != "stdout":
        return path.read_text(errors="replace")
    protected = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            protected.append(line)
            continue
        if event.get("event") == "final":
            event = {key: value for key, value in event.items() if key != "text"}
        protected.append(json.dumps(event, ensure_ascii=False, sort_keys=True))
    return "\n".join(protected)


for path in (root / "stdout", root / "stderr"):
    text = output_without_final_text(path).lower()
    for sentinel in ("embedding", "centroid", "anchor", "fbank", "pcm", "featurevector"):
        if sentinel in text:
            raise SystemExit(f"{path.name} に内部音声情報の sentinel があります: {sentinel}")
    if leakage_sentinel in text:
        raise SystemExit(f"{path.name} に内部音声情報の sentinel があります: {leakage_sentinel}")

ledger_path = state_root / "speaker-registry.enc"
if not ledger_path.exists():
    raise SystemExit("暗号化話者台帳がありません")
ledger_bytes = ledger_path.read_bytes()
for sentinel in (b"speaker-", b"centroid", b"anchor", b"embedding"):
    if sentinel in ledger_bytes:
        raise SystemExit(f"暗号化台帳に平文の内部情報があります: {sentinel!r}")
alias_path = state_root / "aliases.json"
if not alias_path.exists():
    raise SystemExit("canonical 表示用の alias 索引がありません")
aliases = json.loads(alias_path.read_text())
if set(aliases) != {"schemaVersion", "registryId", "aliases"}:
    raise SystemExit(f"alias 索引に許可外の項目があります: {aliases}")

print(
    f"話者 ID 公開実声 E2E backend={backend} phase={phase}: "
    f"{len(finals)} 区間、ID一貫性、台帳、漏えい境界 PASS"
)
