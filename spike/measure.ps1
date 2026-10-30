# Checkpoint 3 driver: for each decoder, start spike-rx, point a HEADLESS fake-camera Edge sender (throwaway profile) at it, collect metrics.csv.
# NEVER launches a visible browser or the real profile.
# Usage: powershell -File measure.ps1 -Secs 130 -Decoders d3d11,cuda,cpu -Res "1920x1080@30" -Fps 30
param([int]$Secs = 130, [string]$Decoders = "d3d11,cuda,cpu", [string]$Res = "1920x1080@30", [int]$Fps = 30, [string]$Metrics = "metrics.csv", [string]$Y4m = "")
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$edge = "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
$prof0 = Join-Path $env:TEMP "phonecam-edge-spike"
foreach ($d in ($Decoders -split ",")) {
  Write-Host "=== $d ==="
  $log = Join-Path $here "run-$d.log"
  $srv = Start-Process -FilePath (Join-Path $here "run.bat") -ArgumentList "--decoder $d --label $d --secs $Secs --metrics $Metrics" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 4
  $prof = "$prof0-$d-$([guid]::NewGuid().ToString('N').Substring(0,6))"
  $edgeArgs = @("--headless=new", "--user-data-dir=`"$prof`"", "--no-first-run", "--no-default-browser-check", "--disable-sync", "--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream", "--autoplay-policy=no-user-gesture-required")
  if ($Y4m) { $edgeArgs += "--use-file-for-fake-video-capture=`"$Y4m`"" }
  $edgeArgs += "http://localhost:8080/?auto=1&res=$Res"
  Start-Process $edge -ArgumentList $edgeArgs -WindowStyle Hidden
  $srv.WaitForExit()
  Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -like "*scratchpad*edge-spike*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 2
}
