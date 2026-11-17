# 计算文件 SHA-256（小写 hex）。
#
# 不依赖 Get-FileHash：GitHub Actions 的 Windows runner 上曾出现该 cmdlet 不可用
# （CommandNotFoundException），因此优先用系统 certutil，再回退 Get-FileHash / .NET。
function Get-Sha256Hex([string]$Path) {
    $certutil = Join-Path $env:SystemRoot 'System32\certutil.exe'
    if (Test-Path $certutil) {
        $out = & $certutil -hashfile $Path SHA256 2>$null
        $hex = ($out | Where-Object { $_ -match '^[0-9a-fA-F ]+$' } | Select-Object -First 1)
        if ($hex) {
            return ($hex -replace '\s', '').ToLowerInvariant()
        }
    }
    if (Get-Command Get-FileHash -ErrorAction SilentlyContinue) {
        return (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return (($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) -join '')
    } finally {
        $stream.Dispose()
        $sha.Dispose()
    }
}
