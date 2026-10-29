# Checkpoint 3 driver: for each decoder, start spike-rx, point a fake-camera Edge/Chrome sender at it, collect metrics.csv.
# Usage: powershell -File measure.ps1 -Secs 130 -Decoders d3d11,cuda,cpu -Res "1920x1080@30" -Fps 30
param([int]$Secs = 130, [string]$Decoders = "d3d11,cuda,cpu", [string]$Res = "1920x1080@30", [int]$Fps = 30, [string]$Metrics = "metrics.csv", [string]$Y4m = "")
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$edge = "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
$prof0 = Join-Path $env:TEMP "edge-spike"
foreach ($d in ($Decoders -split ",")) {
  Write-Host "=== $d ==="
  $log = Join-Path $here "run-$d.log"
  $srv = Start-Process -FilePath (Join-Path $here "run.bat") -ArgumentList "--decoder $d --label $d --secs $Secs --metrics $Metrics" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 4
  $prof = "$prof0-$d"; if (Test-Path $prof) { Remove-Item -Recurse -Force $prof -ErrorAction SilentlyContinue }
  Start-Process $edge -ArgumentList "--user-data-dir=`"$prof`"","--no-first-run","--use-fake-device-for-media-stream=fps=$Fps"$(if ($Y4m) { ",--use-file-for-fake-video-capture=`"$Y4m`"" }),"--use-fake-ui-for-media-stream","http://localhost:8080/?auto=1&res=$Res"
  $srv.WaitForExit()
  Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -match "edge-spike" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 2
}
