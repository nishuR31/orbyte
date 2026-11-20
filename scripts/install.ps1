# Installs the latest (or a pinned) orb release from GitHub Releases.
#
# Environment:
#   ORB_REPO         GitHub slug, e.g. "owner/repo" (default: nishuR31/orbyte)
#   ORB_VERSION      release tag, or "latest" (default)
#   ORB_INSTALL_DIR  destination directory (default: %LOCALAPPDATA%\Orb\bin)
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts/install.ps1

$ErrorActionPreference = 'Stop'

$repo = if ($env:ORB_REPO) { $env:ORB_REPO } else { 'nishuR31/orbyte' }
$version = if ($env:ORB_VERSION) { $env:ORB_VERSION } else { 'latest' }
$installDir = if ($env:ORB_INSTALL_DIR) { $env:ORB_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Orb\bin' }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$asset = "orb-windows-$arch.zip"
$url = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download/$asset"
} else {
    "https://github.com/$repo/releases/download/$version/$asset"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("orb-install-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "downloading $url"
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $asset)
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
    if (-not (Test-Path (Join-Path $tmp 'orb.exe'))) {
        throw "Archive '$asset' does not contain an 'orb.exe' binary"
    }

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    $target = Join-Path $installDir 'orb.exe'
    $verb = 'installed'
    $previousNote = ''
    # Re-running the installer updates in place. A running exe cannot be
    # overwritten on Windows, so move it aside and explain a locked file.
    if (Test-Path $target) {
        try {
            Move-Item $target (Join-Path $installDir 'orb.old.exe') -Force
            $verb = 'updated'
            $previousNote = " (previous binary: $(Join-Path $installDir 'orb.old.exe'))"
        } catch {
            throw "Cannot replace $target while it is running. Close orb and retry. $($_.Exception.Message)"
        }
    }
    Copy-Item (Join-Path $tmp 'orb.exe') $target -Force

    # `orb build --native` uses rustc as the linker and needs this library.
    # find_runtime_lib() looks next to the executable first.
    if (Test-Path (Join-Path $tmp 'runtime.lib')) {
        Copy-Item (Join-Path $tmp 'runtime.lib') (Join-Path $installDir 'runtime.lib') -Force
    } else {
        Write-Warning 'Archive has no runtime.lib; set ORB_RUNTIME_LIB to use AOT output'
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host "orb $verb to $(Join-Path $installDir 'orb.exe')$previousNote"
if ($env:PATH -notlike "*$installDir*") {
    Write-Host "add to PATH: `$env:PATH = \"$installDir;`$env:PATH\""
}
& (Join-Path $installDir 'orb.exe') --version
