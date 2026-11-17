#!/usr/bin/env bash
# 把 macOS .app 里的 libmpv.dylib 及其非系统依赖一起收进 app，并改成 @loader_path 引用，
# 这样用户机器上不需要安装 Homebrew / mpv。
#
# 用法（在 tauri build --bundles app 之后执行）：
#   ./scripts/bundle-runtime-macos.sh "src-tauri/target/release/bundle/macos/VBox.app"
set -euo pipefail

APP="${1:?用法: $0 /path/to/VBox.app}"
RES="$APP/Contents/Resources/resources"
LIB="$RES/libmpv.dylib"

if [[ ! -f "$LIB" ]]; then
  echo "[bundle-runtime-macos] 找不到 $LIB（先跑 fetch-libmpv-macos.sh + tauri build）" >&2
  exit 1
fi

is_system_dep() {
  case "$1" in
    /System/*|/usr/lib/*|/Library/Frameworks/*) return 0 ;;
    *) return 1 ;;
  esac
}

# 读出一个 dylib 的 LC_RPATH 目录（Homebrew 常把依赖写成 @rpath/xxx.dylib）
rpaths_of() {
  otool -l "$1" 2>/dev/null | awk '
    $1 == "cmd" && $2 == "LC_RPATH" { want = 1; next }
    want && $1 == "path" { print $2; want = 0 }
  '
}

process_library() {
  local file="$1"
  # 注意：dep 必须是 local。否则递归处理依赖时会改到父级循环变量，
  # 导致后面的 install_name_tool -change 收到空字符串（表现为部分依赖改不掉）。
  local dep
  local base
  base="$(basename "$file")"

  chmod u+w "$file" 2>/dev/null || true
  install_name_tool -id "@loader_path/$base" "$file" 2>/dev/null || true

  local deps
  deps="$(otool -L "$file" | tail -n +2 | awk '{print $1}')"
  while IFS= read -r dep; do
    [[ -z "$dep" ]] && continue
    is_system_dep "$dep" && continue
    [[ "$dep" == @loader_path/* || "$dep" == @executable_path/* ]] && continue

    local dep_base src=""
    dep_base="$(basename "$dep")"

    if [[ "$dep" == @rpath/* ]]; then
      # @rpath/xxx：从自身所在目录、LC_RPATH、Homebrew 常见目录里找同名 dylib
      local dir
      while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        if [[ -f "$dir/$dep_base" ]]; then
          src="$dir/$dep_base"
          break
        fi
      done < <(
        {
          printf '%s\n' "$RES" "$(dirname "$file")" /opt/homebrew/lib /usr/local/lib
          rpaths_of "$file"
        } | sed 's|^@loader_path|'"$(dirname "$file")"'|'
      )
    else
      src="$dep"
    fi

    if [[ -z "$src" || ! -f "$src" ]]; then
      echo "[bundle-runtime-macos] 警告：找不到依赖 $dep（来自 $base），跳过" >&2
      continue
    fi

    local target="$RES/$dep_base"
    if [[ ! -f "$target" ]]; then
      cp -f "$src" "$target"
      chmod u+w "$target"
      process_library "$target"
    fi
    if ! install_name_tool -change "$dep" "@loader_path/$dep_base" "$file"; then
      echo "[bundle-runtime-macos] 警告：改写依赖失败：$dep（来自 $base）" >&2
    fi
  done <<< "$deps"
}

process_library "$LIB"
chmod u+w "$RES"/*.dylib 2>/dev/null || true

# 未做 Developer ID 签名时至少 ad-hoc 签名，保证本机/右键打开可运行
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 \
    || echo "[bundle-runtime-macos] ad-hoc 签名失败（不影响产物，可后续正式签名）"
fi

echo "[bundle-runtime-macos] 完成：$RES"
ls -1 "$RES"
