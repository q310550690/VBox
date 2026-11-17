# 打包/开发用：下载「嵌入式 Python」到 src-tauri/resources/python/
#
# 用途：VBox 的 .py spider 需要 CPython + requests/bs4/lxml/pycryptodome/pyquery。
# 这个脚本把 Python 3.12.8 embeddable 包解压到 resources/python/，开启 site、
# 装好 pip 与常用依赖，随安装包一起分发；用户不用自己装 Python。
#
# 版本、大小、SHA-256 固定（npmmirror 镜像优先，python.org 兜底），
# get-pip.py 也固定哈希；依赖从清华 PyPI 镜像安装（可用 -PipIndex 换源）。
#
# 用法（在仓库根目录）：
#   pwsh -File scripts/fetch-python.ps1
#   pwsh -File scripts/fetch-python.ps1 -Force
#   pwsh -File scripts/fetch-python.ps1 -PipIndex "https://pypi.org/simple"
param(
    [string]$Url = "",
    [string]$Sha256 = "8d3f33be9eb810f23c102f08475af2854e50484b8e4e06275e937be61ce3d2fb",
    [long]$Size = 11094114,
    [string]$PipIndex = "https://pypi.tuna.tsinghua.edu.cn/simple",
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'hash-util.ps1')
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$cache = Join-Path $root '.tools/cache'
$dest = Join-Path $root 'src-tauri/resources/python'
$exe = Join-Path $dest 'python.exe'
$ready = Join-Path $dest '.vbox-ready'
New-Item -ItemType Directory -Force -Path $cache, $dest | Out-Null

if ((Test-Path $ready) -and -not $Force) {
    Write-Host "已存在：$dest（加 -Force 可重装）"
    exit 0
}

$ver = '3.12.8'
$asset = "https://www.python.org/ftp/python/$ver/python-$ver-embed-amd64.zip"
$mirror = "https://registry.npmmirror.com/-/binary/python/$ver/python-$ver-embed-amd64.zip"
$onCi = ($env:CI -eq 'true') -or ($env:GITHUB_ACTIONS -eq 'true')
if ($onCi -and $PipIndex -eq 'https://pypi.tuna.tsinghua.edu.cn/simple') {
    # runner 在海外，官方 PyPI 更稳
    $PipIndex = 'https://pypi.org/simple'
}
if ([string]::IsNullOrWhiteSpace($Url)) {
    if ($onCi) {
        $candidates = @($asset, $mirror)
    } else {
        $candidates = @($mirror, $asset)
    }
} else {
    $candidates = @($Url)
}

$zip = Join-Path $cache "python-$ver-embed-amd64.zip"
$got = $false
foreach ($u in $candidates) {
    if ((Test-Path $zip) -and ((Get-Sha256Hex $zip) -eq $Sha256)) {
        $got = $true
        break
    }
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Write-Host "下载 $u"
    & curl.exe -s -L --fail --max-time 600 -A 'Mozilla/5.0' -o "$zip" "$u"
    if ($LASTEXITCODE -ne 0) { continue }
    if ((Get-Sha256Hex $zip) -eq $Sha256) {
        $got = $true
        break
    }
    Write-Warning 'SHA-256 不匹配，换下一个镜像'
}
if (-not $got) { throw 'Python embeddable 包下载失败或校验不通过' }
Write-Host 'Python 包 SHA-256 校验通过'

# 重新解压（-Force 时也保证干净）；保留目录里的 README.txt 占位（要进 git）
$readme = Join-Path $dest 'README.txt'
$readmeBytes = if (Test-Path $readme) { [System.IO.File]::ReadAllBytes($readme) } else { $null }
Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $dest | Out-Null
if ($readmeBytes) { [System.IO.File]::WriteAllBytes($readme, $readmeBytes) }
Expand-Archive -Path $zip -DestinationPath $dest -Force

# 打开 site（embeddable 默认禁用），并加入 Lib\site-packages
$pth = Get-ChildItem $dest -Filter '*._pth' | Select-Object -First 1
if (-not $pth) { throw 'Python embeddable 包里没有 ._pth' }
$zipName = "$($pth.BaseName).zip"
$pthLines = @($zipName, '.', 'Lib\site-packages', 'import site')
[System.IO.File]::WriteAllLines($pth.FullName, $pthLines, [System.Text.UTF8Encoding]::new($false))

# get-pip（固定哈希；get-pip 更新后需同步这里）
$getPipSha = '356e0d0de21cdc348c3394084d90939b85e4246c4d14c76aee6ca4f46646ea98'
$getPip = Join-Path $cache 'get-pip.py'
$needPip = -not (Test-Path $getPip)
if (-not $needPip -and ((Get-Sha256Hex $getPip) -ne $getPipSha)) {
    $needPip = $true
}
if ($needPip) {
    Write-Host '下载 get-pip.py'
    & curl.exe -s -L --fail --max-time 120 -o "$getPip" 'https://bootstrap.pypa.io/get-pip.py'
    if ($LASTEXITCODE -ne 0) { throw 'get-pip.py 下载失败' }
    if ((Get-Sha256Hex $getPip) -ne $getPipSha) {
        throw 'get-pip.py SHA-256 不匹配（上游已更新？需同步脚本里的 getPipSha）'
    }
}

Write-Host '安装 pip…'
& "$dest\python.exe" "$getPip" --no-warn-script-location --no-cache-dir
if ($LASTEXITCODE -ne 0) { throw 'get-pip 安装失败' }

Write-Host "安装依赖（$PipIndex）…"
& "$dest\python.exe" -m pip install --no-warn-script-location --no-cache-dir `
    --disable-pip-version-check -i $PipIndex `
    requests beautifulsoup4 lxml pycryptodome pyquery
if ($LASTEXITCODE -ne 0) { throw 'Python 依赖安装失败（检查网络 / -PipIndex）' }

# 清理缓存文件
Get-ChildItem $dest -Recurse -Directory -Filter '__pycache__' -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

& "$dest\python.exe" -c "import requests, bs4, lxml, Crypto, pyquery; print('deps ok')"
if ($LASTEXITCODE -ne 0) { throw '依赖自检失败' }

$sizeMb = [math]::Round(((Get-ChildItem $dest -Recurse | Measure-Object Length -Sum).Sum / 1MB), 1)
# 完成标记：ensure-tools 用它判断「解释器 + 依赖真的装好了」，避免把半成品打进安装包
New-Item -ItemType File -Force -Path (Join-Path $dest '.vbox-ready') | Out-Null
Write-Host "完成：src-tauri/resources/python（$sizeMb MB，含 pip 与常用依赖）—— 打包时会随安装包分发"
