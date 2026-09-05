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

# 署名 identity を選ぶ。アドホック署名はビルドごとに cdhash が変わり、画面収録や
# アクセシビリティの許可が旧ビルドに残ったまま効かなくなる（システム設定ではオンに見える）。
# Apple 発行の証明書なら許可条件が識別子とチームに基づくため、ビルドを重ねても許可が持続する。
identities=$(security find-identity -v -p codesigning 2>/dev/null || true)
pick_identity() {
  printf '%s\n' "$identities" | grep -o "\"$1[^\"]*\"" | head -n 1 | tr -d '"'
}
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  signing_identity=$APPLE_SIGNING_IDENTITY
elif identity=$(pick_identity 'Developer ID Application:') && [ -n "$identity" ]; then
  signing_identity=$identity
elif identity=$(pick_identity 'Apple Development:') && [ -n "$identity" ]; then
  signing_identity=$identity
else
  signing_identity=-
fi

if [ "${1:-}" != "--skip-build" ]; then
  if [ ! -d "$desktop_dir/node_modules" ]; then
    printf 'install-local: デスクトップの依存を入れます\n'
    (cd "$desktop_dir" && npm ci)
  fi
  if [ "$signing_identity" = - ]; then
    printf 'install-local: ビルドします（アドホック署名）\n'
  else
    printf 'install-local: ビルドします（署名: %s）\n' "$signing_identity"
  fi
  (cd "$desktop_dir" && APPLE_SIGNING_IDENTITY="$signing_identity" npm run tauri -- build --bundles app)
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

# 署名者が前回の配置と変わるときだけ許可をリセットし、起動後の要求ダイアログで許可し直せるようにする。
# 同じ Apple 発行の証明書で署名している限り、許可はビルドを跨いで持続する。
signer_of() {
  codesign -dvv "$1" 2>&1 | grep -m 1 '^Authority=' || printf 'adhoc\n'
}
new_signer=$(signer_of "$bundle")
previous_signer=
[ -d "$dest" ] && previous_signer=$(signer_of "$dest")
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist")
if [ "$new_signer" = adhoc ] || [ "$new_signer" != "$previous_signer" ]; then
  for service in ScreenCapture Accessibility; do
    printf 'install-local: %s の許可をリセットします (%s)\n' "$service" "$bundle_id"
    tccutil reset "$service" "$bundle_id"
  done
else
  printf 'install-local: 署名者が同じため許可を保持します (%s)\n' "$new_signer"
fi

if [ -d "$dest" ]; then
  printf 'install-local: 旧アプリを削除します\n'
  rm -rf "$dest"
fi

printf 'install-local: 配置します\n'
ditto "$bundle" "$dest"
codesign -v "$dest"
open "$dest"
printf 'install-local: 完了\n'
