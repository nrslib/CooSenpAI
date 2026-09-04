#!/bin/sh
# クローンした環境でローカルビルドを /Applications へ入れ直す。
# Developer ID が無くてもアドホック署名で動く（配布は tools/release.sh を使う）。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_name="CooSenpAI.app"
bundle="$repo_root/target/release/bundle/macos/$app_name"
dest="/Applications/$app_name"

for command_name in npm cargo codesign ditto tccutil; do
  command -v "$command_name" >/dev/null 2>&1 || { printf 'install-local: %s が見つかりません\n' "$command_name" >&2; exit 1; }
done

desktop_dir="$repo_root/apps/desktop"

if [ "${1:-}" != "--skip-build" ]; then
  if [ ! -d "$desktop_dir/node_modules" ]; then
    printf 'install-local: デスクトップの依存を入れます\n'
    (cd "$desktop_dir" && npm ci)
  fi
  printf 'install-local: ビルドします（アドホック署名）\n'
  (cd "$desktop_dir" && APPLE_SIGNING_IDENTITY=- npm run tauri build)
fi

[ -d "$bundle" ] || { printf 'install-local: ビルド成果物がありません: %s\n' "$bundle" >&2; exit 1; }

if ! codesign -v "$bundle" >/dev/null 2>&1; then
  printf 'install-local: アドホック署名を付けます\n'
  codesign --force --deep -s - "$bundle"
fi

if pgrep -f "$dest/Contents/MacOS/" >/dev/null 2>&1; then
  printf 'install-local: 起動中の本体を終了します\n'
  osascript -e 'tell application "CooSenpAI" to quit' >/dev/null 2>&1 || true
  sleep 3
fi

# アドホック署名はビルドごとに cdhash が変わり、画面収録やアクセシビリティの許可が
# 旧ビルドに残ったまま効かなくなる（システム設定ではオンに見える）。
# 許可をリセットしておき、起動後の要求ダイアログで許可し直せるようにする。
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist")
for service in ScreenCapture Accessibility; do
  printf 'install-local: %s の許可をリセットします (%s)\n' "$service" "$bundle_id"
  tccutil reset "$service" "$bundle_id"
done

if [ -d "$dest" ]; then
  printf 'install-local: 旧アプリを削除します\n'
  rm -rf "$dest"
fi

printf 'install-local: 配置します\n'
ditto "$bundle" "$dest"
codesign -v "$dest"
open "$dest"
printf 'install-local: 完了\n'
