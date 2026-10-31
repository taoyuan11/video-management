# Local-file playback profile for the pinned GStreamer 1.28.8 MSVC x64 SDK.
# Keep this list explicit: plugin discovery loads DLLs dynamically, so PE imports
# alone cannot discover the demuxers, decoders and Windows audio/video backends.
function Get-PlaybackPlugins {
    @(
        'coreelements', 'app', 'playback', 'typefindfunctions',
        'audioconvert', 'audioresample', 'audioparsers', 'volume', 'autodetect',
        'videoconvertscale', 'videofilter', 'videoparsersbad', 'deinterlace',
        'isomp4', 'matroska', 'avi', 'asf', 'flv', 'mpegpsdemux', 'mpegtsdemux',
        'ogg', 'wavparse', 'aiff', 'auparse', 'rawparse', 'id3demux', 'apetag',
        'libav', 'vpx', 'dav1d', 'theora', 'jpeg', 'png', 'openjpeg', 'wic',
        'a52dec', 'dtsdec', 'mpg123', 'flac', 'opus', 'vorbis', 'speex',
        'wavpack', 'alaw', 'mulaw', 'adpcmdec', 'amrnb', 'amrwbdec', 'dv',
        'wasapi', 'wasapi2', 'directsound',
        'd3d11', 'd3d12', 'mediafoundation', 'nvcodec', 'qsv', 'codecalpha',
        'subparse', 'assrender', 'pango', 'dvdspu', 'dvbsuboverlay', 'closedcaption'
    )
}

function Get-PeImports([string]$Path, [string]$Dumpbin) {
    # dumpbin includes both ordinary and delay-loaded imports. It is part of the
    # same x64 MSVC toolchain already required to build the application.
    $output = & $Dumpbin /nologo /dependents $Path 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Cannot inspect native dependencies: $Path`n$output" }
    foreach ($line in $output) {
        if ("$line" -match '^\s+([\w.+-]+\.dll)\s*$') { $Matches[1] }
    }
}

function Copy-PlaybackRuntime([string]$GStreamerRoot, [string]$Bundle, [string]$VsRoot) {
    $toolset = Get-ChildItem -LiteralPath (Join-Path $VsRoot 'VC/Tools/MSVC') -Directory |
        Sort-Object Name -Descending | Select-Object -First 1
    $dumpbin = Join-Path $toolset.FullName 'bin/Hostx64/x64/dumpbin.exe'
    if (!(Test-Path -LiteralPath $dumpbin)) { throw 'MSVC x64 dumpbin.exe was not found.' }
    $redist = Get-ChildItem -LiteralPath (Join-Path $VsRoot 'VC/Redist/MSVC') -Directory |
        Where-Object Name -Match '^\d' | Sort-Object Name -Descending | Select-Object -First 1
    $crt = Get-ChildItem -LiteralPath (Join-Path $redist.FullName 'x64') -Directory -Filter '*.CRT' |
        Select-Object -First 1
    if (!$crt) { throw 'MSVC x64 redistributable DLLs were not found.' }

    $pluginDir = Join-Path $Bundle 'lib/gstreamer-1.0'
    $scannerDir = Join-Path $Bundle 'libexec/gstreamer-1.0'
    New-Item -ItemType Directory -Force -Path $pluginDir,$scannerDir | Out-Null
    $queue = [Collections.Generic.Queue[string]]::new()
    foreach ($program in @('video-management.exe', 'frame-library-updater.exe')) {
        $queue.Enqueue((Join-Path $Bundle $program))
    }
    foreach ($name in Get-PlaybackPlugins) {
        $file = "gst$name.dll"
        $source = Join-Path $GStreamerRoot "lib/gstreamer-1.0/$file"
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing playback plugin: $file" }
        Copy-Item -LiteralPath $source -Destination $pluginDir
        $queue.Enqueue((Join-Path $pluginDir $file))
    }
    $scanner = 'gst-plugin-scanner.exe'
    Copy-Item -LiteralPath (Join-Path $GStreamerRoot "libexec/gstreamer-1.0/$scanner") -Destination $scannerDir
    $queue.Enqueue((Join-Path $scannerDir $scanner))

    # These shader compilers can be loaded by name instead of PE imports.
    foreach ($name in @('dxcompiler.dll', 'dxil.dll')) {
        $source = Join-Path $GStreamerRoot "bin/$name"
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            Copy-Item -LiteralPath $source -Destination $Bundle
            $queue.Enqueue((Join-Path $Bundle $name))
        }
    }
    $visited = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    while ($queue.Count -gt 0) {
        $binary = $queue.Dequeue()
        if (!$visited.Add($binary)) { continue }
        foreach ($name in Get-PeImports $binary $dumpbin) {
            $destination = Join-Path $Bundle $name
            if (Test-Path -LiteralPath $destination -PathType Leaf) { continue }
            # Prefer app-local runtime files to machine-installed copies, so a
            # developer's PATH or installed redistributable cannot hide a gap.
            $source = @((Join-Path $GStreamerRoot "bin/$name"), (Join-Path $crt.FullName $name)) |
                Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
            if ($source) {
                Copy-Item -LiteralPath $source -Destination $destination
                $queue.Enqueue($destination)
            } elseif ($name -notmatch '^(api-ms-win-|ext-ms-win-)' -and
                !(Test-Path -LiteralPath (Join-Path $env:SystemRoot "System32/$name") -PathType Leaf)) {
                throw "Unresolved runtime dependency $name imported by $binary"
            }
        }
    }
    # Preserve license texts and font configuration used by subtitle renderers;
    # omit SDK metadata, translations, build tools and unrelated helper programs.
    foreach ($directory in @('share/licenses', 'etc/fonts', 'share/fontconfig')) {
        $source = Join-Path $GStreamerRoot $directory
        if (Test-Path -LiteralPath $source) {
            $destination = Join-Path $Bundle $directory
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            & robocopy $source $destination /E /XF *.pdb *.lib *.a *.h /NFL /NDL /NJH /NJS /NP | Out-Null
            if ($LASTEXITCODE -ge 8) { throw "Runtime copy failed: $directory" }
        }
    }
    Write-Output "Playback runtime: $(@(Get-PlaybackPlugins).Count) plugins, $(@(Get-ChildItem -LiteralPath $Bundle -Filter '*.dll').Count) DLLs"
}
