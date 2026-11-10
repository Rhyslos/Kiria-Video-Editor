# Runs the decoding measurements on this PC: makes the test clips (first time
# only), measures every approach on them and on any recordings you put in the
# "recordings" folder, and writes results.md next to this script.
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $here
$env:PATH = "$here\ffmpeg;$env:PATH"
$clips = Join-Path $here "clips"
New-Item -ItemType Directory -Force -Path $clips, (Join-Path $here "recordings") | Out-Null

function Make-Clip($name, $size, $rate, [string[]]$codec) {
    $out = Join-Path $clips $name
    if (Test-Path $out) { return }
    Write-Host "Making test clip $name (only the first time)..."
    $src = "testsrc2=size=${size}:rate=${rate},noise=alls=18:allf=t+u,format=yuv420p"
    & ffmpeg -hide_banner -loglevel error -f lavfi -i $src -t 20 -g ([int]$rate * 2) @codec $out
    if ($LASTEXITCODE -ne 0) { throw "making $name failed" }
}

Make-Clip "h264-1080p60.mp4"  "1920x1080" 60  @("-c:v","libx264","-preset","veryfast","-b:v","20M","-maxrate","20M","-bufsize","40M")
Make-Clip "h265-1440p60.mp4"  "2560x1440" 60  @("-c:v","libx265","-preset","ultrafast","-b:v","40M","-x265-params","log-level=error","-tag:v","hvc1")
Make-Clip "h265-1440p144.mkv" "2560x1440" 144 @("-c:v","libx265","-preset","ultrafast","-b:v","60M","-x265-params","log-level=error")
Make-Clip "av1-1440p60.mkv"   "2560x1440" 60  @("-c:v","libsvtav1","-preset","12","-b:v","30M")

$files = @(Get-ChildItem $clips -File | ForEach-Object FullName)
$files += @(Get-ChildItem (Join-Path $here "recordings") -File -Include *.mp4,*.mkv,*.mov -Recurse | ForEach-Object FullName)

Write-Host "Measuring $($files.Count) clips. This takes a few minutes; leave the PC alone meanwhile."
& (Join-Path $here "decode-bench.exe") @files | Tee-Object -FilePath (Join-Path $here "results.md")
Write-Host ""
Write-Host "Done. The results are in results.md. Send that file back."
Read-Host "Press Enter to close"
