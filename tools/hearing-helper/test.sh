#!/bin/sh
set -eu

request_auth=0
# 既定では process tap が利用可能な OS なら両 backend、利用できなければ
# ScreenCaptureKit のみを実収録する。--process-tap は process tap 側を明示する。
process_tap_supported=$(sw_vers -productVersion | awk -F. '{ print ($1 > 14 || ($1 == 14 && $2 >= 2)) ? 1 : 0 }')
process_tap=$process_tap_supported
explicit_process_tap=0
speaker_id_backend=screen-capture-kit
speaker_failures=0
skip_speaker_e2e=0
speaker_identification_e2e=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --request-auth) request_auth=1 ;;
    --process-tap) explicit_process_tap=1; process_tap=1 ;;
    --speaker-failures) speaker_failures=1 ;;
    --skip-speaker-e2e) skip_speaker_e2e=1 ;;
    --speaker-identification-e2e) speaker_identification_e2e=1 ;;
    *) printf '使い方: %s [--request-auth] [--process-tap] [--speaker-failures] [--skip-speaker-e2e] [--speaker-identification-e2e]\n' "$0" >&2; exit 2 ;;
  esac
  shift
done
if [ "$explicit_process_tap" -eq 1 ] && [ "$process_tap_supported" -eq 0 ]; then
  printf '%s\n' '--process-tap は macOS 14.2 以降でのみ使用できます' >&2
  exit 2
fi
if [ "$skip_speaker_e2e" -eq 1 ] && [ "$speaker_failures" -eq 1 ]; then
  printf '%s\n' '--skip-speaker-e2e と --speaker-failures は同時に指定できません' >&2
  exit 2
fi
if [ "$skip_speaker_e2e" -eq 1 ] && [ "$speaker_identification_e2e" -eq 1 ]; then
  printf '%s\n' '--skip-speaker-e2e と --speaker-identification-e2e は同時に指定できません' >&2
  exit 2
fi
speaker_model_path=${COOSENPAI_SPEAKER_MODEL:-}
speaker_golden_model_path=${COOSENPAI_SPEAKER_GOLDEN_MODEL:-}
speaker_fixture_directory=${COOSENPAI_SPEAKER_FIXTURE_DIR:-$HOME/work/data/models/speaker-id/fixtures}
if [ -z "$speaker_golden_model_path" ] && [ "$speaker_identification_e2e" -eq 1 ]; then
  speaker_golden_model_path=$speaker_model_path
fi
if [ "$speaker_identification_e2e" -eq 1 ] && {
  [ ! -e "$speaker_model_path" ] ||
  [ "${speaker_model_path##*.}" != mlpackage ];
}; then
 printf '%s\n' '話者 ID E2E: COOSENPAI_SPEAKER_MODEL に実在する Core ML .mlpackage を指定してください' >&2
 exit 2
fi
if [ "$speaker_identification_e2e" -eq 1 ] && {
  [ ! -f "$speaker_fixture_directory/speaker-a.wav" ] ||
  [ ! -f "$speaker_fixture_directory/speaker-b.wav" ];
}; then
  printf '話者 ID E2E: COOSENPAI_SPEAKER_FIXTURE_DIR に公開実声 fixture を配置してください: %s\n' \
    "$speaker_fixture_directory" >&2
  exit 2
fi
if [ "$process_tap" -eq 1 ]; then speaker_id_backend=process-tap; fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
mkdir -p "$repository_dir/target/helpers"
temporary_path=$(mktemp "$repository_dir/target/helpers/coosenpai-hearing-test.XXXXXX")
temporary_object_path="${temporary_path}.o"
temporary_ring_object_path="${temporary_path}.ring.o"
module_cache_dir="$script_dir/../../target/helpers/test-module-cache"
e2e_root="$repository_dir/target/helpers/hearing-e2e"
e2e_runs_root="$e2e_root/runs"
e2e_fixture_directory="$e2e_root/fixtures"
e2e_directory=
e2e_config=
e2e_app="$repository_dir/target/helpers/HearingE2E.app"
runner_pid=
run_sequence=0
run_id=
cleanup_done=0

wait_for_process_exit() {
  process_pid=$1
  process_deadline=$(( $(date +%s) + 5 ))
  while process_is_running "$process_pid"; do
    if [ "$(date +%s)" -ge "$process_deadline" ]; then
      return 1
    fi
    sleep 0.1
  done
  return 0
}

process_start_time() {
  ps -p "$1" -o lstart= 2>/dev/null | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

process_executable() {
  ps -p "$1" -o comm= 2>/dev/null | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

process_is_running() {
  process_pid=$1
  process_state=$(ps -p "$process_pid" -o stat= 2>/dev/null) || process_state=
  case "$process_state" in
    ''|Z*) return 1 ;;
  esac
  kill -0 "$process_pid" 2>/dev/null
}

process_identity_matches() {
  process_pid=$1
  expected_started_at=$2
  expected_command=$3
  current_started_at=$(process_start_time "$process_pid") || return 1
  current_command=$(process_executable "$process_pid") || return 1
  [ "$current_started_at" = "$expected_started_at" ] \
    && [ "$current_command" = "$expected_command" ]
}

capture_process_identity() {
  captured_started_at=$(process_start_time "$1")
  captured_command=$(process_executable "$1")
  [ -n "$captured_started_at" ] && [ -n "$captured_command" ]
}

write_pid_record() {
  pid_file=$1
  process_pid=$2
  process_run_id=$3
  if ! capture_process_identity "$process_pid"; then return 1; fi
  printf '%s|%s|%s|%s\n' \
    "$process_pid" "$process_run_id" "$captured_started_at" "$captured_command" > "$pid_file"
}

stop_pid_file() {
  pid_file=$1
  expected_run_id=${2-}
  [ -f "$pid_file" ] || return 0
  if ! IFS='|' read -r pid pid_run_id pid_started_at pid_command < "$pid_file"; then
    printf 'E2E cleanup: PID 記録を読めません: %s\n' "$pid_file" >&2
    return 1
  fi
  case "$pid" in ''|*[!0-9]*)
    printf 'E2E cleanup: PID が不正です: %s\n' "$pid_file" >&2
    return 1
    ;;
  esac
  if [ -n "$expected_run_id" ] && [ "$pid_run_id" != "$expected_run_id" ]; then
    printf 'E2E cleanup: PID 記録の実行識別子が一致しません: %s\n' "$pid_file" >&2
    return 1
  fi
  if ! process_is_running "$pid"; then
    rm -f "$pid_file"
    return 0
  fi
  if ! process_identity_matches "$pid" "$pid_started_at" "$pid_command"; then
    if ! process_is_running "$pid"; then
      rm -f "$pid_file"
      return 0
    fi
    printf 'E2E cleanup: PID の同一性を確認できないため終了シグナルを送りません: %s\n' "$pid_file" >&2
    return 1
  fi
  kill "$pid" 2>/dev/null || true
  if ! wait_for_process_exit "$pid"; then
    if ! process_identity_matches "$pid" "$pid_started_at" "$pid_command"; then
      if ! process_is_running "$pid"; then
        rm -f "$pid_file"
        return 0
      fi
      printf 'E2E cleanup: TERM 後に PID の同一性を確認できません: %s\n' "$pid_file" >&2
      return 1
    fi
    kill -KILL "$pid" 2>/dev/null || true
    if ! wait_for_process_exit "$pid"; then
      printf 'E2E cleanup: プロセス終了を確認できません: %s\n' "$pid_file" >&2
      return 1
    fi
  fi
  rm -f "$pid_file"
}

stop_run_directory() {
  run_directory=$1
  run_name=${run_directory##*/}
  case "$run_name" in
    ''|*[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-]*)
      printf 'E2E cleanup: run のディレクトリ名が不正です: %s\n' "$run_directory" >&2
      return 1
      ;;
  esac
  run_config_path="$run_directory/config"
  if [ ! -f "$run_config_path" ]; then
    printf 'E2E cleanup: run の設定がありません: %s\n' "$run_config_path" >&2
    return 1
  fi
  stored_run_id=$(sed -n 's/^run_id=//p' "$run_config_path" | sed -n '1p')
  if [ -z "$stored_run_id" ] || [ "$stored_run_id" != "$run_name" ]; then
    printf 'E2E cleanup: run の識別子が設定と一致しません: %s\n' "$run_config_path" >&2
    return 1
  fi
  stop_status=0
  for pid_file in \
    "$run_directory/runner.pid" \
    "$run_directory/helper.pid" \
    "$run_directory/stdin.pid" \
    "$run_directory/app.pid"; do
    if ! stop_pid_file "$pid_file" "$stored_run_id"; then stop_status=1; fi
  done
  return "$stop_status"
}

stop_e2e_runner() {
  stop_status=0
  if [ -d "$e2e_runs_root" ]; then
    for run_directory in "$e2e_runs_root"/*; do
      [ -d "$run_directory" ] || continue
      if ! stop_run_directory "$run_directory"; then stop_status=1; fi
    done
  fi
  return "$stop_status"
}

remove_stopped_e2e_runs() {
  remove_status=0
  if [ -d "$e2e_runs_root" ]; then
    for run_directory in "$e2e_runs_root"/*; do
      [ -d "$run_directory" ] || continue
      run_name=${run_directory##*/}
      run_config_path="$run_directory/config"
      stored_run_id=$(sed -n 's/^run_id=//p' "$run_config_path" 2>/dev/null | sed -n '1p')
      if [ -z "$stored_run_id" ] || [ "$stored_run_id" != "$run_name" ]; then
        printf 'E2E cleanup: 停止済み run の設定を検証できないため記録を残します: %s\n' "$run_directory" >&2
        remove_status=1
        continue
      fi
      if [ -f "$run_directory/runner.pid" ] \
        || [ -f "$run_directory/helper.pid" ] \
        || [ -f "$run_directory/stdin.pid" ] \
        || [ -f "$run_directory/app.pid" ]; then
        continue
      fi
      if ! rm -rf "$run_directory"; then
        printf 'E2E cleanup: 停止済み run を削除できません: %s\n' "$run_directory" >&2
        remove_status=1
      fi
    done
  fi
  return "$remove_status"
}

cleanup() {
  original_status=$?
  if [ "$cleanup_done" -eq 1 ]; then return 0; fi
  cleanup_done=1
  cleanup_status=0
  if ! stop_e2e_runner; then cleanup_status=1; fi
  if ! rm -f "$temporary_path" "$temporary_object_path" "$temporary_ring_object_path"; then
    cleanup_status=1
  fi
  for artifact in stdout stderr exit; do
    if [ -n "$e2e_directory" ] && [ -f "$e2e_directory/$artifact" ]; then
      if ! cp "$e2e_directory/$artifact" "$e2e_root/last-$artifact"; then cleanup_status=1; fi
    fi
  done
  if ! remove_stopped_e2e_runs; then cleanup_status=1; fi
  if [ "$cleanup_status" -eq 0 ]; then
    if [ -d "$e2e_runs_root" ]; then
      for run_directory in "$e2e_runs_root"/*; do
        [ -d "$run_directory" ] || continue
        cleanup_status=1
        break
      done
    fi
    if [ "$cleanup_status" -eq 0 ] && ! rm -rf "$e2e_fixture_directory" \
      "$e2e_root/speaker-identification-state-process-tap" \
      "$e2e_root/speaker-identification-state-screen-capture-kit"; then
      cleanup_status=1
    fi
  fi
  final_status=$original_status
  if [ "$final_status" -eq 0 ] && [ "$cleanup_status" -ne 0 ]; then final_status=1; fi
  exit "$final_status"
}
trap cleanup EXIT
trap 'exit 143' HUP INT TERM

mkdir -p "$module_cache_dir"
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
clang -target "$(uname -m)-apple-macosx13.0" -isysroot "$sdk_path" -fobjc-arc -c \
  "$script_dir/Sources/audio_tap_installer.m" -o "$temporary_object_path"
clang -target "$(uname -m)-apple-macosx13.0" -isysroot "$sdk_path" -std=c11 -O2 -c \
  "$script_dir/Sources/speaker_audio_ring.c" -o "$temporary_ring_object_path"
swiftc -target "$(uname -m)-apple-macosx13.0" \
  -parse-as-library \
  -module-cache-path "$module_cache_dir" \
  -import-objc-header "$script_dir/Sources/audio_tap_installer.h" \
  "$script_dir/Sources/speaker_audio_tap.swift" \
  "$script_dir/Sources/speaker_backend.swift" \
  "$script_dir/Sources/speaker_screen_capture.swift" \
  "$script_dir/Sources/speaker_screen_device.swift" \
  "$script_dir/Sources/speaker_audio_device.swift" \
  "$script_dir/Sources/audio_stats.swift" \
  "$script_dir/Sources/audio_scaling.swift" \
  "$script_dir/Sources/audio_buffer_copy.swift" \
  "$script_dir/Sources/audio_conversion.swift" \
  "$script_dir/Sources/audio_input_processing.swift" \
  "$script_dir/Sources/music_gate.swift" \
  "$script_dir/Sources/microphone_input_recovery.swift" \
  "$script_dir/Sources/speaker_identification.swift" \
  "$script_dir/../speech-helper/Sources/speech_analysis.swift" \
  "$script_dir/../speech-helper/Sources/audio_queue.swift" \
  "$script_dir/../speech-helper/Sources/transcript_accumulator.swift" \
  "$script_dir/../speech-helper/Sources/recognition_session.swift" \
  "$script_dir/../speech-helper/Sources/audio_converter.swift" \
  "$script_dir/../speech-helper/Sources/speech_audio_gain.swift" \
  "$script_dir/../speech-helper/Sources/speech_analyzer.swift" \
  "$script_dir/../speech-helper/Sources/speech_recognizer.swift" \
  "$script_dir/../speech-helper/Sources/speech_engine.swift" \
  "$script_dir/Sources/recognition_state.swift" \
  "$script_dir/Sources/segment_controller.swift" \
  "$script_dir/Sources/voice_activity.swift" \
  "$script_dir/Sources/wav_input.swift" \
  "$script_dir/Sources/appended_audio_dump.swift" \
  "$script_dir/Sources/main.swift" \
  "$temporary_object_path" "$temporary_ring_object_path" \
  "$script_dir/Tests/audio_stats_test.swift" \
  "$script_dir/Tests/speaker_audio_test.swift" \
  "$script_dir/Tests/speaker_screen_capture_test.swift" \
  "$script_dir/Tests/audio_scaling_test.swift" \
  "$script_dir/Tests/audio_buffer_copy_test.swift" \
  "$script_dir/Tests/audio_conversion_test.swift" \
  "$script_dir/Tests/voice_activity_test.swift" \
  "$script_dir/Tests/recognition_state_test.swift" \
  "$script_dir/Tests/wav_input_test.swift" \
  "$script_dir/Tests/appended_audio_dump_test.swift" \
  "$script_dir/Tests/music_gate_test.swift" \
  "$script_dir/Tests/microphone_input_recovery_test.swift" \
  "$script_dir/Tests/speaker_identification_test.swift" \
  -o "$temporary_path"
if [ -n "$speaker_golden_model_path" ]; then
  COOSENPAI_SPEAKER_GOLDEN_MODEL="$speaker_golden_model_path" "$temporary_path"
else
  "$temporary_path"
fi
python3 - "$temporary_path" <<'PYTEST'
import subprocess, sys
result = subprocess.run([sys.argv[1], "--speaker-stop-timeout"], capture_output=True, text=True, timeout=5)
assert result.returncode == 1, result
assert "operation=stop-timeout action=terminate-process" in result.stderr, result.stderr
assert "completion before release" not in result.stderr, result.stderr
print("Speaker blocked cleanup: process exited without completion")
result = subprocess.run([sys.argv[1], "--speaker-stop-failure"], capture_output=True, text=True, timeout=5)
assert result.returncode == 1, result
assert "operation=stop-device" in result.stderr and "action=terminate-process" in result.stderr, result.stderr
assert "completion after stop failure" not in result.stderr, result.stderr
print("Speaker stop failure: process exited without completion")
result = subprocess.run([sys.argv[1], "--screen-capture-stop-failure"], capture_output=True, text=True, timeout=5)
assert result.returncode == 1, result
assert "screen-capture stop failed:" in result.stderr, result.stderr
assert "stopped" not in result.stderr and "closed" not in result.stdout, result
print("ScreenCaptureKit stop failure: process exited without stopped/closed")
PYTEST

"$script_dir/build.sh" >/dev/null
if [ "$skip_speaker_e2e" -eq 1 ]; then
  printf '%s\n' 'WAV E2E: --skip-speaker-e2e により実収録を省略しました' >&2
  exit 0
fi
mkdir -p "$e2e_root"
if ! stop_e2e_runner || ! remove_stopped_e2e_runs; then
  printf '%s\n' 'E2E cleanup: 過去の run を安全に停止できないため、新しい E2E を開始しません' >&2
  exit 1
fi
rm -rf "$e2e_fixture_directory"
mkdir -p "$e2e_runs_root" "$e2e_fixture_directory"
"$script_dir/prepare-e2e-wav.sh" "$e2e_fixture_directory" >/dev/null
if [ "$speaker_identification_e2e" -eq 1 ]; then
  "$script_dir/prepare-speaker-identification-e2e.sh" "$e2e_fixture_directory" >/dev/null
fi
mkdir -p \
  "$e2e_app/Contents/MacOS" \
  "$e2e_app/Contents/Resources"
cat > "$e2e_app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>dev.nrslib.coosenpai.hearing-e2e</string>
  <key>CFBundleName</key>
  <string>HearingE2E</string>
  <key>CFBundleExecutable</key>
  <string>run</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>LSBackgroundOnly</key>
  <false/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>ScreenCaptureKit のスピーカー録音を検証するため画面収録を使用します。</string>
  <key>NSAudioCaptureUsageDescription</key>
  <string>スピーカー録音の E2E テストにシステムオーディオを使用します。</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>音声認識の E2E テストにマイクを使用します。</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>音声認識の E2E テストに音声認識を使用します。</string>
</dict>
</plist>
PLIST
cat > "$e2e_app/Contents/MacOS/runner.sh" <<'RUN'
#!/bin/sh
set -eu

run_id_argument=${1-}
case "$run_id_argument" in
  ''|*[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-]*)
    printf '%s\n' 'hearing E2E の実行識別子が不正です' >&2
    exit 1
    ;;
esac
hearing_e2e_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../hearing-e2e" && pwd)
config_path="$hearing_e2e_root/runs/$run_id_argument/config"
config_value() {
  sed -n "s/^$1=//p" "$config_path" | sed -n '1p'
}
helper_path=$(config_value helper_path)
mode=$(config_value mode)
test_state=$(config_value test_state)
input_wav=$(config_value input_wav)
speaker_model=$(config_value speaker_model)
speaker_ledger=$(config_value speaker_ledger)
speaker_backend=$(config_value speaker_backend)
explicit_process_tap=$(config_value explicit_process_tap)
run_id=$(config_value run_id)
if [ "$run_id" != "$run_id_argument" ]; then
  printf '%s\n' 'hearing E2E の実行識別子が設定と一致しません' >&2
  exit 1
fi
dump_dir=$(config_value dump_dir)
stdin_path=$(config_value stdin_path)
stdout_path=$(config_value stdout_path)
stderr_path=$(config_value stderr_path)
exit_path=$(config_value exit_path)
runner_pid_path=$(config_value runner_pid_path)
helper_pid_path=$(config_value helper_pid_path)
stdin_pid_path=$(config_value stdin_pid_path)

process_start_time() {
  ps -p "$1" -o lstart= 2>/dev/null | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

process_executable() {
  ps -p "$1" -o comm= 2>/dev/null | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

process_is_running() {
  process_pid=$1
  process_state=$(ps -p "$process_pid" -o stat= 2>/dev/null) || process_state=
  case "$process_state" in
    ''|Z*) return 1 ;;
  esac
  kill -0 "$process_pid" 2>/dev/null
}

process_identity_matches() {
  process_pid=$1
  expected_started_at=$2
  expected_command=$3
  current_started_at=$(process_start_time "$process_pid") || return 1
  current_command=$(process_executable "$process_pid") || return 1
  [ "$current_started_at" = "$expected_started_at" ] \
    && [ "$current_command" = "$expected_command" ]
}

capture_process_identity() {
  captured_started_at=$(process_start_time "$1")
  captured_command=$(process_executable "$1")
  [ -n "$captured_started_at" ] && [ -n "$captured_command" ]
}

write_pid_record() {
  pid_file=$1
  process_pid=$2
  if ! capture_process_identity "$process_pid"; then return 1; fi
  printf '%s|%s|%s|%s\n' \
    "$process_pid" "$run_id" "$captured_started_at" "$captured_command" > "$pid_file"
}

pid_record_identity_matches() {
  pid_file=$1
  process_pid=$2
  [ -f "$pid_file" ] || return 1
  if ! IFS='|' read -r recorded_pid recorded_run_id recorded_started_at recorded_command < "$pid_file"; then
    return 1
  fi
  [ "$recorded_pid" = "$process_pid" ] \
    && [ "$recorded_run_id" = "$run_id" ] \
    && process_identity_matches "$process_pid" "$recorded_started_at" "$recorded_command"
}

write_stdin_pid_record() {
  stdin_record_deadline=$(( $(date +%s) + 5 ))
  while :; do
    if ! process_is_running "$stdin_pid"; then return 1; fi
    if ! capture_process_identity "$stdin_pid"; then
      captured_command=
    fi
    case "$captured_command" in
      tail|/usr/bin/tail)
        if write_pid_record "$stdin_pid_path" "$stdin_pid"; then return 0; fi
        ;;
    esac
    if [ "$(date +%s)" -ge "$stdin_record_deadline" ]; then return 1; fi
    sleep 0.01
  done
}

if ! write_pid_record "$runner_pid_path" "$$"; then
  printf '%s\n' 'hearing E2E: runner の PID 同一性情報を記録できませんでした' >&2
  exit 1
fi
helper_pid=
stdin_pid=
helper_started_at=
helper_command=
stdin_started_at=
stdin_command=

stop_child() {
  child_pid=$1
  child_pid_file=$2
  expected_started_at=${3-}
  expected_command=${4-}
  [ -n "$child_pid" ] || return 0
  if process_is_running "$child_pid"; then
    if ! pid_record_identity_matches "$child_pid_file" "$child_pid"; then
      if ! process_is_running "$child_pid"; then
        wait "$child_pid" 2>/dev/null || true
        return 0
      fi
      if [ -z "$expected_started_at" ] || [ -z "$expected_command" ] \
        || ! process_identity_matches "$child_pid" "$expected_started_at" "$expected_command"; then
        printf 'hearing E2E: 終了対象の PID 同一性を確認できません: %s\n' "$child_pid_file" >&2
        return 1
      fi
    fi
    kill "$child_pid" 2>/dev/null || true
    child_deadline=$(( $(date +%s) + 5 ))
    while process_is_running "$child_pid"; do
      if [ "$(date +%s)" -ge "$child_deadline" ]; then
        if ! pid_record_identity_matches "$child_pid_file" "$child_pid"; then
          if ! process_is_running "$child_pid"; then
            wait "$child_pid" 2>/dev/null || true
            return 0
          fi
          if [ -z "$expected_started_at" ] || [ -z "$expected_command" ] \
            || ! process_identity_matches "$child_pid" "$expected_started_at" "$expected_command"; then
            printf 'hearing E2E: KILL 前に PID 同一性を確認できません: %s\n' "$child_pid_file" >&2
            return 1
          fi
        fi
        kill -KILL "$child_pid" 2>/dev/null || true
        if process_is_running "$child_pid"; then return 1; fi
        break
      fi
      sleep 0.1
    done
  fi
  wait "$child_pid" 2>/dev/null || true
}

cleanup_children() {
  cleanup_status=0
  if ! stop_child "$helper_pid" "$helper_pid_path" "$helper_started_at" "$helper_command"; then cleanup_status=1; fi
  if ! stop_child "$stdin_pid" "$stdin_pid_path" "$stdin_started_at" "$stdin_command"; then cleanup_status=1; fi
  if [ "$cleanup_status" -eq 0 ]; then
    rm -f "$helper_pid_path" "$stdin_pid_path"
  fi
  return "$cleanup_status"
}
trap 'cleanup_children || true; exit 143' HUP INT TERM

rm -f "$stdin_path" "$stdout_path" "$stderr_path" "$exit_path"
mkfifo "$stdin_path"
# 先に read/write で FIFO を開くことで、リダイレクト中の一時的な shell PID を記録しない。
exec 3<> "$stdin_path"
tail -f /dev/null >&3 &
stdin_pid=$!
exec 3>&-
if ! write_stdin_pid_record; then
  stdin_started_at=${captured_started_at-}
  stdin_command=${captured_command-}
  printf '%s\n' 'hearing E2E: stdin 供給プロセスの PID 同一性情報を記録できませんでした' >&2
  cleanup_status=0
  if ! stop_child "$stdin_pid" "$stdin_pid_path" "$stdin_started_at" "$stdin_command"; then
    cleanup_status=1
  fi
  if [ "$cleanup_status" -eq 0 ]; then
    rm -f "$stdin_pid_path"
  fi
  exit 1
fi
stdin_started_at=$captured_started_at
stdin_command=$captured_command

if [ "$mode" = screen-auth ]; then
  "$helper_path" \
    --locale ja-JP \
    --input-device default \
    --sources speaker \
    --debug-request-screen-capture-auth \
    < "$stdin_path" \
    > "$stdout_path" \
    2> "$stderr_path" &
elif [ "$mode" = auth ] || [ "$mode" = failure-auth ]; then
  "$helper_path" \
    --locale ja-JP \
    --input-device default \
    --sources microphone \
    --debug-request-auth \
    < "$stdin_path" \
    > "$stdout_path" \
    2> "$stderr_path" &
elif [ "$mode" = speaker-failure ]; then
  COOSENPAI_SPEAKER_TEST_STATE="$test_state" "$helper_path" \
    --locale ja-JP --input-device default --sources microphone,speaker \
    --debug-input-wav "$input_wav" \
    < "$stdin_path" > "$stdout_path" 2> "$stderr_path" &
elif [ "$mode" = process-tap ] || [ "$mode" = screen-capture-kit ] || [ "$mode" = speaker-recovered ]; then
  backend=$mode
  if [ "$mode" = process-tap ] && [ "$explicit_process_tap" -eq 0 ]; then backend=auto; fi
  if [ "$mode" = speaker-recovered ]; then backend=auto; fi
  "$helper_path" --locale ja-JP --input-device default --sources speaker --speaker-backend "$backend" \
    < "$stdin_path" > "$stdout_path" 2> "$stderr_path" &
elif [ "$mode" = speaker-identification ]; then
  "$helper_path" --locale en-US --input-device default --sources speaker --speaker-backend "$speaker_backend" \
    --speaker-identification --speaker-model "$speaker_model" --speaker-ledger "$speaker_ledger" \
    < "$stdin_path" > "$stdout_path" 2> "$stderr_path" &
else
  "$helper_path" \
    --locale ja-JP \
    --input-device default \
    --sources microphone \
    --debug-input-wav "$input_wav" \
    --debug-dump-appended "$dump_dir" \
    < "$stdin_path" \
    > "$stdout_path" \
    2> "$stderr_path" &
fi
helper_pid=$!
if ! write_pid_record "$helper_pid_path" "$helper_pid"; then
  helper_started_at=${captured_started_at-}
  helper_command=${captured_command-}
  printf '%s\n' 'hearing E2E: helper の PID 同一性情報を記録できないため起動を失敗扱いにします' >&2
  cleanup_status=0
  if ! stop_child "$helper_pid" "$helper_pid_path" "$helper_started_at" "$helper_command"; then
    cleanup_status=1
  fi
  if ! stop_child "$stdin_pid" "$stdin_pid_path" "$stdin_started_at" "$stdin_command"; then
    cleanup_status=1
  fi
  if [ "$cleanup_status" -eq 0 ]; then
    rm -f "$helper_pid_path" "$stdin_pid_path"
  fi
  exit 1
fi
helper_started_at=$captured_started_at
helper_command=$captured_command

if wait "$helper_pid"; then
  exit_status=0
else
  exit_status=$?
fi
cleanup_status=0
if ! stop_child "$stdin_pid" "$stdin_pid_path" "$stdin_started_at" "$stdin_command"; then cleanup_status=1; fi
helper_pid=
stdin_pid=
if [ "$cleanup_status" -eq 0 ] && ! rm -f "$helper_pid_path" "$stdin_pid_path"; then
  cleanup_status=1
fi
if [ "$cleanup_status" -ne 0 ] && [ "$exit_status" -eq 0 ]; then exit_status=1; fi
printf '%s\n' "$exit_status" > "$exit_path"
exit "$exit_status"
RUN
chmod 755 "$e2e_app/Contents/MacOS/runner.sh"
swiftc -target "$(uname -m)-apple-macosx13.0" -O -module-cache-path "$module_cache_dir" \
  "$script_dir/Tests/e2e_launcher.swift" -o "$e2e_app/Contents/MacOS/run"
codesign --force --deep --sign - "$e2e_app" >/dev/null

write_e2e_config() {
  run_sequence=$((run_sequence + 1))
  run_id=$(uuidgen)
  e2e_directory="$e2e_runs_root/$run_id"
  e2e_config="$e2e_directory/config"
  mkdir -p "$e2e_directory"
  cp "$e2e_fixture_directory/input-two-stereo.wav" "$e2e_directory/input-two-stereo.wav"
  if [ "$speaker_identification_e2e" -eq 1 ]; then
    cp "$e2e_fixture_directory/speaker-identification-"* \
      "$e2e_fixture_directory/speaker-identification-manifest.json" "$e2e_directory/"
  fi
  cat > "$e2e_directory/speaker-helper.sh" <<'SPEAKER'
#!/bin/sh
set -eu
test_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$test_root/../../../coosenpai-hearing" "$@" --debug-dump-appended "$test_root/dump"
SPEAKER
  chmod 755 "$e2e_directory/speaker-helper.sh"
  helper_name=coosenpai-hearing
  case "$1" in speaker-failure|speaker-recovered|failure-auth) helper_name=coosenpai-hearing-failure-test ;; esac
  helper_path="$repository_dir/target/helpers/$helper_name"
  case "$1" in process-tap|screen-capture-kit|speaker-identification) helper_path="$e2e_directory/speaker-helper.sh" ;; esac
  speaker_backend_selection=$speaker_id_backend
  if [ "$speaker_id_backend" = process-tap ] && [ "$explicit_process_tap" -eq 0 ]; then
    speaker_backend_selection=auto
  fi
  cat > "$e2e_config" <<EOF
helper_path=$helper_path
mode=$1
test_state=$e2e_directory/failure-state
input_wav=$e2e_directory/input-two-stereo.wav
speaker_model=$speaker_model_path
speaker_ledger=$e2e_root/speaker-identification-state-$speaker_id_backend/speaker-registry.enc
speaker_backend=$speaker_backend_selection
explicit_process_tap=$explicit_process_tap
run_id=$run_id
dump_dir=$e2e_directory/dump
stdin_path=$e2e_directory/stdin
stdout_path=$e2e_directory/stdout
stderr_path=$e2e_directory/stderr
exit_path=$e2e_directory/exit
runner_pid_path=$e2e_directory/runner.pid
helper_pid_path=$e2e_directory/helper.pid
stdin_pid_path=$e2e_directory/stdin.pid
EOF
}

launch_e2e() {
  if ! stop_e2e_runner || ! remove_stopped_e2e_runs; then
    printf '%s\n' 'WAV E2E: 過去の run の後始末に失敗したため起動しません' >&2
    return 1
  fi
  write_e2e_config "$1"
  rm -f \
    "$e2e_directory/stdin" \
    "$e2e_directory/stdout" \
    "$e2e_directory/stderr" \
    "$e2e_directory/exit" \
    "$e2e_directory/runner.pid" \
    "$e2e_directory/helper.pid" \
    "$e2e_directory/stdin.pid" \
    "$e2e_directory/app.pid"
  # 直接起動への切替は TCC の認可主体を変えるため、起動失敗を隠さない。
  if ! open -n -a "$e2e_app" --args "$run_id"; then
    printf '%s\n' 'WAV E2E: LaunchServices による起動に失敗しました。音声認識の権限は未判定です。サンドボックス外のログイン済み macOS セッションで再実行してください。' >&2
    return 1
  fi
  launch_deadline=$(( $(date +%s) + 10 ))
  while [ ! -f "$e2e_directory/runner.pid" ]; do
    if [ "$(date +%s)" -ge "$launch_deadline" ]; then
      printf '%s\n' 'WAV E2E: 起動後10秒以内に runner が応答しませんでした。音声認識の権限は未判定です。' >&2
      stop_e2e_runner
      return 1
    fi
    sleep 0.1
  done
}

wait_for_e2e() {
  deadline=$(( $(date +%s) + $1 ))
  while [ ! -f "$e2e_directory/exit" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      return 1
    fi
    sleep 1
  done
  return 0
}

if [ "$request_auth" -eq 1 ]; then
  launch_e2e auth
  if [ ! -f "$e2e_directory/exit" ]; then
    printf '%s\n' '音声認識の許可ダイアログが出たら「許可」を押してください' >&2
  fi
  if ! wait_for_e2e 60; then
    printf '%s\n' 'WAV E2E: 音声認識の認可要求がタイムアウトしました。認可は確認できていません。' >&2
    exit 1
  fi
  auth_probe_exit=$(cat "$e2e_directory/exit")
  auth_probe_status=$(sed -n 's/^speech-auth status=//p' "$e2e_directory/stderr" | sed -n '1p')
  printf 'WAV E2E auth-probe status=%s exit=%s\n' "${auth_probe_status:-unknown}" "$auth_probe_exit" >&2
  if [ "$auth_probe_exit" != 0 ] || {
    [ "$auth_probe_status" != authorized ] \
      && [ "$auth_probe_status" != 'not-required engine=SpeechAnalyzer' ];
  }; then
    printf '%s\n' 'WAV E2E: テスト用アプリの音声認識認可を確認できませんでした。' >&2
    cat "$e2e_directory/stderr" >&2
    exit 1
  fi
fi

launch_e2e normal
if ! wait_for_e2e 45; then
  printf '%s\n' 'coosenpai-hearing WAV E2E がタイムアウトしました' >&2
  sed -n '1,160p' "$e2e_directory/stderr" >&2
  sed -n '1,160p' "$e2e_directory/stdout" >&2
  exit 1
fi
e2e_exit_status=$(sed -n '1p' "$e2e_directory/exit")
if [ "$e2e_exit_status" -ne 0 ]; then
  printf 'coosenpai-hearing WAV E2E が異常終了しました: status=%s\n' "$e2e_exit_status" >&2
  sed -n '1,160p' "$e2e_directory/stderr" >&2
  sed -n '1,160p' "$e2e_directory/stdout" >&2
  exit 1
fi

speech_auth_status=$(sed -n 's/^speech-auth status=//p' "$e2e_directory/stderr" | sed -n '1p')
if [ -z "$speech_auth_status" ]; then
  printf '%s\n' 'speech-auth の起動ログがありません' >&2
  sed -n '1,120p' "$e2e_directory/stderr" >&2
  exit 1
fi
printf 'WAV E2E speech-auth status=%s\n' "$speech_auth_status" >&2
if [ "$speech_auth_status" != authorized ] \
  && [ "$speech_auth_status" != 'not-required engine=SpeechAnalyzer' ]; then
  printf '%s\n' 'WAV E2E: 音声認識が許可されていないため不合格です。テスト用アプリの認可状態を確認してください。' >&2
  exit 1
fi
python3 - "$e2e_directory/stdout" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    events = [json.loads(line) for line in stream if line.strip()]
event_names = [event.get("event") for event in events]
try:
    ready_index = event_names.index("ready")
    closed_index = event_names.index("closed")
except ValueError as error:
    raise SystemExit(f"ready/closed イベントがありません: {error}")
if ready_index > closed_index:
    raise SystemExit("ready が closed より後に出ています")
if event_names[-1] != "closed":
    raise SystemExit("closed がイベント列の末尾ではありません")
PY
if ! grep -q '"event":"closed"' "$e2e_directory/stdout"; then
  printf '%s\n' 'WAV E2E に closed イベントがありません' >&2
  sed -n '1,120p' "$e2e_directory/stdout" >&2
  exit 1
fi

if ! grep -q '"event":"ready"' "$e2e_directory/stdout"; then
  printf '%s\n' 'WAV E2E に ready イベントがありません' >&2
  sed -n '1,120p' "$e2e_directory/stdout" >&2
  exit 1
fi
dump_path=$(find "$e2e_directory/dump" -type f -name 'segment-microphone-*.wav' -print -quit)
if [ -z "$dump_path" ]; then
  printf '%s\n' 'WAV E2E に追加音声ダンプがありません' >&2
  find "$e2e_directory" -maxdepth 2 -type f -print >&2
  exit 1
fi
python3 - "$e2e_directory/input-two-stereo.wav" "$dump_path" <<'PY'
import struct
import sys

input_path, dump_path = sys.argv[1:]

def wav_format(path):
    with open(path, "rb") as stream:
        if stream.read(12)[:4] != b"RIFF":
            raise SystemExit(f"WAV ヘッダが不正です: {path}")
        while True:
            chunk_header = stream.read(8)
            if len(chunk_header) != 8:
                raise SystemExit(f"fmt チャンクがありません: {path}")
            chunk_id, chunk_size = struct.unpack("<4sI", chunk_header)
            chunk = stream.read(chunk_size)
            if len(chunk) != chunk_size:
                raise SystemExit(f"WAV チャンクが短すぎます: {path}")
            if chunk_id == b"fmt ":
                if len(chunk) < 8:
                    raise SystemExit(f"fmt チャンクが短すぎます: {path}")
                audio_format, channels, sample_rate = struct.unpack_from("<HHI", chunk)
                return audio_format, channels, sample_rate
            if chunk_size % 2:
                stream.read(1)

input_format = wav_format(input_path)
if input_format[1:] != (2, 48000):
    raise SystemExit("WAV E2E の入力が 2ch 48kHz ではありません")
dump_format = wav_format(dump_path)
if dump_format[1:] != (1, 48000):
    raise SystemExit("WAV E2E の追加音声ダンプが mono 48kHz ではありません")
PY

python3 - "$e2e_directory/stdout" "$e2e_directory/stderr" <<'PY'
import json
import re
import sys
import unicodedata

def normalize_text(text):
    return "".join(
        character for character in text
        if not character.isspace() and not unicodedata.category(character).startswith("P")
    )

with open(sys.argv[1], encoding="utf-8") as stream:
    finals = [
        event
        for event in (json.loads(line) for line in stream if line.strip())
        if event.get("event") == "final"
    ]
if len(finals) != 2:
    raise SystemExit(f"Speech 認可済みなのに final が2件ありません: {len(finals)}件")
texts = [event.get("text", "").strip() for event in finals]
if not all(texts):
    raise SystemExit("空の final が含まれています")
if "アルファ" not in texts[0]:
    raise SystemExit(f"一つ目の final にアルファがありません: {texts[0]}")
if "ベータ" not in texts[1]:
    raise SystemExit(f"二つ目の final にベータがありません: {texts[1]}")
if normalize_text(texts[0]) in normalize_text(texts[1]):
    raise SystemExit(f"二つ目の final が前区間の本文を含んでいます: {texts}")
if texts[0] == texts[1]:
    raise SystemExit("二つ目の final が一つ目と同じです")

with open(sys.argv[2], encoding="utf-8") as stream:
    stderr_lines = [line.rstrip("\n") for line in stream]
final_generations = set()
cancel_generations = set()
for line in stderr_lines:
    final_match = re.match(
        r"^recognition-final-received source=microphone generation=(\d+) chars=[1-9][0-9]*$",
        line,
    )
    if final_match:
        final_generations.add(final_match.group(1))
    cancel_match = re.match(
        r"^recognition-session-cancel source=microphone generation=(\d+) reason=",
        line,
    )
    if cancel_match:
        cancel_generations.add(cancel_match.group(1))
overlap = final_generations & cancel_generations
if overlap:
    raise SystemExit(
        f"final を受け取った generation が cancel されています: {sorted(overlap)}"
    )
PY
close_count=$(grep -c '^recognition-segment-close source=microphone' "$e2e_directory/stderr" || true)
if [ "$close_count" -ne 2 ]; then
  printf '二連発話の segment-close が2件ではありません: %s件\n' "$close_count" >&2
  sed -n '1,160p' "$e2e_directory/stderr" >&2
  exit 1
fi
printf '%s\n' 'WAV E2E final-count=2 (一つ目・二つ目)' >&2

speaker_backends=screen-capture-kit
if [ "$process_tap" -eq 1 ]; then speaker_backends="process-tap screen-capture-kit"; fi
if [ "$skip_speaker_e2e" -eq 1 ]; then
  speaker_backends=
  printf '%s\n' 'Speaker E2E: --skip-speaker-e2e により実録音のみ省略（単体・WAV は検証済み）' >&2
fi
speaker_id_backends=$speaker_backends
if [ "$speaker_identification_e2e" -eq 1 ]; then
  speaker_backends=
fi

if [ "$request_auth" -eq 1 ] && [ -n "$speaker_id_backends" ]; then
  case " $speaker_id_backends " in
    *" screen-capture-kit "*)
      printf '%s\n' '画面収録とシステムオーディオ録音の許可ダイアログ、またはシステム設定の「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で HearingE2E を許可してください' >&2
      launch_e2e screen-auth
      if ! wait_for_e2e 75; then
        printf '%s\n' 'WAV E2E: 画面収録の認可要求がタイムアウトしました。HearingE2E を許可してから再実行してください。' >&2
        exit 1
      fi
      screen_auth_status=$(sed -n 's/^screen-capture-auth status=//p' "$e2e_directory/stderr" | tail -n 1)
      printf 'WAV E2E screen-capture-auth status=%s\n' "${screen_auth_status:-unknown}" >&2
      if [ "$screen_auth_status" != granted ]; then
        printf '%s\n' 'WAV E2E: 画面収録が許可されませんでした。システム設定の「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で HearingE2E を有効にし、アプリを再起動してから再実行してください。' >&2
        cat "$e2e_directory/stderr" >&2
        exit 1
      fi
      ;;
  esac
fi
for backend in $speaker_backends; do
  for cycle in 1 2 3; do
    launch_e2e "$backend"
    python3 "$script_dir/Tests/process_tap_e2e.py" "$e2e_directory" normal "$request_auth" "$backend" "$cycle"
    printf 'Speaker E2E backend=%s cycle=%s PASS\n' "$backend" "$cycle" >&2
  done
done

if [ "$speaker_identification_e2e" -eq 1 ]; then
  for backend in $speaker_id_backends; do
    speaker_id_backend=$backend
    launch_e2e speaker-identification
    COOSENPAI_SPEAKER_ID_STATE="$e2e_root/speaker-identification-state-$backend" \
      python3 "$script_dir/Tests/speaker_identification_e2e.py" "$e2e_directory" first "$backend"
    printf 'Speaker identification fixture E2E backend=%s 初回: PASS\n' "$backend" >&2
    launch_e2e speaker-identification
    COOSENPAI_SPEAKER_ID_STATE="$e2e_root/speaker-identification-state-$backend" \
      python3 "$script_dir/Tests/speaker_identification_e2e.py" "$e2e_directory" restart "$backend"
    printf 'Speaker identification fixture E2E backend=%s 再起動: PASS\n' "$backend" >&2
  done
fi

if [ "$speaker_failures" -eq 1 ]; then
  "$script_dir/build.sh" '' --test-speaker-failure >/dev/null
  if [ "$request_auth" -eq 1 ]; then
    launch_e2e failure-auth
    wait_for_e2e 60 || { printf '%s\n' 'Failure fixture authorization timed out' >&2; exit 1; }
    if ! grep -Eq '^speech-auth status=(authorized|not-required engine=SpeechAnalyzer)$' "$e2e_directory/stderr"; then
      cat "$e2e_directory/stderr" >&2
      exit 1
    fi
  fi
  for failure in permission no-output recovered; do
    printf '%s\n' "$failure" > "$e2e_directory/failure-state"
    if [ "$failure" = recovered ]; then
      launch_e2e speaker-recovered
    else
      launch_e2e speaker-failure
    fi
    python3 "$script_dir/Tests/process_tap_e2e.py" "$e2e_directory" "$failure" "$request_auth"
    printf 'Speaker failure E2E scenario=%s PASS\n' "$failure" >&2
  done
fi

if [ -n "${COOSENPAI_HEARING_RESULT_FILE:-}" ]; then
  printf 'PASS\n' >"$COOSENPAI_HEARING_RESULT_FILE"
fi
