param([string]$Version = '1.26.11')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$dependencyRoot = Join-Path $projectRoot '.deps'
$downloads = Join-Path $dependencyRoot 'downloads'
$extractRoot = Join-Path $dependencyRoot 'gstreamer'
New-Item -ItemType Directory -Force -Path $downloads,$extractRoot | Out-Null
$baseUrl = "https://gstreamer.freedesktop.org/data/pkg/windows/$Version/msvc"
foreach ($package in @("gstreamer-1.0-msvc-x86_64-$Version.msi", "gstreamer-1.0-devel-msvc-x86_64-$Version.msi")) {
    $archive = Join-Path $downloads $package
    $expected = ((Invoke-RestMethod "$baseUrl/$package.sha256sum").Trim() -split '\s+')[0]
    if (!(Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) {
        & "$PSScriptRoot/download.ps1" -Url "$baseUrl/$package" -OutputPath $archive
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) { throw "Checksum mismatch: $package" }
    $marker = Join-Path $extractRoot "$package.extracted"
    if (!(Test-Path -LiteralPath $marker)) {
        $logPath = Join-Path $downloads "$package.log"
        $arguments = @('/a', "`"$archive`"", '/qn', "TARGETDIR=`"$extractRoot`"", '/l*v', "`"$logPath`"")
        $extractor = Start-Process -FilePath 'msiexec.exe' -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
        if ($extractor.ExitCode -notin @(0,3010)) { throw "Extraction failed ($($extractor.ExitCode)); see $logPath" }
        Set-Content -LiteralPath $marker -Value $expected
    }
}
$gstDll = Get-ChildItem -LiteralPath $extractRoot -Recurse -Filter 'gstreamer-1.0-0.dll' | Select-Object -First 1
if (!$gstDll) { throw 'GStreamer runtime was not extracted.' }
$gstRoot = Split-Path -Parent (Split-Path -Parent $gstDll.FullName)
Set-Content -LiteralPath (Join-Path $dependencyRoot 'gstreamer-root.txt') -Value $gstRoot
Write-Output "GStreamer $Version ready: $gstRoot"
