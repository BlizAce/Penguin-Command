# penguin command installer for Windows
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   $env:PC_INSTALL_DIR = "C:\tools"; .\install.ps1
$ErrorActionPreference = "Stop"

function Say($m)  { Write-Host "🐧 $m" -ForegroundColor Magenta }
function Warn($m) { Write-Host "⚠ $m" -ForegroundColor Yellow }

Set-Location $PSScriptRoot

# locate cargo, bootstrap rustup if missing
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    $rustupEnv = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $rustupEnv) {
        $env:Path = (Join-Path $env:USERPROFILE ".cargo\bin") + ";$env:Path"
    } else {
        $reply = Read-Host "Rust not found. Install via rustup? [Y/n]"
        if ($reply -match '^[nN]') { throw "cargo is required to build penguin command." }
        Say "installing rust toolchain…"
        Invoke-WebRequest -UseBasicParsing https://rustup.rs -OutFile "$env:TEMP\rustup-init.exe" | Out-Null
        & "$env:TEMP\rustup-init.exe" -y --profile minimal
        $env:Path = (Join-Path $env:USERPROFILE ".cargo\bin") + ";$env:Path"
    }
}

Say "building penguin command (release)…"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$bin = Join-Path $PSScriptRoot "target\release\pc.exe"
if (-not (Test-Path $bin)) { throw "build produced no binary" }

# pick install dir
if ($env:PC_INSTALL_DIR) {
    $dest = $env:PC_INSTALL_DIR
} else {
    $dest = Join-Path $env:LOCALAPPDATA "Programs\pc"
}
New-Item -ItemType Directory -Force -Path $dest | Out-Null

Say "installing to $dest\pc.exe"
Copy-Item $bin (Join-Path $dest "pc.exe") -Force

# add to user PATH if missing
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $dest) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    $env:Path = "$env:Path;$dest"
    Warn "added $dest to your user PATH (restart your terminal)"
}

Say "installed: $(& (Join-Path $dest 'pc.exe') --version)"
Say "run 'pc' to launch, or 'pc setup' to configure a provider."
