# Runs the rendering measurements on this PC's graphics card and writes
# results.md next to this script. Takes about two minutes; nothing is
# installed and nothing but results.md is written.
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $here

Write-Host "Measuring. This takes about two minutes; close games and leave the PC alone meanwhile."
& (Join-Path $here "bin\render-bench.exe") | Tee-Object -FilePath (Join-Path $here "results.md")
if ($LASTEXITCODE -ne 0) { Write-Host "The measurement stopped early (see above). Send what is in results.md anyway." }
Write-Host ""
Write-Host "Done. The results are in results.md. Send that file back."
Read-Host "Press Enter to close"
