# 一键准备本地原生依赖（首次克隆后执行一次即可）
#
# 会依次拉取：
#   1. src-tauri/resources/libmpv-2.dll        （播放内核，约 121 MB，GH 代理 + 分段并发）
#   2. src-tauri/resources/ffmpeg/ffmpeg.exe   （保存兜底，约 100 MB，固定版本 + SHA-256）
#   3. src-tauri/resources/python/             （.py spider，约 55 MB，嵌入式 CPython + 依赖）
#   4. src-tauri/resources/node/node.exe       （drpy，约 80 MB）
#
# 这些都体积大、不进 git，但会随安装包分发（内置哪些可在打包时用 --bundle 选择）；
# 普通用户安装 VBox 后不需要任何操作。
#
# 用法（在仓库根目录）：
#   pwsh -File scripts/fetch-tools.ps1
#   pwsh -File scripts/fetch-tools.ps1 -Force
#   pwsh -File scripts/fetch-tools.ps1 -SkipRuntime   # 只拉 libmpv + ffmpeg
param(
    [switch]$Force,
    [switch]$SkipRuntime,
    [string]$LibmpvProxy = "https://ghproxy.net/"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$libmpv = Join-Path $root 'src-tauri/resources/libmpv-2.dll'
$ffmpeg = Join-Path $root 'src-tauri/resources/ffmpeg/ffmpeg.exe'
$python = Join-Path $root 'src-tauri/resources/python/python.exe'
$node = Join-Path $root 'src-tauri/resources/node/node.exe'

if ((Test-Path $libmpv) -and -not $Force) {
    Write-Host "[跳过] libmpv-2.dll 已存在"
} else {
    & (Join-Path $PSScriptRoot 'fetch-libmpv.ps1') -ProxyPrefix $LibmpvProxy
}

if ((Test-Path $ffmpeg) -and -not $Force) {
    Write-Host "[跳过] ffmpeg.exe 已存在"
} else {
    & (Join-Path $PSScriptRoot 'fetch-ffmpeg.ps1') -Force:$Force
}

if (-not $SkipRuntime) {
    if ((Test-Path $python) -and -not $Force) {
        Write-Host "[跳过] python.exe 已存在"
    } else {
        & (Join-Path $PSScriptRoot 'fetch-python.ps1') -Force:$Force
    }
    if ((Test-Path $node) -and -not $Force) {
        Write-Host "[跳过] node.exe 已存在"
    } else {
        & (Join-Path $PSScriptRoot 'fetch-node.ps1') -Force:$Force
    }
}

Write-Host "原生依赖就绪："
Write-Host "  src-tauri/resources/libmpv-2.dll"
Write-Host "  src-tauri/resources/ffmpeg/ffmpeg.exe"
if (-not $SkipRuntime) {
    Write-Host "  src-tauri/resources/python/"
    Write-Host "  src-tauri/resources/node/node.exe"
}
Write-Host "现在可以 npm run tauri dev；打包用 npm run tauri:build（可 --bundle 选择内置哪些）。"
