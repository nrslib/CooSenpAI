#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
unit_only=0
input_wav="$script_dir/Tests/Fixtures/two-utterances-microphone.wav"
case "$#:$*" in
  0:) ;;
  1:--unit-only) unit_only=1 ;;
  2:--wav\ *) input_wav=$2 ;;
  *) printf '使い方: %s [--unit-only | --wav WAV_PATH]\n' "$0" >&2; exit 2 ;;
esac
if [ "$unit_only" -eq 0 ] && [ ! -f "$input_wav" ]; then
  printf '検証 WAV がありません: %s\n' "$input_wav" >&2
  exit 2
fi

mkdir -p "$repository_dir/target/helpers"
temporary_path=$(mktemp "$repository_dir/target/helpers/coosenpai-speech-test.XXXXXX")
module_cache_dir="$repository_dir/target/helpers/speech-test-module-cache"
e2e_directory=
cleanup() {
  if [ -n "$e2e_directory" ]; then
    for engine in analyzer sf; do
      pid_path="$e2e_directory/$engine/runner.pid"
      if [ -f "$pid_path" ]; then
        runner_pid=$(cat "$pid_path")
        case "$runner_pid" in
          ''|*[!0-9]*) ;;
          *) if kill -0 "$runner_pid" 2>/dev/null; then kill -TERM "$runner_pid"; fi ;;
        esac
      fi
    done
  fi
  rm -f "$temporary_path"
}
trap cleanup EXIT
trap 'exit 143' HUP TERM
trap 'exit 130' INT

mkdir -p "$module_cache_dir"
swiftc -target "$(uname -m)-apple-macosx13.0" -O \
  -module-cache-path "$module_cache_dir" \
  "$script_dir/../hearing-helper/Sources/audio_buffer_copy.swift" \
  "$script_dir/../hearing-helper/Sources/audio_stats.swift" \
  "$script_dir/../hearing-helper/Sources/wav_input.swift" \
  "$script_dir/Sources/audio_queue.swift" \
  "$script_dir/Sources/wav_dump.swift" \
  "$script_dir/Sources/wav_file.swift" \
  "$script_dir/Sources/transcript_accumulator.swift" \
  "$script_dir/Sources/recognition_session.swift" \
  "$script_dir/Sources/audio_converter.swift" \
  "$script_dir/Sources/speech_analyzer.swift" \
  "$script_dir/Sources/speech_recognizer.swift" \
  "$script_dir/Sources/speech_engine.swift" \
  "$script_dir/Tests/transcript_accumulator_test.swift" \
  "$script_dir/Tests/speech_engine_test.swift" \
  "$script_dir/Tests/early_finish_test.swift" \
  "$script_dir/Tests/recognition_session_test.swift" \
  "$script_dir/Tests/audio_converter_test.swift" \
  "$script_dir/Tests/wav_input_test.swift" \
  "$script_dir/Tests/wav_dump_test.swift" \
  "$script_dir/Tests/test_main.swift" \
  -o "$temporary_path"
if "$temporary_path" --expect-failure 2>/dev/null; then
  printf '%s\n' '最適化ビルドでテストの失敗が検出されませんでした' >&2
  exit 1
else
  failure_status=$?
  if [ "$failure_status" -ne 1 ]; then
    printf 'テストの失敗検出が異常終了しました: %s\n' "$failure_status" >&2
    exit 1
  fi
fi
"$temporary_path"

if [ "$unit_only" -eq 0 ]; then
  "$script_dir/build.sh" >/dev/null
  e2e_directory=$(mktemp -d "$repository_dir/target/helpers/speech-e2e.XXXXXX")
  cp "$input_wav" "$e2e_directory/two-utterances.wav"
  e2e_app="$repository_dir/target/helpers/SpeechE2E.app"
  mkdir -p "$e2e_app/Contents/MacOS"
  cat > "$e2e_app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.nrslib.coosenpai.speech-e2e</string>
  <key>CFBundleName</key><string>SpeechE2E</string>
  <key>CFBundleExecutable</key><string>run</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>WAV fixture の文字起こしを端末内で検証するために音声認識を使用します。</string>
</dict></plist>
PLIST
  swiftc -target "$(uname -m)-apple-macosx13.0" -O -module-cache-path "$module_cache_dir" \
    "$script_dir/Tests/wav_test_launcher.swift" -o "$e2e_app/Contents/MacOS/run"
  codesign --force --sign - "$e2e_app" >/dev/null
  status=0
  engines=sf
  if [ "$(sw_vers -productVersion | cut -d. -f1)" -ge 26 ]; then
    engines="analyzer sf"
  fi
  for engine in $engines; do
    early_output="$e2e_directory/$engine/early-finish"
    open -n -W -a "$e2e_app" --args "$(command -v python3)" "$script_dir/Tests/early_finish_test.py" \
      --binary "$temporary_path" --engine "$engine" --output "$early_output" || status=$?
    if [ ! -f "$early_output/exit-status" ] || [ "$(cat "$early_output/exit-status")" != 0 ]; then
      printf '先行 finish テスト (%s) が失敗しました: %s\n' "$engine" "$early_output" >&2
      status=1
    else
      printf '先行 finish テスト (%s): 3 cases PASS\n' "$engine"
    fi
  done
  for engine in $engines; do
    printf 'WAV 実認識ログ (%s): %s\n' "$engine" "$e2e_directory/$engine"
    open -n -W -a "$e2e_app" --args "$(command -v python3)" "$script_dir/Tests/wav_recognition_test.py" \
      --helper "$repository_dir/target/helpers/coosenpai-speech" \
      --wav "$e2e_directory/two-utterances.wav" \
      --output "$e2e_directory/$engine" --runs 5 --engine "$engine" || status=$?
    if [ ! -f "$e2e_directory/$engine/exit-status" ]; then
      printf 'WAV テスト (%s) の終了結果がありません\n' "$engine" >&2
      status=1
    elif [ "$(cat "$e2e_directory/$engine/exit-status")" != 0 ]; then
      status=1
    fi
    if [ -f "$e2e_directory/$engine/runner.log" ]; then
      sed -n '1,120p' "$e2e_directory/$engine/runner.log"
    fi
  done
  exit "$status"
fi
