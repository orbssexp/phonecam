<#
.SYNOPSIS
    Builds the patched PhoneCam DirectShow filter DLL and sender library from softcam source.
.PARAMETER SourceDir
    Path to the third-party softcam-src directory.
.PARAMETER OutputDir
    Destination path for the patched build tree and distribution layout.
#>
[CmdletBinding()]
param(
    [string]$SourceDir = "C:\Users\gusta\orca\projects\phone-cam\spike\vendor\softcam-src",
    [string]$OutputDir = "C:\Users\gusta\orca\projects\phone-cam\spike\vendor\softcam-phonecam"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $SourceDir)) {
    throw "Source directory not found: $SourceDir"
}

# Fixed CLSID generated for PhoneCam
$CLSID_GUID_STR = "{4CD75097-5A06-4341-9D29-8C12662AC2C0}"
$CLSID_DEFINE = "0x4cd75097, 0x5a06, 0x4341, 0x9d, 0x29, 0x8c, 0x12, 0x66, 0x2a, 0xc2, 0xc0"

Write-Host "=== Building PhoneCam Softcam ===" -ForegroundColor Cyan
Write-Host "Source dir: $SourceDir"
Write-Host "Output dir: $OutputDir"
Write-Host "CLSID:      $CLSID_GUID_STR"

# Locate vswhere and MSBuild
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    throw "vswhere.exe not found at $vswhere"
}

$msbuild = & $vswhere -latest -products * -find MSBuild\**\Bin\MSBuild.exe | Select-Object -First 1
if (-not $msbuild -or -not (Test-Path $msbuild)) {
    throw "MSBuild.exe not found via vswhere"
}
Write-Host "MSBuild:    $msbuild"

# 1) Copy source to $OutputDir\src
$targetSrc = Join-Path $OutputDir "src"
if (-not (Test-Path $targetSrc)) {
    New-Item -ItemType Directory -Path $targetSrc -Force | Out-Null
}

Write-Host "Copying source to $targetSrc..."
& robocopy $SourceDir $targetSrc /E /XD dist bin obj x64 Win32 /XF *.obj *.lib *.dll *.pdb | Out-Null
if ($LASTEXITCODE -gt 7) {
    throw "Robocopy failed with exit code $LASTEXITCODE"
}

# Clean any existing .git from target source
$gitDir = Join-Path $targetSrc ".git"
if (Test-Path $gitDir) {
    Remove-Item -Path $gitDir -Recurse -Force
}

# 2) Patch the copy
Write-Host "Applying patches..."
# softcam.cpp: CLSID and filter friendly name
$softcamCpp = Join-Path $targetSrc "src\softcam\softcam.cpp"
$content = Get-Content $softcamCpp -Raw
$content = $content -replace "// \{AEF3B972-5FA5-4647-9571-358EB472BC9E\}", "// $CLSID_GUID_STR"
$content = $content -replace "0xaef3b972, 0x5fa5, 0x4647, 0x95, 0x71, 0x35, 0x8e, 0xb4, 0x72, 0xbc, 0x9e", $CLSID_DEFINE
$content = $content -replace 'const wchar_t FILTER_NAME\[\] = L"DirectShow Softcam";', 'const wchar_t FILTER_NAME[] = L"PhoneCam";'
Set-Content -Path $softcamCpp -Value $content -NoNewline

# DShowSoftcam.cpp: internal name strings
$dshowCpp = Join-Path $targetSrc "src\softcamcore\DShowSoftcam.cpp"
$content = Get-Content $dshowCpp -Raw
$content = $content.Replace('NAME("DirectShow Softcam")', 'NAME("PhoneCam")')
$content = $content.Replace('L"DirectShow Softcam Stream"', 'L"PhoneCam Stream"')
$content = $content.Replace('NAME("DirectShow Softcam Stream")', 'NAME("PhoneCam Stream")')
Set-Content -Path $dshowCpp -Value $content -NoNewline

# FrameBuffer.cpp: named mutex and shared memory strings
$fbCpp = Join-Path $targetSrc "src\softcamcore\FrameBuffer.cpp"
$content = Get-Content $fbCpp -Raw
$content = $content.Replace('"DirectShow Softcam/NamedMutex"', '"PhoneCam/NamedMutex"')
$content = $content.Replace('"DirectShow Softcam/SharedMemory"', '"PhoneCam/SharedMemory"')
Set-Content -Path $fbCpp -Value $content -NoNewline

# 3) Build Release x64 and Release Win32 with MSBuild
$slnPath = Join-Path $targetSrc "softcam.sln"

Write-Host "Building Release x64..."
& $msbuild $slnPath /t:softcam /p:Configuration=Release /p:Platform=x64 /m
if ($LASTEXITCODE -ne 0) {
    throw "MSBuild Release x64 failed with exit code $LASTEXITCODE"
}

Write-Host "Building Release Win32..."
& $msbuild $slnPath /t:softcam /p:Configuration=Release /p:Platform=Win32 /m
if ($LASTEXITCODE -ne 0) {
    throw "MSBuild Release Win32 failed with exit code $LASTEXITCODE"
}

# 4) Lay out artifacts under $OutputDir matching SOFTCAM_DIR expectations
Write-Host "Staging distribution layout in $OutputDir..."
$distSrc = Join-Path $targetSrc "dist"
Copy-Item -Path "$distSrc\*" -Destination $OutputDir -Recurse -Force

# 5) Verify presence of artifacts
$dll64 = Join-Path $OutputDir "bin\x64\softcam.dll"
$dll32 = Join-Path $OutputDir "bin\Win32\softcam.dll"
$lib64 = Join-Path $OutputDir "lib\x64\softcam.lib"
$header = Join-Path $OutputDir "include\softcam\softcam.h"

foreach ($file in @($dll64, $dll32, $lib64, $header)) {
    if (-not (Test-Path $file)) {
        throw "Expected artifact not found: $file"
    }
}

# Strings check on x64 DLL
$bytes64 = [System.IO.File]::ReadAllBytes($dll64)
$strUnicode = [System.Text.Encoding]::Unicode.GetString($bytes64)
$strAscii = [System.Text.Encoding]::ASCII.GetString($bytes64)

if (-not ($strUnicode.Contains("PhoneCam") -or $strAscii.Contains("PhoneCam"))) {
    throw "Verification failed: 'PhoneCam' not found in $dll64"
}
if ($strUnicode.Contains("DirectShow Softcam") -or $strAscii.Contains("DirectShow Softcam")) {
    throw "Verification failed: 'DirectShow Softcam' still present in $dll64"
}

Write-Host "Build and verification succeeded!" -ForegroundColor Green
