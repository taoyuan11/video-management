param([Parameter(Mandatory)][string]$Url,[Parameter(Mandatory)][string]$OutputPath)
$ErrorActionPreference='Stop'
$length=[long](Invoke-WebRequest -Method Head -Uri $Url -TimeoutSec 60).Headers['Content-Length'][0]
if ($length -le 0) { throw 'Download size is missing or invalid.' }
# Bounded ranges keep large SDK downloads retryable on connections that stall.
$chunk=[long](1MB)
$partCount=[int][Math]::Ceiling($length/$chunk)
Write-Output "Downloading $length bytes in $partCount ranges: $Url"
$parts=0..($partCount-1) | ForEach-Object -Parallel {
    $ErrorActionPreference='Stop'
    $part=$_; $start=$part*$using:chunk; $end=[Math]::Min(($part+1)*$using:chunk-1,$using:length-1)
    $destination="$using:OutputPath.range$start-$end"
    if (!(Test-Path -LiteralPath $destination) -or (Get-Item -LiteralPath $destination).Length -ne ($end-$start+1)) {
        $PSNativeCommandUseErrorActionPreference=$false
        $curlOutput = & curl.exe --silent --show-error --fail --location --retry 3 --retry-all-errors --connect-timeout 30 --max-time 120 --speed-time 30 --speed-limit 1024 --range "$start-$end" --output $destination $using:Url 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Download failed: part $part`n$curlOutput" }
    }
    if ((Get-Item -LiteralPath $destination).Length -ne ($end-$start+1)) { throw "Invalid range response: part $part" }
    $part
} -ThrottleLimit 8
if ($parts.Count -ne $partCount) { throw 'Download did not complete.' }
$stream=[IO.File]::Create($OutputPath)
try {
    foreach ($part in 0..($partCount-1)) {
        $start=$part*$chunk; $end=[Math]::Min(($part+1)*$chunk-1,$length-1)
        $source=[IO.File]::OpenRead("$OutputPath.range$start-$end")
        try { $source.CopyTo($stream) } finally { $source.Dispose() }
    }
} finally { $stream.Dispose() }
