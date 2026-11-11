# Builds the Windows install layout in $Stage:
#   launcher.exe, current, versions/<version>/{editor.exe, launcher.exe, FFmpeg DLLs, licenses/}
# and the update package <name>-<version>-windows-x64.zip (the versions/<version> folder).
#
# Run from the repository root after `cargo build --release -p app -p launcher`.
# Needs APP_NAME and APP_VERSION in the environment (`cargo xtask brand`).

param(
    [string]$Stage = "target/stage-windows",
    [string]$Out = "target/dist"
)
$ErrorActionPreference = "Stop"

$name = $env:APP_NAME
$version = $env:APP_VERSION
if (-not $name -or -not $version) { throw "APP_NAME and APP_VERSION must be set (cargo xtask brand)." }

Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
$versionDir = Join-Path $Stage "versions/$version"
$licenses = Join-Path $versionDir "licenses"
New-Item -ItemType Directory -Force $licenses, $Out | Out-Null

Copy-Item target/release/launcher.exe $Stage
# Shortcuts and the uninstall entry show this icon; both programs also carry
# it built in. Each version brings a copy so updates can refresh it.
Copy-Item packaging/icons/icon.ico $Stage
Copy-Item packaging/icons/icon.ico $versionDir
Copy-Item target/release/editor.exe $versionDir
# A copy of the launcher travels in every version, so an update can replace the installed one.
Copy-Item target/release/launcher.exe $versionDir
Set-Content -NoNewline -Path (Join-Path $Stage "current") -Value $version

# FFmpeg: shared libraries and licence.
$ffmpegUrl = (Get-Content packaging/ffmpeg.env | Where-Object { $_ -match '^FFMPEG_WINDOWS_URL=' }) -replace '^FFMPEG_WINDOWS_URL=', ''
$download = Join-Path $env:RUNNER_TEMP "ffmpeg.zip"
if (-not $env:RUNNER_TEMP) { $download = "target/ffmpeg.zip" }
Invoke-WebRequest -Uri $ffmpegUrl -OutFile $download
$extract = "target/ffmpeg-windows"
Remove-Item -Recurse -Force $extract -ErrorAction SilentlyContinue
Expand-Archive $download -DestinationPath $extract
$root = Get-ChildItem $extract | Select-Object -First 1
Copy-Item (Join-Path $root.FullName "bin/*.dll") $versionDir
Copy-Item (Join-Path $root.FullName "LICENSE.txt") (Join-Path $licenses "FFmpeg-LICENSE.txt")
& (Join-Path $root.FullName "bin/ffmpeg.exe") -hide_banner -version | Select-Object -First 1 |
    Set-Content (Join-Path $licenses "FFMPEG-VERSION.txt")
"SHA-256 of the downloaded archive: " + (Get-FileHash $download -Algorithm SHA256).Hash.ToLower() |
    Add-Content (Join-Path $licenses "FFMPEG-VERSION.txt")

# Our licence and the bundled fonts' licences.
Copy-Item LICENSE (Join-Path $licenses "LICENSE.txt")
Copy-Item crates/ui-kit/assets/fonts/*-OFL.txt $licenses

# The update package: the contents of versions/<version>.
$package = Join-Path $Out "$name-$version-windows-x64.zip"
Remove-Item -Force $package -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $versionDir "*") -DestinationPath $package
Write-Host "Staged $Stage and wrote $package"
