#!/usr/bin/env bash
# 把已收好 dylib 的 .app 打成 .dmg（带 /Applications 快捷方式）
#
# 用法：
#   ./scripts/package-dmg-macos.sh "src-tauri/target/release/bundle/macos/VBox.app" VBox-macos.dmg
set -euo pipefail

APP="${1:?用法: $0 /path/to/VBox.app [输出.dmg]}"
OUT="${2:-VBox-macos.dmg}"

if [[ ! -d "$APP" ]]; then
  echo "[package-dmg-macos] 找不到 .app：$APP" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
hdiutil create -volname "VBox" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
echo "[package-dmg-macos] 完成：$OUT"
