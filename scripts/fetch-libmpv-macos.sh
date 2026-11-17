#!/usr/bin/env bash
# 准备 macOS 的 libmpv 动态库：复制到 src-tauri/resources/libmpv.dylib
#
# 来源优先级：
#   1. $VBOX_LIBMPV（指向现成的 libmpv*.dylib）
#   2. $MPV_PREFIX/lib/libmpv*.dylib
#   3. brew --prefix mpv（brew install mpv）
#   4. /opt/homebrew/lib、/usr/local/lib
#
# 用法：./scripts/fetch-libmpv-macos.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST_DIR="$ROOT/src-tauri/resources"
DEST="$DEST_DIR/libmpv.dylib"
mkdir -p "$DEST_DIR"

candidates=()
if [[ -n "${VBOX_LIBMPV:-}" ]]; then
  candidates+=("$VBOX_LIBMPV")
fi
if [[ -n "${MPV_PREFIX:-}" ]]; then
  candidates+=("$MPV_PREFIX/lib/libmpv.2.dylib" "$MPV_PREFIX/lib/libmpv.dylib")
fi
if command -v brew >/dev/null 2>&1; then
  brew_prefix="$(brew --prefix mpv 2>/dev/null || true)"
  if [[ -n "$brew_prefix" ]]; then
    candidates+=("$brew_prefix/lib/libmpv.2.dylib" "$brew_prefix/lib/libmpv.dylib")
  fi
fi
candidates+=(
  "/opt/homebrew/lib/libmpv.2.dylib"
  "/opt/homebrew/lib/libmpv.dylib"
  "/usr/local/lib/libmpv.2.dylib"
  "/usr/local/lib/libmpv.dylib"
)

src=""
for candidate in "${candidates[@]}"; do
  if [[ -f "$candidate" ]]; then
    src="$candidate"
    break
  fi
done

if [[ -z "$src" ]]; then
  echo "[fetch-libmpv-macos] 没找到 libmpv 动态库。" >&2
  echo "  先执行: brew install mpv" >&2
  echo "  或指定: VBOX_LIBMPV=/path/to/libmpv.dylib ./scripts/fetch-libmpv-macos.sh" >&2
  exit 1
fi

cp -f "$src" "$DEST"
# 统一 install name，方便后面 bundle-runtime 脚本改成 @loader_path
install_name_tool -id "@rpath/libmpv.dylib" "$DEST" 2>/dev/null || true
echo "[fetch-libmpv-macos] $src -> $DEST"
