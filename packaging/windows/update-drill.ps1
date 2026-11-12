# The update drill: proves on a clean machine that an installed copy of the
# previous release ends up on this one, by itself, and then says it is up to
# date. The release workflow runs it twice:
#
#   before publishing   the previous release reads the list of releases from
#                       this machine (the files in $Dist), so a release that
#                       can't be updated to is never published;
#   -Published          the previous release asks GitHub itself, as installed
#                       copies do, so a release they can't find is noticed
#                       within minutes.
#
# It also installs this build on its own and checks that it can be drilled,
# so the next release always has a previous one to start from.
#
# Every step is the launcher's own `--update-step` (crates/launcher/src/drill.rs):
# install what is waiting, look for a newer version and download it, start
# the editor and wait for it to report that it works.
#
# Needs the GitHub CLI with a token (GH_TOKEN), and Python for the local list.

param(
    [Parameter(Mandatory)] [string]$Repo,
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [string]$Version,
    [string]$Dist = "dist",
    [switch]$Published
)
$ErrorActionPreference = "Stop"

# It installs, uninstalls and empties the app's data folder: never on a
# machine somebody uses.
if ($env:GITHUB_ACTIONS -ne "true") { throw "The update drill only runs on a workflow runner: it empties the app's data folder." }

$port = 8765
$work = Join-Path $env:RUNNER_TEMP "update-drill"
$data = Join-Path $env:LOCALAPPDATA $Name
$feedVariable = "$($Name.ToUpper())_UPDATE_FEED"
$tokenVariable = "$($Name.ToUpper())_UPDATE_TOKEN"
if (-not $Published) { $Dist = (Resolve-Path $Dist).Path }
New-Item -ItemType Directory -Force $work | Out-Null

# A version as text that sorts in version order, or $null for a tag that is
# not vX.Y.Z or vX.Y.Z-beta.N. A stable version sorts after its betas.
function Get-VersionKey([string]$text) {
    if ($text -notmatch '^v?(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$') { return $null }
    $stable = if ($Matches[4]) { 0 } else { 1 }
    "{0:D6}.{1:D6}.{2:D6}.{3}.{4:D9}" -f [int]$Matches[1], [int]$Matches[2], [int]$Matches[3], $stable, [int]("0" + $Matches[4])
}

function Install-Copy([string]$setup, [string]$dir) {
    Start-Process -Wait $setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER", "/DIR=$dir"
    if (-not (Test-Path "$dir/launcher.exe")) { throw "$setup did not install a launcher into $dir." }
}

function Uninstall-Copy([string]$dir) {
    if (Test-Path "$dir/unins000.exe") {
        Start-Process -Wait "$dir/unins000.exe" -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES"
    }
    # A clean machine again: no settings, downloads or start records left.
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

function Show-Logs {
    Get-ChildItem "$data/logs" -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "---- $($_.Name) (last 60 lines)"
        Get-Content $_.FullName -Tail 60
    }
}

# Runs one update step of the copy in $dir and returns its report.
function Invoke-Step([string]$dir, [string]$label) {
    $report = Join-Path $work "report.txt"
    Remove-Item $report -ErrorAction SilentlyContinue
    Start-Process "$dir/launcher.exe" -ArgumentList "--update-step", $report | Out-Null
    # The launcher may hand over to a newer launcher, which then reports: wait for the file, not the program.
    $deadline = (Get-Date).AddMinutes(6)
    while (-not (Test-Path $report)) {
        if ((Get-Date) -gt $deadline) {
            Show-Logs
            throw "$label`: no report within 6 minutes."
        }
        Start-Sleep -Milliseconds 500
    }
    $facts = @{}
    Get-Content $report | ForEach-Object {
        $key, $value = $_ -split "=", 2
        $facts[$key] = $value
    }
    Write-Host "$label`: launcher $($facts.launcher), in use $($facts.in_use), installed '$($facts.installed)', check '$($facts.check)', editor '$($facts.editor)'"
    $facts
}

function Assert-Fact($facts, [string]$key, [string]$expected, [string]$label) {
    if ($facts[$key] -ne $expected) {
        Show-Logs
        throw "$label`: $key is '$($facts[$key])', expected '$expected'."
    }
}

# ---- The list of releases this machine serves: just this one. ----
$server = $null
if (-not $Published) {
    $files = @("$Name-$Version-windows-x64.zip", "manifest.json", "manifest.json.sig")
    $assets = foreach ($file in $files) {
        $path = Join-Path $Dist $file
        if (-not (Test-Path $path)) { throw "$file is missing from $Dist." }
        @{ name = $file; browser_download_url = "http://127.0.0.1:$port/$file"; size = (Get-Item $path).Length }
    }
    $release = @{ tag_name = "v$Version"; draft = $false; prerelease = $true; body = "update drill"; assets = @($assets) }
    ConvertTo-Json -Depth 5 -InputObject @($release) | Set-Content -Encoding utf8 (Join-Path $Dist "releases.json")
    $server = Start-Process python -ArgumentList "-m", "http.server", $port, "--bind", "127.0.0.1", "--directory", $Dist -PassThru -WindowStyle Hidden
    $up = $false
    foreach ($try in 1..40) {
        try { Invoke-WebRequest "http://127.0.0.1:$port/releases.json" -UseBasicParsing | Out-Null; $up = $true; break }
        catch { Start-Sleep -Milliseconds 250 }
    }
    if (-not $up) { throw "The local list of releases did not come up on port $port." }
    Set-Item "env:$feedVariable" "http://127.0.0.1:$port/releases.json"
} else {
    # GitHub limits unnamed callers by address, and runners share theirs.
    Set-Item "env:$tokenVariable" $env:GH_TOKEN
}

try {
    # ---- 1. This build on its own can be drilled, and is content with itself. ----
    if (-not $Published) {
        $own = Join-Path $work "this"
        Install-Copy (Join-Path $Dist "$Name-Setup-$Version.exe") $own
        $step = Invoke-Step $own "This build"
        Assert-Fact $step "launcher" $Version "This build"
        Assert-Fact $step "in_use" $Version "This build"
        Assert-Fact $step "check" "up to date" "This build"
        Assert-Fact $step "editor" "started" "This build"
        Uninstall-Copy $own
    }

    # ---- 2. The previous release ends up on this one. ----
    $releases = gh api --paginate "repos/$Repo/releases?per_page=100" --jq '.[] | select(.draft | not) | .tag_name'
    $mine = Get-VersionKey $Version
    if (-not $mine) { throw "$Version is not a version this drill can order." }
    $previous = $releases |
        Where-Object { $key = Get-VersionKey $_; $key -and $key -lt $mine } |
        Sort-Object { Get-VersionKey $_ } |
        Select-Object -Last 1
    if (-not $previous) {
        Write-Host "::notice::No earlier release to update from: nothing more to drill."
        return
    }
    $from = $previous.TrimStart("v")
    Write-Host "Updating from $from to $Version."
    $downloads = Join-Path $work "previous"
    New-Item -ItemType Directory -Force $downloads | Out-Null
    gh release download $previous --repo $Repo --pattern "*Setup*.exe" --dir $downloads --clobber
    if ($LASTEXITCODE -ne 0) { throw "The installer of $previous could not be downloaded." }
    $setup = (Get-ChildItem $downloads -Filter "*Setup*.exe" | Select-Object -First 1).FullName
    $dir = Join-Path $work "installed"
    Install-Copy $setup $dir

    # Releases from before the drill existed can't take part. Every release
    # since has passed part 1 above, so this only happens for those.
    $launcher = [System.IO.File]::ReadAllBytes("$dir/launcher.exe")
    if (-not [System.Text.Encoding]::ASCII.GetString($launcher).Contains("--update-step")) {
        Write-Host "::notice::$from was made before the update drill existed, so it can't be drilled. The next release can."
        Uninstall-Copy $dir
        return
    }

    $step = Invoke-Step $dir "Start 1 ($from finds and downloads $Version)"
    # Asking GitHub itself, a release newer than this one may be out already
    # (another run published meanwhile). Installed copies then go straight to
    # that one, as they should, and its own run checks it.
    if ($Published -and $step.check -match '^downloaded (.+)$') {
        $found = $Matches[1]
        $foundKey = Get-VersionKey $found
        if ($foundKey -and $foundKey -gt $mine) {
            Write-Host "::notice::$found was published meanwhile and is newer than $Version`: installed copies update to that one. Nothing more to check here."
            Uninstall-Copy $dir
            return
        }
    }
    Assert-Fact $step "in_use" $from "Start 1"
    Assert-Fact $step "check" "downloaded $Version" "Start 1"
    Assert-Fact $step "editor" "started" "Start 1"

    $step = Invoke-Step $dir "Start 2 ($Version is installed and starts)"
    Assert-Fact $step "in_use" $Version "Start 2"
    # The launcher program may still be the old one here: it must not offer the update again.
    Assert-Fact $step "check" "up to date" "Start 2"
    Assert-Fact $step "editor" "started" "Start 2"
    if ((Get-Content "$dir/current") -ne $Version) { throw "Start 2: the current-version file does not say $Version." }

    $step = Invoke-Step $dir "Start 3 (the new launcher has taken over)"
    Assert-Fact $step "launcher" $Version "Start 3"
    Assert-Fact $step "in_use" $Version "Start 3"
    Assert-Fact $step "check" "up to date" "Start 3"
    Assert-Fact $step "editor" "started" "Start 3"

    Uninstall-Copy $dir
    Write-Host "The update from $from to $Version works."
}
finally {
    if ($server) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
}
