param([Parameter(Mandatory=$true)][string]$Bundle, [switch]$SkipBuild)
. "$PSScriptRoot/env.ps1"
$Bundle = (Resolve-Path -LiteralPath $Bundle).Path
if (!(Test-Path -LiteralPath (Join-Path $Bundle 'release.json'))) { throw 'Expected a built portable package.' }
Push-Location $projectRoot
$probe = Join-Path $Bundle ('.runtime-test-' + [guid]::NewGuid().ToString('N') + '.exe')
$qa = Join-Path $projectRoot ('.qa/runtime-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $qa | Out-Null
try {
    if (!$SkipBuild) {
        cargo build --release --locked --example player_smoke
        if ($LASTEXITCODE -ne 0) { throw 'Cannot build headless playback test.' }
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/examples/player_smoke.exe') -Destination $probe
    # Encoding helpers only run from the SDK to create disposable fixtures;
    # none of these encoders or test sources is needed in the portable package.
    $formats = [ordered]@{
        'h264.mp4' = 'x264enc tune=zerolatency ! h264parse ! mux.video_0 audiotestsrc volume=0 num-buffers=141 ! audio/x-raw,rate=48000 ! avenc_aac ! aacparse ! mux.audio_0 mp4mux name=mux'
        'hevc.mkv' = 'x265enc log-level=none ! h265parse ! matroskamux'
        'vp8.webm' = 'vp8enc deadline=1 ! webmmux'
        'vp9.webm' = 'vp9enc deadline=1 ! webmmux'
        'mjpeg.avi' = 'jpegenc ! avimux'
        'h264.flv' = 'x264enc tune=zerolatency ! h264parse ! flvmux'
        'h264.ts' = 'x264enc tune=zerolatency ! h264parse ! mpegtsmux'
        'mpeg2.mpg' = 'avenc_mpeg2video ! mpegvideoparse ! mpegpsmux'
        'wmv.wmv' = 'avenc_wmv2 ! asfmux'
    }
    foreach ($name in $formats.Keys) {
        $fixture = Join-Path $qa $name
        $pipeline = "videotestsrc num-buffers=90 ! video/x-raw,format=I420,width=320,height=240,framerate=30/1 ! $($formats[$name]) ! filesink"
        & (Join-Path $gstRoot 'bin/gst-launch-1.0.exe') -q @($pipeline.Split(' ')) ('location="' + $fixture.Replace('\', '/') + '"')
        if ($LASTEXITCODE -ne 0) { throw "Fixture generation failed: $name" }
        if (!(Test-Path -LiteralPath $fixture -PathType Leaf)) { throw "Missing generated fixture: $fixture" }
        $start = [Diagnostics.ProcessStartInfo]::new($probe, ('"' + $fixture + '"'))
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        # This process must not discover SDK/system-installed GStreamer plugins
        # or reuse the developer's registry. It loads only the packaged runtime.
        $start.EnvironmentVariables['PATH'] = "$env:SystemRoot\System32;$env:SystemRoot"
        $start.EnvironmentVariables['GST_PLUGIN_PATH_1_0'] = ''
        $start.EnvironmentVariables['GST_PLUGIN_SYSTEM_PATH_1_0'] = Join-Path $Bundle 'lib/gstreamer-1.0'
        $start.EnvironmentVariables['GST_PLUGIN_SCANNER_1_0'] = Join-Path $Bundle 'libexec/gstreamer-1.0/gst-plugin-scanner.exe'
        $start.EnvironmentVariables['GST_REGISTRY_1_0'] = Join-Path $qa 'registry.bin'
        $process = [Diagnostics.Process]::Start($start)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (!$process.WaitForExit(30000)) { throw "Playback probe timed out: $name" }
            Write-Output $stdout.Result
            if ($process.ExitCode -ne 0) { throw "Playback probe failed: $name`n$($stderr.Result)" }
        } finally {
            if (!$process.HasExited) {
                & taskkill /PID $process.Id /T /F | Out-Null
                $process.WaitForExit()
            }
            $process.Dispose()
        }
    }
    Write-Output "Portable playback and covers passed for $($formats.Count) generated formats. Fixtures: $qa"
} finally {
    # Delete only the uniquely named test executable created above.
    if (Test-Path -LiteralPath $probe) { Remove-Item -LiteralPath $probe -Force }
    Pop-Location
}
