# 打包/开发用：下载 Temurin JRE 17（Windows x64）到 src-tauri/resources/jvm/jre/
#
# 用途：VBox 的 jar 蜘蛛跑在真 JVM 上（兼容层 + 原始 jar 字节码），需要一个 JRE 17。
# 版本固定 17.0.21+9（Adoptium Temurin，校验 SHA-256；镜像取清华 TUNA）；解压后压平成
#   src-tauri/resources/jvm/jre/bin/java.exe
# 与 scripts/build-jvm.mjs 产出的 stubs.jar / spider-host.jar / libs/ 同级。
#
# 用法（在仓库根目录）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-jre.ps1
#   powershell ... -Force        # 重装
#   powershell ... -Verify       # 只校验已就位的运行时
param(
    [string]$Url = "",
    [string]$Sha256 = "fe4656bd78fb09812dc38c49da777f79cc33e7ee435a254be82f9e7f40d9d117",
    [long]$Size = 43860024,
    [string]$Version = "17.0.21+9",
    [string]$Asset = "OpenJDK17U-jre_x64_windows_hotspot_17.0.21_9.zip",
    [switch]$Force,
    [switch]$Verify
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'hash-util.ps1')
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$cache = Join-Path $root '.tools/cache'
$dest = Join-Path $root 'src-tauri/resources/jvm'
$jre = Join-Path $dest 'jre'
$java = Join-Path $jre 'bin/java.exe'
$marker = Join-Path $dest 'marker.json'
New-Item -ItemType Directory -Force -Path $cache, $dest | Out-Null

# 调用 java 并安全捕获输出：$ErrorActionPreference='Stop' 时，
# 原生命令往 stderr 写东西（java -version 就是写 stderr）会被 PowerShell 当成
# NativeCommandError 终止脚本，所以这里临时降级为 Continue。
function Invoke-Java {
    param([string[]]$JavaArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $text = & "$java" @JavaArgs 2>&1 | Out-String
        return @{ code = $LASTEXITCODE; text = $text }
    } finally {
        $ErrorActionPreference = $prev
    }
}

function Get-JavaVersion([string]$Text) {
    $m = [regex]::Match($Text, 'java\.version = (\S+)')
    if ($m.Success) { return $m.Groups[1].Value }
    $m = [regex]::Match($Text, 'version "(\S+)"')
    if ($m.Success) { return $m.Groups[1].Value }
    return ''
}

function Write-Marker {
    $payload = [ordered]@{
        mode         = 'jre'
        asset        = $Asset
        sha256       = $Sha256
        bytes        = $Size
        version      = $Version
        javaVersion  = (Get-JavaVersion (Invoke-Java @('-XshowSettings:properties', '-version')).text)
        arch         = 'x64'
        source       = 'adoptium-temurin'
        stagedAt     = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    }
    $json = $payload | ConvertTo-Json -Depth 4
    [System.IO.File]::WriteAllText($marker, $json, [System.Text.UTF8Encoding]::new($false))
}

function Test-Installed {
    if (-not (Test-Path $java)) { return $false }
    if (-not (Test-Path $marker)) { return $false }
    try {
        $m = Get-Content $marker -Raw | ConvertFrom-Json
        return ($m.sha256 -eq $Sha256)
    } catch {
        return $false
    }
}

if ($Verify) {
    if (-not (Test-Installed)) { throw "JRE 未就位：$jre（先跑本脚本不带 -Verify）" }
    $v = Get-JavaVersion (Invoke-Java @('-version')).text
    Write-Host "JRE 已就位：$jre"
    Write-Host "  $v"
    exit 0
}

if ((Test-Installed) -and -not $Force) {
    Write-Host "已存在：$jre（加 -Force 可重装）"
    exit 0
}

$onCi = ($env:CI -eq 'true') -or ($env:GITHUB_ACTIONS -eq 'true')
$tuna = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/17/jre/x64/windows/$Asset"
$direct = "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-$([uri]::EscapeDataString($Version))/$Asset"
$api = "https://api.adoptium.net/v3/binary/version/jdk-$([uri]::EscapeDataString($Version))/windows/x64/jre/hotspot/normal/eclipse"
if (-not [string]::IsNullOrWhiteSpace($Url)) {
    $candidates = @($Url)
} else {
    # 国内先走清华 TUNA 镜像（实测 4~10 MB/s）；CI 直连 release 资源，API 兜底
    if ($onCi) { $candidates = @($direct, $api, $tuna) } else { $candidates = @($tuna, $api, $direct) }
}

$zip = Join-Path $cache "jre-$($Version.Replace('+', '_'))-win-x64.zip"
$got = $false
foreach ($u in $candidates) {
    if ((Test-Path $zip) -and ((Get-Sha256Hex $zip) -eq $Sha256)) {
        $got = $true
        break
    }
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Write-Host "下载 $u"
    & curl.exe -s -L --fail --retry 3 --retry-delay 2 --max-time 1800 -A 'Mozilla/5.0' -o "$zip" "$u"
    if ($LASTEXITCODE -ne 0) { continue }
    $actual = Get-Sha256Hex $zip
    if ($actual -eq $Sha256) {
        $got = $true
        break
    }
    Write-Warning "SHA-256 不匹配（实际 $actual），换下一个镜像"
}
if (-not $got) { throw 'JRE 包下载失败或校验不通过' }
$sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "JRE 包 SHA-256 校验通过（$sizeMb MB）"

# 解压：压平顶层目录（jdk-17.0.21.1+2-jre/bin/java.exe → jre/bin/java.exe）
$tmp = Join-Path $dest 'jre.tmp'
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
try {
    $count = 0
    foreach ($entry in $archive.Entries) {
        if ([string]::IsNullOrEmpty($entry.Name)) { continue }   # 目录条目
        $rel = $entry.FullName -replace '^[^/]+/', ''            # 去掉顶层目录
        if ([string]::IsNullOrEmpty($rel)) { continue }
        $target = Join-Path $tmp ($rel -replace '/', [System.IO.Path]::DirectorySeparatorChar)
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
        $count += 1
    }
    Write-Host "解压 $count 个文件"
} finally { $archive.Dispose() }

if (-not (Test-Path (Join-Path $tmp 'bin/java.exe'))) { throw 'JRE 包结构异常：没有 bin/java.exe' }

Remove-Item $jre -Recurse -Force -ErrorAction SilentlyContinue
Move-Item $tmp $jre

$check = Invoke-Java @('-version')
if ($check.code -ne 0) { throw "java 自检失败：$($check.text)" }
$version = Get-JavaVersion $check.text
Write-Host "java 版本：$version"
Write-Marker
$jreSize = [math]::Round((Get-ChildItem $jre -Recurse -File | Measure-Object Length -Sum).Sum / 1MB, 1)
Write-Host "完成：src-tauri/resources/jvm/jre（$jreSize MB）—— 打包时会随安装包分发"
