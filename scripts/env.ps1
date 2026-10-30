$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$rootFile = Join-Path $projectRoot '.deps/gstreamer-root.txt'
if (!(Test-Path -LiteralPath $rootFile)) { throw 'Run scripts/setup-deps.ps1 first.' }
$gstRoot = (Get-Content -LiteralPath $rootFile -Raw).Trim()
$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $gstRoot
$env:GSTREAMER_1_0_ROOT_X86_64 = $gstRoot
$env:PKG_CONFIG_PATH = Join-Path $gstRoot 'lib/pkgconfig'
$env:PATH = "$(Join-Path $gstRoot 'bin');$env:PATH"
$pkgConfig = Join-Path $gstRoot 'bin/pkg-config.exe'
if (Test-Path -LiteralPath $pkgConfig) { $env:PKG_CONFIG = $pkgConfig }
