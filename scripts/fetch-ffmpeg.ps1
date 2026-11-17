# 下载 FFmpeg（Windows x64 静态构建）到 src-tauri/resources/ffmpeg/
#
# 目的：发布包「自带 FFmpeg」，用户安装后无需再自己装（SAMPLE-AES 解密、
# 分离音轨合并、DASH/MP4 兜底保存需要它）。
#
# 版本、大小与 SHA-256 固定（与 src-tauri/src/ffmpeg_prefs.rs 的 DOWNLOAD_* 常量一致），
# 保证下载内容可校验、可复现。国内直连 GitHub 常常不可用，所以默认走 gh-proxy 镜像，
# 失败自动换 ghproxy.net / 直连；大文件按 8 段 Range 并发下载（单连接常被限速）。
#
# 用法（在仓库根目录）：
#   pwsh -File scripts/fetch-ffmpeg.ps1
#   pwsh -File scripts/fetch-ffmpeg.ps1 -Force        # 已存在也重新下载
#   pwsh -File scripts/fetch-ffmpeg.ps1 -Url "<自建镜像 zip 地址>"
param(
    [string]$Url = "",
    [string]$Sha256 = "ff646d864bd9b0fa6e0be55acaa2e1f0ad7210c309325f13853c89bf7afd79e7",
    [long]$Size = 117638417,
    [int]$Parts = 8,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'hash-util.ps1')
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$tools = Join-Path $root '.tools'
$dest = Join-Path $root 'src-tauri/resources/ffmpeg'
$exe = Join-Path $dest 'ffmpeg.exe'
New-Item -ItemType Directory -Force -Path $tools, $dest | Out-Null

if ((Test-Path $exe) -and -not $Force) {
    Write-Host "已存在：$exe（加 -Force 可重新下载）"
    exit 0
}

$asset = "https://github.com/GyanD/codexffmpeg/releases/download/2026-11-02-git-b034eb5e7e/ffmpeg-2026-11-02-git-b034eb5e7e-essentials_build.zip"
$onCi = ($env:CI -eq 'true') -or ($env:GITHUB_ACTIONS -eq 'true')
if ([string]::IsNullOrWhiteSpace($Url)) {
    if ($onCi) {
        # GitHub runner 直连 GitHub 最快；镜像绕路反而可能超时
        $candidates = @($asset, "https://gh-proxy.com/$asset")
    } else {
        $candidates = @("https://gh-proxy.com/$asset", "https://ghproxy.net/$asset", $asset)
    }
} else {
    $candidates = @($Url)
}

$zip = Join-Path $tools 'ffmpeg-essentials.zip'
$partsDir = Join-Path $tools 'ffmpeg-parts'
$downloaded = $false
foreach ($u in $candidates) {
    Write-Host "下载 $u（$Parts 段并发）"
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Remove-Item $partsDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $partsDir | Out-Null

    $chunk = [int64][math]::Ceiling($Size / $Parts)
    $procs = @()
    for ($i = 0; $i -lt $Parts; $i++) {
        $start = [int64]($i * $chunk)
        $end = [int64][math]::Min($Size - 1, ($i + 1) * $chunk - 1)
        if ($start -gt $end) { continue }
        $out = Join-Path $partsDir ("p{0:D2}" -f $i)
        # 每段独立文件：curl --retry 只会重下本段，不会像单文件那样把整体截断
        $argList = @('-s', '-L', '--fail', '--retry', '5', '--retry-all-errors', '--retry-delay', '2',
            '--max-time', '1800', '--speed-limit', '20480', '--speed-time', '60',
            '-r', "$start-$end", '-A', 'Mozilla/5.0', '-o', "`"$out`"", "`"$u`"")
        $procs += Start-Process -FilePath 'curl.exe' -ArgumentList $argList -NoNewWindow -PassThru
    }
    $procs | Wait-Process -Timeout 1800 -ErrorAction SilentlyContinue

    $total = 0L
    for ($i = 0; $i -lt $Parts; $i++) {
        $p = Join-Path $partsDir ("p{0:D2}" -f $i)
        if (Test-Path $p) { $total += (Get-Item $p).Length }
    }
    if ($total -ne $Size) {
        Write-Warning "下载不完整（$total / $Size 字节），换下一个镜像"
        continue
    }

    $fs = [System.IO.File]::Open($zip, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
    try {
        for ($i = 0; $i -lt $Parts; $i++) {
            $part = Join-Path $partsDir ("p{0:D2}" -f $i)
            if (-not (Test-Path $part)) { continue }
            $bytes = [System.IO.File]::ReadAllBytes($part)
            $fs.Write($bytes, 0, $bytes.Length)
        }
    } finally { $fs.Dispose() }
    Remove-Item $partsDir -Recurse -Force -ErrorAction SilentlyContinue

    $actual = (Get-Sha256Hex $zip)
    if ($actual -eq $Sha256.ToLowerInvariant()) {
        Write-Host "SHA-256 校验通过"
        $downloaded = $true
        break
    }
    Write-Warning "SHA-256 不匹配（实际 $actual），换下一个镜像"
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
}
if (-not $downloaded) {
    throw "所有镜像都下载失败或校验不通过；可换 -Url 指定镜像"
}

$extract = Join-Path $tools 'ffmpeg-extract'
Remove-Item $extract -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $extract | Out-Null
Expand-Archive -Path $zip -DestinationPath $extract -Force

$found = Get-ChildItem -Path $extract -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
if (-not $found) { throw "压缩包里没有 ffmpeg.exe" }
Copy-Item $found.FullName $exe -Force
Remove-Item $extract -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $zip -Force -ErrorAction SilentlyContinue

$sizeMb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "完成：src-tauri/resources/ffmpeg/ffmpeg.exe ($sizeMb MB) —— 打包时会随安装包分发"
