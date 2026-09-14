# penguin command uninstaller for Windows
#   powershell -ExecutionPolicy Bypass -File uninstall.ps1            # keep config
#   powershell -ExecutionPolicy Bypass -File uninstall.ps1 -Purge     # also delete config
param([switch]$Purge)

function Say($m)  { Write-Host "🐧 $m" -ForegroundColor Magenta }
function Warn($m) { Write-Host "⚠ $m" -ForegroundColor Yellow }

function Is-Ours($path) {
    if (-not (Test-Path $path)) { return $false }
    try { (& $path --version) -match '^pc ' } catch { $false }
}

$removed = $false
$candidates = @()
if ($env:PC_INSTALL_DIR) { $candidates += Join-Path $env:PC_INSTALL_DIR "pc.exe" }
$candidates += (Join-Path $env:LOCALAPPDATA "Programs\pc\pc.exe")

foreach ($exe in $candidates) {
    if (Is-Ours $exe) {
        Remove-Item $exe -Force
        Say "removed $exe"
        $removed = $true
        # drop the containing dir from user PATH if now empty and we added it
        $dir = Split-Path $exe
        if (-not (Get-ChildItem $dir -ErrorAction SilentlyContinue)) {
            Remove-Item $dir -Force -ErrorAction SilentlyContinue
            $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
            $kept = ($userPath -split ";") | Where-Object { $_ -and $_ -ne $dir }
            [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
            Say "removed $dir from user PATH"
        }
    }
}

if (-not $removed) { Warn "no penguin command binary found in known locations" }

$cfg = Join-Path $env:APPDATA "penguin"
if (Test-Path $cfg) {
    if ($Purge) {
        Remove-Item $cfg -Recurse -Force
        Say "removed config $cfg"
    } else {
        Say "kept config in $cfg (providers, permission rules) — rerun with -Purge to delete"
    }
}

Say "done."
