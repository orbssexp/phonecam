<#
.SYNOPSIS
    Builds the signed PhoneCam installer and publishes a GitHub release the in-app updater can see.
.DESCRIPTION
    Fails unless tauri.conf.json, src-tauri/Cargo.toml and package.json already say -Version and
    the git tree is clean. Signs with %USERPROFILE%\.phonecam\updater.key (never printed).
    Output goes to dist-release\ (git-ignored). -DryRun does everything except `gh release create`.
.PARAMETER Version
    X.Y.Z, must match the three version files.
.PARAMETER Ipa
    Optional path to the iOS .ipa to attach to the release.
.PARAMETER Notes
    Release notes (default: commit subjects since the previous tag).
.PARAMETER DryRun
    Build, sign and write latest.json, but do not create the GitHub release.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [string]$Ipa,
    [string]$Notes,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$desktop = Join-Path $root "apps\desktop"
$keyPath = Join-Path $env:USERPROFILE ".phonecam\updater.key"
$out = Join-Path $root "dist-release"
$tag = "v$Version"

function Fail($m) { throw "release: $m" }
function Run($label, [scriptblock]$cmd) {
    & $cmd
    if ($LASTEXITCODE -ne 0) { Fail "$label failed (exit $LASTEXITCODE)" }
}

# ---- preconditions -------------------------------------------------------------------------
if (-not (Test-Path $keyPath)) { Fail "signing key not found at $keyPath (see docs/RELEASING.md)" }
if ($Ipa -and -not (Test-Path $Ipa)) { Fail "ipa not found: $Ipa" }

$confVer = (Get-Content (Join-Path $desktop "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$pkgVer = (Get-Content (Join-Path $desktop "package.json") -Raw | ConvertFrom-Json).version
$cargoText = Get-Content (Join-Path $desktop "src-tauri\Cargo.toml") -Raw
if ($cargoText -notmatch '(?m)^version\s*=\s*"([^"]+)"') { Fail "no version in src-tauri/Cargo.toml" }
$cargoVer = $Matches[1]
foreach ($pair in @(@("tauri.conf.json", $confVer), @("Cargo.toml", $cargoVer), @("package.json", $pkgVer))) {
    if ($pair[1] -ne $Version) { Fail "$($pair[0]) says $($pair[1]), expected $Version. Bump all three first." }
}

Push-Location $root
try {
    if (git status --porcelain) { Fail "git tree is not clean; commit or stash first" }
    $repo = (gh repo view --json nameWithOwner -q .nameWithOwner).Trim()
    if (-not $repo) { Fail "could not resolve the GitHub repo (gh logged in?)" }

    if (-not $DryRun) {
        git ls-remote --exit-code --tags origin "refs/tags/$tag" *> $null
        if ($LASTEXITCODE -eq 0) { Fail "tag $tag already exists on origin" }
        $global:LASTEXITCODE = 0
        git fetch -q origin
        if (-not (git branch -r --contains HEAD)) { Fail "HEAD is not pushed to origin; push first" }
    }

    if (-not $Notes) {
        $prev = git describe --tags --abbrev=0 2>$null
        $global:LASTEXITCODE = 0
        $range = if ($prev) { "$prev..HEAD" } else { "HEAD" }
        $Notes = (git log --no-merges --pretty="- %s" $range) -join "`n"
        if (-not $Notes) { $Notes = "PhoneCam $tag" }
    }

    # ---- build + sign ---------------------------------------------------------------------
    $env:TAURI_SIGNING_PRIVATE_KEY = $keyPath
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
    Push-Location $desktop
    try {
        Run "npm ci" { npm ci }
        Run "tauri build" { npm run tauri -- build --bundles nsis }
    } finally { Pop-Location }

    $bundle = Join-Path $root "target\release\bundle\nsis"
    $setup = Join-Path $bundle "PhoneCam_${Version}_x64-setup.exe"
    $sig = "$setup.sig"
    if (-not (Test-Path $setup)) { Fail "installer not produced: $setup" }
    if (-not (Test-Path $sig)) { Fail "signature not produced: $sig (is createUpdaterArtifacts on?)" }

    # ---- stage assets + latest.json -------------------------------------------------------
    if (Test-Path $out) { Remove-Item $out -Recurse -Force }
    New-Item -ItemType Directory $out | Out-Null
    Copy-Item $setup $out
    Copy-Item $sig $out
    $setupName = Split-Path $setup -Leaf
    $latest = [ordered]@{
        version  = $Version
        notes    = $Notes
        pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = [ordered]@{
            "windows-x86_64" = [ordered]@{
                signature = (Get-Content $sig -Raw).Trim()
                url       = "https://github.com/$repo/releases/download/$tag/$setupName"
            }
        }
    }
    $json = $latest | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText((Join-Path $out "latest.json"), $json, (New-Object Text.UTF8Encoding($false)))
    $check = Get-Content (Join-Path $out "latest.json") -Raw | ConvertFrom-Json
    if ($check.version -ne $Version -or -not $check.platforms.'windows-x86_64'.signature) { Fail "latest.json is invalid" }

    $assets = @((Join-Path $out $setupName), (Join-Path $out "$setupName.sig"), (Join-Path $out "latest.json"))
    if ($Ipa) { $assets += (Resolve-Path $Ipa).Path }

    Write-Host "Assets:" -ForegroundColor Cyan
    $assets | ForEach-Object { Write-Host "  $_" }

    if ($DryRun) {
        Write-Host "DRY RUN: not creating GitHub release $tag on $repo." -ForegroundColor Yellow
        return
    }

    # ---- publish --------------------------------------------------------------------------
    $notesFile = Join-Path $out "notes.txt"
    [IO.File]::WriteAllText($notesFile, $Notes, (New-Object Text.UTF8Encoding($false)))
    Run "gh release create" { gh release create $tag @assets --repo $repo --title "PhoneCam $tag" --notes-file $notesFile --target (git rev-parse HEAD) }
    Write-Host "Released $tag: https://github.com/$repo/releases/tag/$tag" -ForegroundColor Green
} finally {
    Pop-Location
}
