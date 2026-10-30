. "$PSScriptRoot/env.ps1"
Push-Location $projectRoot
try { cargo run --locked; if ($LASTEXITCODE -ne 0) { throw 'Application exited with an error.' } }
finally { Pop-Location }
