#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
temporary_path=$(mktemp /tmp/coosenpai-hearing-test.XXXXXX)
temporary_object_path="${temporary_path}.o"
module_cache_dir="$script_dir/../../target/helpers/test-module-cache"
e2e_root="$repository_dir/target/helpers/hearing-e2e"
e2e_directory="$e2e_root/data"
e2e_config="$e2e_root/config"
e2e_app="$repository_dir/target/helpers/HearingE2E.app"
runner_pid=

stop_e2e_runner() {
  if [ -f "$e2e_directory/runner.pid" ]; then
    runner_pid=$(sed -n '1p' "$e2e_directory/runner.pid")
    if [ -n "$runner_pid" ] && kill -0 "$runner_pid" 2>/dev/null; then
      kill "$runner_pid" 2>/dev/null || true
      runner_deadline=$(( $(date +%s) + 5 ))
      while kill -0 "$runner_pid" 2>/dev/null \
        && [ "$(date +%s)" -lt "$runner_deadline" ]; do
        sleep 1
      done
    fi
  fi
}

cleanup() {
  stop_e2e_runner
  rm -f "$temporary_path" "$temporary_object_path"
  rm -rf "$e2e_directory"
  rm -f "$e2e_config"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$module_cache_dir"
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
clang -isysroot "$sdk_path" -fobjc-arc -c \
  "$script_dir/Sources/audio_tap_installer.m" -o "$temporary_object_path"
swiftc \
  -parse-as-library \
  -module-cache-path "$module_cache_dir" \
  -import-objc-header "$script_dir/Sources/audio_tap_installer.h" \
  "$script_dir/Sources/audio_stats.swift" \
  "$script_dir/Sources/audio_scaling.swift" \
  "$script_dir/Sources/audio_buffer_copy.swift" \
  "$script_dir/Sources/audio_conversion.swift" \
  "$script_dir/Sources/audio_input_processing.swift" \
  "$script_dir/Sources/recognition_state.swift" \
  "$script_dir/Sources/segment_controller.swift" \
  "$script_dir/Sources/voice_activity.swift" \
  "$script_dir/Sources/wav_input.swift" \
  "$script_dir/Sources/appended_audio_dump.swift" \
  "$script_dir/Sources/main.swift" \
  "$temporary_object_path" \
  "$script_dir/Tests/audio_stats_test.swift" \
  "$script_dir/Tests/audio_scaling_test.swift" \
  "$script_dir/Tests/audio_buffer_copy_test.swift" \
  "$script_dir/Tests/audio_conversion_test.swift" \
  "$script_dir/Tests/voice_activity_test.swift" \
  "$script_dir/Tests/recognition_state_test.swift" \
  "$script_dir/Tests/wav_input_test.swift" \
  "$script_dir/Tests/appended_audio_dump_test.swift" \
  -o "$temporary_path"
"$temporary_path"

"$script_dir/build.sh" >/dev/null
mkdir -p "$e2e_root"
stop_e2e_runner
rm -rf "$e2e_directory"
rm -f "$e2e_config"
mkdir -p "$e2e_directory"
say -v Kyoko -o "$e2e_directory/input.aiff" \
  'これはアルファの発話です'
say -v Kyoko -o "$e2e_directory/input-b.aiff" \
  'これはベータの発話です'
afconvert -f WAVE -d LEI16@48000 -c 1 \
  "$e2e_directory/input.aiff" "$e2e_directory/input.wav"
afconvert -f WAVE -d LEI16@48000 -c 1 \
  "$e2e_directory/input-b.aiff" "$e2e_directory/input-b.wav"
python3 - "$e2e_directory/input.wav" "$e2e_directory/input-b.wav" \
  "$e2e_directory/input-two-stereo.wav" <<'PY'
import sys
import wave

first_path, second_path, output_path = sys.argv[1:]
with wave.open(first_path, "rb") as first, wave.open(second_path, "rb") as second:
    first_params = first.getparams()
    second_params = second.getparams()
    if (
        first_params.nchannels,
        first_params.sampwidth,
        first_params.framerate,
        first_params.comptype,
    ) != (
        second_params.nchannels,
        second_params.sampwidth,
        second_params.framerate,
        second_params.comptype,
    ):
        raise SystemExit("二連発話 WAV のフォーマットが一致しません")
    if first_params.nchannels != 1:
        raise SystemExit("二連発話 WAV の入力は mono である必要があります")
    first_frames = first.readframes(first_params.nframes)
    second_frames = second.readframes(second_params.nframes)

def stereoize(frames, sample_width):
    return b"".join(
        frames[offset:offset + sample_width] * 2
        for offset in range(0, len(frames), sample_width)
    )

silence_frames = int(first_params.framerate * 1.5)
silence = b"\0" * silence_frames * 2 * first_params.sampwidth
with wave.open(output_path, "wb") as output:
    output.setnchannels(2)
    output.setsampwidth(first_params.sampwidth)
    output.setframerate(first_params.framerate)
    output.writeframes(
        stereoize(first_frames, first_params.sampwidth)
        + silence
        + stereoize(second_frames, first_params.sampwidth)
        + silence
    )
PY
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
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>音声認識の E2E テストにマイクを使用します。</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>音声認識の E2E テストに音声認識を使用します。</string>
</dict>
</plist>
PLIST
cat > "$e2e_app/Contents/MacOS/runner.sh" <<'RUN'
#!/bin/sh
set -u

config_path=$(CDPATH= cd -- "$(dirname -- "$0")/../../../hearing-e2e" && pwd)/config
config_value() {
  sed -n "s/^$1=//p" "$config_path" | sed -n '1p'
}
helper_path=$(config_value helper_path)
mode=$(config_value mode)
input_wav=$(config_value input_wav)
dump_dir=$(config_value dump_dir)
stdin_path=$(config_value stdin_path)
stdout_path=$(config_value stdout_path)
stderr_path=$(config_value stderr_path)
exit_path=$(config_value exit_path)
runner_pid_path=$(config_value runner_pid_path)
printf '%s\n' "$$" > "$runner_pid_path"
helper_pid=
stdin_pid=

cleanup_children() {
  if [ -n "$helper_pid" ] && kill -0 "$helper_pid" 2>/dev/null; then
    kill "$helper_pid" 2>/dev/null || true
  fi
  if [ -n "$stdin_pid" ] && kill -0 "$stdin_pid" 2>/dev/null; then
    kill "$stdin_pid" 2>/dev/null || true
  fi
}
trap 'cleanup_children; exit 143' HUP INT TERM

rm -f "$stdin_path" "$stdout_path" "$stderr_path" "$exit_path"
mkfifo "$stdin_path"
tail -f /dev/null > "$stdin_path" &
stdin_pid=$!

if [ "$mode" = auth ]; then
  "$helper_path" \
    --locale ja-JP \
    --input-device default \
    --sources microphone \
    --debug-request-auth \
    < "$stdin_path" \
    > "$stdout_path" \
    2> "$stderr_path" &
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

if wait "$helper_pid"; then
  exit_status=0
else
  exit_status=$?
fi
if [ -n "$stdin_pid" ] && kill -0 "$stdin_pid" 2>/dev/null; then
  kill "$stdin_pid" 2>/dev/null || true
fi
wait "$stdin_pid" 2>/dev/null || true
helper_pid=
stdin_pid=
printf '%s\n' "$exit_status" > "$exit_path"
exit 0
RUN
chmod 755 "$e2e_app/Contents/MacOS/runner.sh"
cat > "$e2e_directory/e2e-launcher.c" <<'C'
#include <libgen.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    char executable[PATH_MAX];
    if (argc < 1 || realpath(argv[0], executable) == NULL) {
        return 127;
    }
    char *directory = dirname(executable);
    char script[PATH_MAX];
    if (snprintf(script, sizeof(script), "%s/runner.sh", directory)
        >= (int)sizeof(script)) {
        return 127;
    }
    execl("/bin/sh", "sh", script, (char *)NULL);
    return 127;
}
C
clang "$e2e_directory/e2e-launcher.c" \
  -o "$e2e_app/Contents/MacOS/run"
codesign --force --deep --sign - "$e2e_app" >/dev/null

write_e2e_config() {
  cat > "$e2e_config" <<EOF
helper_path=$repository_dir/target/helpers/coosenpai-hearing
mode=$1
input_wav=$e2e_directory/input-two-stereo.wav
dump_dir=$e2e_directory/dump
stdin_path=$e2e_directory/stdin
stdout_path=$e2e_directory/stdout
stderr_path=$e2e_directory/stderr
exit_path=$e2e_directory/exit
runner_pid_path=$e2e_directory/runner.pid
EOF
}

launch_e2e() {
  stop_e2e_runner
  write_e2e_config "$1"
  rm -f \
    "$e2e_directory/stdin" \
    "$e2e_directory/stdout" \
    "$e2e_directory/stderr" \
    "$e2e_directory/exit" \
    "$e2e_directory/runner.pid"
  open -n -a "$e2e_app" >/dev/null 2>&1 || true
  sleep 1
  if [ ! -f "$e2e_directory/runner.pid" ]; then
    printf '%s\n' 'LaunchServices が利用できないため、同じ app runner を直接実行します' >&2
    "$e2e_app/Contents/MacOS/run" >/dev/null 2>&1 &
  fi
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

launch_e2e auth
if [ ! -f "$e2e_directory/exit" ]; then
  printf '%s\n' '音声認識の許可ダイアログが出たら「許可」を押してください' >&2
fi
auth_deadline=$(( $(date +%s) + 60 ))
while [ ! -f "$e2e_directory/exit" ] \
  && [ "$(date +%s)" -lt "$auth_deadline" ]; do
  sleep 1
done
if [ -f "$e2e_directory/exit" ]; then
  auth_probe_status=$(sed -n 's/^speech-auth status=//p' "$e2e_directory/stderr" | sed -n '1p')
  if [ -z "$auth_probe_status" ]; then
    auth_probe_status=unknown
  fi
  printf 'WAV E2E auth-probe status=%s\n' "$auth_probe_status" >&2
else
  printf '%s\n' 'WAV E2E の認可プローブがタイムアウトしました。dump-only にフォールバックします' >&2
  stop_e2e_runner
fi

if [ "${auth_probe_status:-}" = denied ]; then
  printf '%s\n' '認可が denied の場合は tccutil reset SpeechRecognition dev.nrslib.coosenpai.hearing-e2e を実行してから再試行してください' >&2
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

if [ "$speech_auth_status" = authorized ]; then
  python3 - "$e2e_directory/stdout" "$e2e_directory/stderr" <<'PY'
import json
import re
import sys

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
        r"^recognition-task-cancel source=microphone generation=(\d+) reason=",
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
else
  printf '%s\n' 'WAV E2E final=skipped (Speech 認可が必要、dump-only で波形検証)' >&2
fi
