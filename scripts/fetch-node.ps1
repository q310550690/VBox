# 打包/开发用：下载 Node.js（只取 node.exe）到 src-tauri/resources/node/
#
# 用途：VBox 的 drpy/drpy2 规则由 Node.js 执行；内置一份 node.exe 后
# 用户不需要自己装 Node。版本固定 v22.11.0（npmmirror 镜像优先，nodejs.org 兜底）。
#
# 用法（在仓库根目录）：
#   pwsh -File scripts/fetch-node.ps1
#   pwsh -File scripts/fetch-node.ps1 -Force
param(
    [string]$Url = "",
    [string]$Sha256 = "905373a059aecaf7f48c1ce10ffbd5334457ca00f678747f19db5ea7d256c236",
    [long]$Size = 34188182,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'hash-util.ps1')
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$cache = Join-Path $root '.tools/cache'
$dest = Join-Path $root 'src-tauri/resources/node'
$exe = Join-Path $dest 'node.exe'
$ready = Join-Path $dest '.vbox-ready'
New-Item -ItemType Directory -Force -Path $cache, $dest | Out-Null

if ((Test-Path $ready) -and -not $Force) {
    Write-Host "已存在：$dest（加 -Force 可重装）"
    exit 0
}

$ver = 'v22.11.0'
$asset = "https://nodejs.org/dist/$ver/node-$ver-win-x64.zip"
$mirror = "https://registry.npmmirror.com/-/binary/node/$ver/node-$ver-win-x64.zip"
$onCi = ($env:CI -eq 'true') -or ($env:GITHUB_ACTIONS -eq 'true')
if ([string]::IsNullOrWhiteSpace($Url)) {
    if ($onCi) {
        $candidates = @($asset, $mirror)
    } else {
        $candidates = @($mirror, $asset)
    }
} else {
    $candidates = @($Url)
}

$zip = Join-Path $cache "node-$ver-win-x64.zip"
$got = $false
foreach ($u in $candidates) {
    if ((Test-Path $zip) -and ((Get-Sha256Hex $zip) -eq $Sha256)) {
        $got = $true
        break
    }
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Write-Host "下载 $u"
    & curl.exe -s -L --fail --max-time 900 -A 'Mozilla/5.0' -o "$zip" "$u"
    if ($LASTEXITCODE -ne 0) { continue }
    if ((Get-Sha256Hex $zip) -eq $Sha256) {
        $got = $true
        break
    }
    Write-Warning 'SHA-256 不匹配，换下一个镜像'
}
if (-not $got) { throw 'Node.js 包下载失败或校验不通过' }
Write-Host 'Node.js 包 SHA-256 校验通过'

# 只取出 node.exe（+ LICENSE），避免把 npm/文档全塞进安装包；
# 保留目录里的 README.txt 占位（要进 git）
$readme = Join-Path $dest 'README.txt'
$readmeBytes = if (Test-Path $readme) { [System.IO.File]::ReadAllBytes($readme) } else { $null }
Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $dest | Out-Null
if ($readmeBytes) { [System.IO.File]::WriteAllBytes($readme, $readmeBytes) }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
try {
    $nodeEntry = $archive.Entries | Where-Object { $_.FullName -match '/node\.exe$' } | Select-Object -First 1
    if (-not $nodeEntry) { throw '压缩包里没有 node.exe' }
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($nodeEntry, $exe, $true)
    $licenseEntry = $archive.Entries | Where-Object { $_.FullName -match '/LICENSE$' } | Select-Object -First 1
    if ($licenseEntry) {
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile(
            $licenseEntry, (Join-Path $dest 'LICENSE.txt'), $true)
    }
} finally { $archive.Dispose() }

$version = (& "$exe" --version) 2>&1
if ($LASTEXITCODE -ne 0) { throw "node.exe 自检失败：$version" }
Write-Host "Node 版本：$version"

$sizeMb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
# 完成标记：ensure-tools 用它判断 node.exe 真的可用，避免把半成品打进安装包
New-Item -ItemType File -Force -Path $ready | Out-Null
Write-Host "完成：src-tauri/resources/node/node.exe（$sizeMb MB）—— 打包时会随安装包分发"
