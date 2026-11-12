# Runs the highlight finder measurements on this PC: makes the synthetic test
# clips (first time only), runs every approach on them and on the recordings
# in the "recordings" folder, scores them against the picked moments in each
# recording's .csv, and writes results.md and the "found" folder next to
# this script.
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $here
$env:PATH = "$here\bin;$env:PATH"
$bench = Join-Path $here "bin\highlight-bench.exe"
$clips = Join-Path $here "clips"
$recordings = Join-Path $here "recordings"
New-Item -ItemType Directory -Force -Path $clips, $recordings | Out-Null

Write-Host "Making the synthetic test clips (only the first time, a few minutes)..."
& $bench make-clips $clips --seconds 180
if ($LASTEXITCODE -ne 0) { throw "making the test clips failed" }

$files = @(Get-ChildItem $clips -File -Filter *.mkv | ForEach-Object FullName)
$mine = @(Get-ChildItem $recordings -File -Include *.mp4,*.mkv,*.mov -Recurse | ForEach-Object FullName)
foreach ($r in $mine) {
    $list = [IO.Path]::ChangeExtension($r, ".csv")
    if (-not (Test-Path $list)) {
        Write-Host "No picked moments for $(Split-Path -Leaf $r) (expected $(Split-Path -Leaf $list)): it is measured but not scored."
    }
}
$files += $mine

Write-Host "Measuring $($files.Count) recordings. About 10 to 15 minutes per half hour of recording; leave the PC alone meanwhile."
& $bench --decode d3d11va --decode software --frames all,keys --found (Join-Path $here "found") @files | Tee-Object -FilePath (Join-Path $here "results.md")
Write-Host ""
Write-Host "Done. The results are in results.md, and the moments each approach found are in the found folder. Send results.md back."
Read-Host "Press Enter to close"
