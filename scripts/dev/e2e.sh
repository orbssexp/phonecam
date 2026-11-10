#!/bin/bash
# End-to-end check: installed PhoneCam app + headless Edge (temp profile, fake camera) + frame grab from the virtual camera.
S="${OUT:-$(cd "$(dirname "$0")" && pwd)}"; D="$(cd "$(dirname "$0")" && pwd)"
REPO=/c/Users/gusta/orca/projects/phone-cam
FF="$REPO/spike/vendor/ffmpeg/ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1/bin/ffmpeg.exe"
EDGE="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
CAM="${CAM:-DirectShow Softcam}"; RES="${RES:-1280x720@30}"; WAIT="${WAIT:-14}"
if ! tasklist //FI "IMAGENAME eq phonecam.exe" | grep -qi phonecam; then
  ( "${APP_EXE:-/c/Program Files/PhoneCam/phonecam.exe}" >/dev/null 2>&1 & ); sleep 6
fi
URL=$(node -e "const p=JSON.parse(require('fs').readFileSync(process.env.APPDATA+'/PhoneCam/pairing.json','utf8'));const r=p.room_id||p.roomId,k=p.secret_hex||p.secretHex;process.stdout.write('https://phonecam-eta.vercel.app/?auto=1&res=$RES#room='+r+'&relay=wss://phonecam-relay.phonecam-relay.workers.dev&k='+k)")
PROF="$S/edge-prof-$$"; mkdir -p "$PROF"
"$EDGE" --headless=new "--user-data-dir=$(cygpath -w "$PROF")" --no-first-run --no-default-browser-check --disable-sync --use-fake-device-for-media-stream --use-fake-ui-for-media-stream --autoplay-policy=no-user-gesture-required --remote-debugging-port=0 "$URL" >/dev/null 2>&1 &
sleep "$WAIT"
PID=$(tasklist //FI "IMAGENAME eq phonecam.exe" //FO CSV //NH | head -1 | cut -d, -f2 | tr -d '"')
echo "app pid=$PID"
powershell -NoProfile -Command "Get-NetTCPConnection -OwningProcess $PID -EA SilentlyContinue | % { 'tcp ' + \$_.RemoteAddress + ':' + \$_.RemotePort + ' ' + \$_.State }; 'udp sockets: ' + @(Get-NetUDPEndpoint -OwningProcess $PID -EA SilentlyContinue).Count; \$p=Get-Process -Id $PID; 'mem MB: ' + [int](\$p.WorkingSet64/1MB)"
rm -f "$S/frame.png"
"$FF" -hide_banner -loglevel error -f dshow -i "video=$CAM" -frames:v 1 -y "$(cygpath -w "$S/frame.png")" 2>&1 | tail -3
ls -la "$S/frame.png" 2>&1 | awk '{print "frame bytes:", $5}'
if [ -n "$SHOT" ]; then powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$D/shot.ps1")" -Out "$(cygpath -w "$S/$SHOT")"; fi
if [ -n "$CPU" ]; then powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$D/meas.ps1")" -ProcId $PID; fi
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='msedge.exe'\" | ? { \$_.CommandLine -like '*edge-prof-$$*' } | % { Stop-Process -Id \$_.ProcessId -Force -EA SilentlyContinue }"
sleep 1; rm -rf "$PROF" 2>/dev/null
echo e2e-done
