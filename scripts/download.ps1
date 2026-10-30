param([Parameter(Mandatory)][string]$Url,[Parameter(Mandatory)][string]$OutputPath)
$ErrorActionPreference='Stop'
$length=[long](Invoke-WebRequest -Method Head -Uri $Url).Headers['Content-Length'][0]
$chunk=[long][Math]::Ceiling($length/8)
$parts=0..7 | ForEach-Object -Parallel {
    $part=$_; $start=$part*$using:chunk; $end=[Math]::Min(($part+1)*$using:chunk-1,$using:length-1)
    $destination="$using:OutputPath.part$part"
    if (!(Test-Path -LiteralPath $destination) -or (Get-Item -LiteralPath $destination).Length -ne ($end-$start+1)) {
        & curl.exe --silent --show-error --fail --location --retry 3 --range "$start-$end" --output $destination $using:Url
        if ($LASTEXITCODE -ne 0) { throw "Download failed: part $part" }
    }
    if ((Get-Item -LiteralPath $destination).Length -ne ($end-$start+1)) { throw "Invalid range response: part $part" }
    $part
} -ThrottleLimit 8
if ($parts.Count -ne 8) { throw 'Download did not complete.' }
$stream=[IO.File]::Create($OutputPath)
try {
    foreach ($part in 0..7) {
        $source=[IO.File]::OpenRead("$OutputPath.part$part")
        try { $source.CopyTo($stream) } finally { $source.Dispose() }
    }
} finally { $stream.Dispose() }
