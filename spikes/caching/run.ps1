# Runs the caching measurements on this PC: makes the test clips (first time
# only), measures every approach on them and on any recordings you put in the
# "recordings" folder, and writes results.md next to this script.
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $here
$env:PATH = "$here\bin;$env:PATH"
$clips = Join-Path $here "clips"
$work = Join-Path $here "work"
New-Item -ItemType Directory -Force -Path $clips, $work, (Join-Path $here "recordings") | Out-Null

function Make-Clip($name, $size, $rate, $keys, [string[]]$codec) {
    $out = Join-Path $clips $name
    if (Test-Path $out) { return }
    Write-Host "Making test clip $name (only the first time)..."
    $src = "testsrc2=size=${size}:rate=${rate},noise=alls=18:allf=t+u,format=yuv420p"
    & ffmpeg -hide_banner -loglevel error -f lavfi -i $src -t 20 -g $keys @codec $out
    if ($LASTEXITCODE -ne 0) { throw "making $name failed" }
}

$x265 = "log-level=error:bframes=0:scenecut=0"
Make-Clip "h264-1080p60-2s.mp4" "1920x1080" 60 120 @("-c:v","libx264","-preset","veryfast","-bf","0","-sc_threshold","0","-b:v","20M","-maxrate","20M","-bufsize","40M")
Make-Clip "h265-1440p60-2s.mp4" "2560x1440" 60 120 @("-c:v","libx265","-preset","ultrafast","-b:v","40M","-x265-params",$x265,"-tag:v","hvc1")
Make-Clip "h265-1440p60-8s.mp4" "2560x1440" 60 480 @("-c:v","libx265","-preset","ultrafast","-b:v","40M","-x265-params",$x265,"-tag:v","hvc1")
Make-Clip "av1-1440p60-2s.mkv"  "2560x1440" 60 120 @("-c:v","libsvtav1","-preset","12","-b:v","30M")

$files = @(Get-ChildItem $clips -File | ForEach-Object FullName)
$files += @(Get-ChildItem (Join-Path $here "recordings") -File -Include *.mp4,*.mkv,*.mov -Recurse | ForEach-Object FullName)

Write-Host "Measuring $($files.Count) clips. This takes about 10 minutes a clip; leave the PC alone meanwhile."
& (Join-Path $here "bin\cache-bench.exe") --decode d3d11va --work $work @files | Tee-Object -FilePath (Join-Path $here "results.md")
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
Write-Host ""
Write-Host "Done. The results are in results.md. Send that file back."
Read-Host "Press Enter to close"
