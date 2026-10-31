param([ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version = '1.28.8')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$dependencyRoot = Join-Path $projectRoot '.deps'
$downloads = Join-Path $dependencyRoot 'downloads'
# Isolate SDK versions so upgrades cannot retain obsolete DLLs or plugins.
$gstRoot = Join-Path $dependencyRoot "gstreamer/$Version/msvc_x86_64"
New-Item -ItemType Directory -Force -Path $downloads,$gstRoot | Out-Null
$baseUrl = "https://gstreamer.freedesktop.org/data/pkg/windows/$Version/msvc"
$package = "gstreamer-1.0-msvc-x86_64-$Version.exe"
$archive = Join-Path $downloads $package
$expected = ((Invoke-RestMethod "$baseUrl/$package.sha256sum").Trim() -split '\s+')[0]
if ($expected -notmatch '^[a-fA-F0-9]{64}$') { throw "Invalid checksum: $package" }
if (!(Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) {
    & "$PSScriptRoot/download.ps1" -Url "$baseUrl/$package" -OutputPath $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) { throw "Checksum mismatch: $package" }
$marker = Join-Path $gstRoot '.extracted-sha256'
if (!(Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw).Trim() -ne $expected) {
    $logPath = Join-Path $downloads "$package.log"
    # GStreamer 1.28 uses Inno Setup. Official portable mode disables global
    # environment/registry changes, VS templates, VC runtime setup and uninstall registration.
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER',
        '/PORTABLE=1', '/TYPE=devel', '/TASKS=""', "/DIR=`"$gstRoot`"", "/LOG=`"$logPath`"")
    $extractor = Start-Process -FilePath $archive -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    try {
        if ($extractor.ExitCode -ne 0) { throw "Extraction failed ($($extractor.ExitCode)); see $logPath" }
    } finally { $extractor.Dispose() }
}
foreach ($file in @('bin/gstreamer-1.0-0.dll', 'bin/pkg-config.exe', 'lib/pkgconfig/gstreamer-1.0.pc',
    'lib/gstreamer-1.0/gstplayback.dll', 'libexec/gstreamer-1.0/gst-plugin-scanner.exe')) {
    if (!(Test-Path -LiteralPath (Join-Path $gstRoot $file) -PathType Leaf)) { throw "Incomplete GStreamer SDK: $file" }
}
$sdkVersion = (Select-String -LiteralPath (Join-Path $gstRoot 'lib/pkgconfig/gstreamer-1.0.pc') -Pattern '^Version:\s*(.+)$').Matches.Groups[1].Value
# Official Windows packages may include a fourth numeric build component.
if ($sdkVersion -notmatch ('^' + [regex]::Escape($Version) + '(\.\d+)?$')) {
    throw "Expected GStreamer $Version, found $sdkVersion"
}
Set-Content -LiteralPath $marker -Value $expected
Set-Content -LiteralPath (Join-Path $dependencyRoot 'gstreamer-root.txt') -Value $gstRoot
Write-Output "GStreamer $Version ready: $gstRoot"
