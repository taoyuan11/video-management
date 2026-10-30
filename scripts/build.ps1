param([switch]$SkipBuild)
. "$PSScriptRoot/env.ps1"
. "$PSScriptRoot/package-runtime.ps1"
# Windows PowerShell 5.1 does not load ZIP support automatically.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$utf8NoBom = [Text.UTF8Encoding]::new($false)
Push-Location $projectRoot
try {
    # dist is disposable build output. Validate its boundary before removing anything.
    $dist = [IO.Path]::GetFullPath((Join-Path $projectRoot 'dist'))
    if ([IO.Path]::GetDirectoryName($dist) -ne [IO.Path]::GetFullPath($projectRoot)) {
        throw 'Invalid dist path.'
    }
    if (Test-Path -LiteralPath $dist) {
        $distItem = Get-Item -LiteralPath $dist -Force
        if (!$distItem.PSIsContainer -or ($distItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'dist must be a regular directory, not a link.'
        }
        # Do not follow junctions or symbolic links outside the output directory.
        $directories = [Collections.Generic.Stack[string]]::new()
        $directories.Push($dist)
        while ($directories.Count -gt 0) {
            foreach ($entry in Get-ChildItem -LiteralPath $directories.Pop() -Force) {
                if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    throw "Cannot clean dist containing a link: $($entry.FullName)"
                }
                if ($entry.PSIsContainer) { $directories.Push($entry.FullName) }
            }
        }
        foreach ($entry in Get-ChildItem -LiteralPath $dist -Force) {
            $entryPath = [IO.Path]::GetFullPath($entry.FullName)
            if ([IO.Path]::GetDirectoryName($entryPath) -ne $dist) { throw 'Invalid cleanup path.' }
            Remove-Item -LiteralPath $entryPath -Recurse -Force
        }
    } else {
        New-Item -ItemType Directory -Path $dist | Out-Null
    }
    Write-Output "Clean output directory: $dist"
    if (!$SkipBuild) {
        cargo build --release --locked --no-default-features --bin frame-library-updater
        if ($LASTEXITCODE -ne 0) { throw 'Updater build failed.' }
        cargo build --release --locked --bin video-management
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    }
    $packageMetadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read application version.' }
    $version = ($packageMetadata.packages | Where-Object name -EQ 'video-management').version
    if (!$version) { throw 'Application version was not found.' }
    foreach ($program in @('video-management.exe', 'frame-library-updater.exe')) {
        # Explicit redirection/wait is required for Windows GUI-subsystem executables.
        $versionStart = [Diagnostics.ProcessStartInfo]::new((Join-Path $projectRoot "target/release/$program"), '--version')
        $versionStart.UseShellExecute = $false
        $versionStart.CreateNoWindow = $true
        $versionStart.RedirectStandardOutput = $true
        $versionStart.RedirectStandardError = $true
        $versionProcess = [Diagnostics.Process]::Start($versionStart)
        if (!$versionProcess.WaitForExit(10000)) { $versionProcess.Kill(); $versionProcess.WaitForExit(); throw "Version probe timed out: $program" }
        $binaryVersion = $versionProcess.StandardOutput.ReadToEnd().Trim()
        $versionExit = $versionProcess.ExitCode
        $versionProcess.Dispose()
        if ($versionExit -ne 0 -or $binaryVersion -ne $version) { throw "Binary version does not match Cargo.toml: $program" }
    }
    $baseName = "FrameLibrary-$version-windows-x64"
    $bundle = Join-Path $dist ('.package-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $bundle | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/video-management.exe') -Destination $bundle -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/frame-library-updater.exe') -Destination $bundle -Force
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vsRoot=& $vswhere -latest -products '*' -property installationPath
    Copy-PlaybackRuntime -GStreamerRoot $gstRoot -Bundle $bundle -VsRoot $vsRoot
    Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination $bundle -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'design.md') -Destination $bundle -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'VALIDATION.md') -Destination $bundle -Force
    $licenses=Join-Path $bundle 'licenses'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    $fontLicense = Join-Path $licenses 'source-han-sans-2.005'
    New-Item -ItemType Directory -Force -Path $fontLicense | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/fonts/LICENSE.txt'), (Join-Path $projectRoot 'assets/fonts/README.md') -Destination $fontLicense -Force
    $metadata=cargo metadata --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate dependency licenses.' }
    $notice=@('# Third-party components', '', 'GStreamer runtime: 1.26.11 (MSVC x64), dynamically linked.', 'Official binaries: https://gstreamer.freedesktop.org/data/pkg/windows/1.26.11/msvc/', 'Corresponding sources and build recipes: https://gitlab.freedesktop.org/gstreamer/cerbero/-/tree/1.26.11', 'GStreamer sources: https://gstreamer.freedesktop.org/src/', 'Microsoft Visual C++ Redistributable runtime libraries are included using the app-local deployment files provided by Visual Studio.', '', 'The runtime ships its component licenses under share/licenses. Rust dependency notices follow.', '')
    $notice += @('Bundled font: Source Han Sans CN 2.005 (Regular and Bold), embedded in video-management.exe.', 'Source: https://github.com/adobe-fonts/source-han-sans/tree/2.005R/SubsetOTF/CN', 'License: SIL Open Font License 1.1; see licenses/source-han-sans-2.005/LICENSE.txt and README.md.', '')
    foreach ($package in $metadata.packages) {
        if (!$package.source -and $package.name -notin @('iced_wgpu','iced_winit','iced_video_player')) { continue }
        $notice+="- $($package.name) $($package.version): $($package.license) | $($package.repository)"
        $packageRoot=Split-Path -Parent $package.manifest_path
        $texts=Get-ChildItem -LiteralPath $packageRoot -File | Where-Object Name -Match '^(LICENSE|LICENCE|COPYING|NOTICE|UNLICENSE)'
        if ($texts) {
            $dest=Join-Path $licenses "$($package.name)-$($package.version)"
            New-Item -ItemType Directory -Force -Path $dest | Out-Null
            $texts | Copy-Item -Destination $dest -Force
        }
    }
    [IO.File]::WriteAllLines((Join-Path $bundle 'THIRD_PARTY.md'), [string[]]$notice, $utf8NoBom)
    # Build only from this new staging directory, never from a user's portable installation.
    # Every enumerated file is below this absolute prefix; this also works on .NET Framework.
    $bundlePrefix = $bundle + [IO.Path]::DirectorySeparatorChar
    $files = @(Get-ChildItem -LiteralPath $bundle -Recurse -File | Sort-Object FullName | ForEach-Object {
        @{ path = $_.FullName.Substring($bundlePrefix.Length).Replace('\', '/');
           size = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
    $releaseJson = @{ product='FrameLibrary'; version=$version; platform='windows-x64'; files=$files } |
        ConvertTo-Json -Depth 6
    [IO.File]::WriteAllText((Join-Path $bundle 'release.json'), $releaseJson, $utf8NoBom)
    $zip = Join-Path $dist "$baseName.zip"
    $zipTemporary = Join-Path $dist ('.package-' + [guid]::NewGuid().ToString('N') + '.zip')
    Get-ChildItem -LiteralPath $licenses -Recurse -File | Where-Object LastWriteTime -LT ([datetime]'1980-01-01') | ForEach-Object { $_.LastWriteTime=[datetime]'1980-01-01' }
    # Use explicit ZIP paths: .NET Framework's CreateFromDirectory can emit backslashes.
    $zipArchive = [IO.Compression.ZipFile]::Open($zipTemporary, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in Get-ChildItem -LiteralPath $bundle -Recurse) {
            $entryPath = $entry.FullName.Substring($bundlePrefix.Length).Replace('\', '/')
            if ($entry.PSIsContainer) {
                if ([IO.Directory]::GetFileSystemEntries($entry.FullName).Length -eq 0) {
                    $null = $zipArchive.CreateEntry("$entryPath/")
                }
            } else {
                $null = [IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $zipArchive, $entry.FullName, $entryPath, [IO.Compression.CompressionLevel]::Optimal)
            }
        }
    } finally { $zipArchive.Dispose() }
    Move-Item -LiteralPath $zipTemporary -Destination $zip -Force
    $hash=Get-FileHash -LiteralPath $zip -Algorithm SHA256
    Set-Content -LiteralPath "$zip.sha256" -Value "$($hash.Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($zip))" -Encoding ascii
    $manifest = Join-Path $dist "$baseName.update.json"
    $manifestTemporary = "$manifest.tmp"
    $updateJson = @{ product='FrameLibrary'; version=$version; platform='windows-x64'; archive=[IO.Path]::GetFileName($zip);
       size=(Get-Item -LiteralPath $zip).Length; sha256=$hash.Hash.ToLowerInvariant() } |
        ConvertTo-Json
    [IO.File]::WriteAllText($manifestTemporary, $updateJson, $utf8NoBom)
    # Publish last. Consumers ignore unfinished ZIPs without a complete matching manifest.
    Move-Item -LiteralPath $manifestTemporary -Destination $manifest -Force
    $published = Join-Path $dist $baseName
    if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($published)) -ne $dist -or
        [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($bundle)) -ne $dist) { throw 'Invalid bundle path.' }
    if (Test-Path -LiteralPath $published) { throw 'Output directory was created by another process during packaging.' }
    Move-Item -LiteralPath $bundle -Destination $published
    $hash | Format-List
    Write-Output "Portable application: $published"
    Write-Output "Archive: $zip"
    Write-Output "Update manifest: $manifest"
} finally { Pop-Location }
