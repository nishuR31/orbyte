# orbx for Windows PowerShell - runs the orb built from this checkout.
# Usage: powershell -ExecutionPolicy Bypass -File scripts/orbx.ps1 --version
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$OrbArgs
)

$ErrorActionPreference = 'Stop'
$root = if ($env:ORB_ROOT) { $env:ORB_ROOT } else { Split-Path -Parent $PSScriptRoot }

if ($env:ORB_BIN) {
    & $env:ORB_BIN @OrbArgs
    exit $LASTEXITCODE
}

function Find-OrbBinary($profile) {
    foreach ($name in @('orb.exe', 'orb')) {
        $candidate = Join-Path $root "target/$profile/$name"
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

$bin = Find-OrbBinary 'release'
if (-not $bin) { $bin = Find-OrbBinary 'debug' }
if (-not $bin) {
    Write-Host "orbx: no built binary under $root/target; building release..."
    cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $bin = Find-OrbBinary 'release'
}

if ($env:ORBX_REBUILD -eq '1') {
    cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $bin = Find-OrbBinary 'release'
}

& $bin @OrbArgs
exit $LASTEXITCODE
