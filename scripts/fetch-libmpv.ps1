# 下载 libmpv-2.dll（Windows x64）到 src-tauri/resources/
#
# 说明：官方 mpv 不提供稳定的 libmpv 分发，这里用 shinchiro 的 mpv-dev 构建。
# GitHub release 在国内直连很慢/会断，脚本默认走 ghproxy.net 并做 10 路 Range 并发，
# 失败会自动重试；也可用 -ProxyPrefix "" 走直连，或换其它 GitHub 代理。
#
# 用法（在仓库根目录）：
#   pwsh -File scripts/fetch-libmpv.ps1
#   pwsh -File scripts/fetch-libmpv.ps1 -ProxyPrefix "https://ghproxy.net/"
param(
    [string]$ProxyPrefix = "https://ghproxy.net/",
    [string]$Release = "20261021",
    [string]$Asset = "mpv-dev-x86_64-20261021-git-e471496c56.7z",
    [int]$Parts = 10
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$tools = Join-Path $root '.tools'
$resources = Join-Path $root 'src-tauri/resources'
New-Item -ItemType Directory -Force -Path $tools, $resources | Out-Null

$assetUrl = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$Release/$Asset"
$url = "$ProxyPrefix$assetUrl"
$archive = Join-Path $tools $Asset

function Get-RemoteSize([string]$u) {
    $head = & curl.exe -s -I -L --max-time 40 -A "Mozilla/5.0" $u 2>$null
    $line = $head | Where-Object { $_ -match '^content-length:' } | Select-Object -Last 1
    if (-not $line) { return 0 }
    return [int64](($line -split ':')[1].Trim())
}

$size = Get-RemoteSize $url
if ($size -le 0) { throw "无法获取文件大小：$url" }
Write-Host "文件大小: $size 字节"

$partsDir = Join-Path $tools 'mpv-parts'
Remove-Item $partsDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $partsDir | Out-Null

$chunk = [math]::Ceiling($size / $Parts)
$procs = @()
for ($i = 0; $i -lt $Parts; $i++) {
    $start = [int64]($i * $chunk)
    $end = [int64][math]::Min($size - 1, ($i + 1) * $chunk - 1)
    $out = Join-Path $partsDir ("p{0:D2}" -f $i)
    $args = @('-L', '--retry', '20', '--retry-all-errors', '--retry-delay', '3', '--max-time', '3600',
        '-s', '-r', "$start-$end", '-A', 'Mozilla/5.0', '-o', "`"$out`"", "`"$url`"")
    $procs += Start-Process -FilePath 'curl.exe' -ArgumentList $args -NoNewWindow -PassThru
}
$procs | Wait-Process -Timeout 3600 -ErrorAction SilentlyContinue

$total = 0L
for ($i = 0; $i -lt $Parts; $i++) {
    $p = Join-Path $partsDir ("p{0:D2}" -f $i)
    if (Test-Path $p) { $total += (Get-Item $p).Length }
}
if ($total -ne $size) { throw "下载不完整：$total / $size（可重试或换 -ProxyPrefix）" }

$fs = [System.IO.File]::Open($archive, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
try {
    for ($i = 0; $i -lt $Parts; $i++) {
        $bytes = [System.IO.File]::ReadAllBytes((Join-Path $partsDir ("p{0:D2}" -f $i)))
        $fs.Write($bytes, 0, $bytes.Length)
    }
} finally { $fs.Dispose() }
Remove-Item $partsDir -Recurse -Force -ErrorAction SilentlyContinue

$extract = Join-Path $tools 'mpv-dev'
Remove-Item $extract -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $extract | Out-Null
& tar -xf $archive -C $extract
$dll = Join-Path $extract 'libmpv-2.dll'
if (-not (Test-Path $dll)) { throw "压缩包里没有 libmpv-2.dll" }
Copy-Item $dll (Join-Path $resources 'libmpv-2.dll') -Force
Write-Host "完成：src-tauri/resources/libmpv-2.dll ($((Get-Item (Join-Path $resources 'libmpv-2.dll')).Length) 字节)"
