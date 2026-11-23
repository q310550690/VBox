#!/usr/bin/env bash
# 打包/开发用（非 Windows 宿主 / MSYS 备用路径）：下载 Temurin JRE 17 到 src-tauri/resources/jvm/jre/
#
# 与 scripts/fetch-jre.ps1 等价：固定 17.0.21+9 + SHA-256 校验，解压压平成
#   src-tauri/resources/jvm/jre/bin/java
# 依赖：curl、python3（或 python；只用标准库 zipfile/hashlib）。
#
# 用法：
#   bash scripts/fetch-jre.sh              # 缺才装
#   bash scripts/fetch-jre.sh --force      # 重装
#   bash scripts/fetch-jre.sh --verify     # 只校验
set -euo pipefail

VERSION="17.0.21+9"
ASSET="OpenJDK17U-jre_x64_windows_hotspot_17.0.21_9.zip"
SHA256="fe4656bd78fb09812dc38c49da777f79cc33e7ee435a254be82f9e7f40d9d117"
FORCE=0
VERIFY=0
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    --verify) VERIFY=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done

PY=$(command -v python3 || command -v python || true)
[ -n "$PY" ] || { echo "需要 python3/python（只用标准库）" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="$ROOT/.tools/cache"
DEST="$ROOT/src-tauri/resources/jvm"
JRE="$DEST/jre"
MARKER="$DEST/marker.json"
[ -d "$JRE/bin" ] && command -v "$JRE/bin/java" >/dev/null 2>&1 && JAVA="$JRE/bin/java" || JAVA="$JRE/bin/java.exe"
mkdir -p "$CACHE" "$DEST"

sha_of() { "$PY" - "$1" <<'PYEOF'
import hashlib,sys
h=hashlib.sha256()
with open(sys.argv[1],'rb') as f:
    for chunk in iter(lambda: f.read(1<<20), b''):
        h.update(chunk)
print(h.hexdigest())
PYEOF
}

installed() {
  [ -f "$JAVA" ] && [ -f "$MARKER" ] || return 1
  "$PY" - "$MARKER" "$SHA256" <<'PYEOF'
import json,sys
try:
    m=json.load(open(sys.argv[1],encoding='utf-8'))
except Exception:
    sys.exit(1)
sys.exit(0 if m.get('sha256')==sys.argv[2] else 1)
PYEOF
}

if [ "$VERIFY" = 1 ]; then
  installed || { echo "JRE 未就位：$JRE（先跑本脚本不带 --verify）" >&2; exit 1; }
  echo "JRE 已就位：$JRE"; "$JAVA" -version 2>&1 | head -1; exit 0
fi

if installed && [ "$FORCE" = 0 ]; then
  echo "已存在：$JRE（--force 可重装）"; exit 0
fi

TUNA="https://mirrors.tuna.tsinghua.edu.cn/Adoptium/17/jre/x64/windows/$ASSET"
DIRECT="https://github.com/adoptium/temurin17-binaries/releases/download/jdk-${VERSION/+/%2B}/$ASSET"
API="https://api.adoptium.net/v3/binary/version/jdk-${VERSION/+/%2B}/windows/x64/jre/hotspot/normal/eclipse"
ZIP="$CACHE/jre-${VERSION/+/_}-win-x64.zip"

ok=0
for url in "$TUNA" "$API" "$DIRECT"; do
  if [ -f "$ZIP" ] && [ "$(sha_of "$ZIP")" = "$SHA256" ]; then ok=1; break; fi
  rm -f "$ZIP"
  echo "下载 $url"
  if curl -sSL --fail --retry 3 --retry-delay 2 --max-time 1800 -A 'Mozilla/5.0' -o "$ZIP" "$url"; then
    if [ "$(sha_of "$ZIP")" = "$SHA256" ]; then ok=1; break; fi
    echo "SHA-256 不匹配，换下一个镜像" >&2
  fi
done
[ "$ok" = 1 ] || { echo "JRE 包下载失败或校验不通过" >&2; exit 1; }
echo "JRE 包 SHA-256 校验通过（$(du -h "$ZIP" | cut -f1)）"

TMP="$DEST/jre.tmp"
rm -rf "$TMP"; mkdir -p "$TMP"
"$PY" - "$ZIP" "$TMP" <<'PYEOF'
import os,sys,zipfile
src,dst=sys.argv[1],sys.argv[2]
n=0
with zipfile.ZipFile(src) as z:
    for info in z.infolist():
        if info.is_dir():
            continue
        rel=info.filename.split('/',1)[1] if '/' in info.filename else info.filename
        if not rel:
            continue
        target=os.path.join(dst,rel.replace('/',os.sep))
        os.makedirs(os.path.dirname(target),exist_ok=True)
        with z.open(info) as fin, open(target,'wb') as fout:
            fout.write(fin.read())
        n+=1
print(f"解压 {n} 个文件")
PYEOF

[ -f "$TMP/bin/java.exe" ] || [ -f "$TMP/bin/java" ] || { echo "JRE 包结构异常：没有 bin/java(.exe)" >&2; exit 1; }
rm -rf "$JRE"; mv "$TMP" "$JRE"

"$JAVA" -version 2>&1 | head -1
"$PY" - "$MARKER" "$ASSET" "$SHA256" "$VERSION" <<'PYEOF'
import json,sys,datetime,subprocess
marker,asset,sha,version=sys.argv[1:5]
java_exe=marker.replace('marker.json','jre/bin/java.exe')
try:
    out=subprocess.run([java_exe,'-version'],capture_output=True,text=True).stderr.strip().splitlines()
    ver=next((l.split('"')[1] for l in out if '"' in l),'')
except Exception:
    ver=''
payload={"mode":"jre","asset":asset,"sha256":sha,"version":version,"javaVersion":ver,
         "arch":"x64","source":"adoptium-temurin",
         "stagedAt":datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}
with open(marker,'w',encoding='utf-8',newline='\n') as f:
    f.write(json.dumps(payload,ensure_ascii=False,indent=2)+"\n")
print("marker.json 已写入")
PYEOF
echo "完成：src-tauri/resources/jvm/jre（$(du -sh "$JRE" | cut -f1)）"
