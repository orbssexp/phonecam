# spike-rx (Round 0, throwaway)

str0m ICE-lite H.264 receiver + FFmpeg decode (d3d11 / cuda / cpu) + softcam sink + HTTPS/WSS test page.

## Build (Windows, Git Bash)
```
export PATH="$HOME/.cargo/bin:$PATH" LIBCLANG_PATH="C:\Program Files\LLVM\bin"
export FFMPEG_DIR="$(pwd -W)/vendor/ffmpeg/ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1"
cd spike && cargo build --release
```
Vendor (git-ignored): BtbN `ffmpeg-n7.1-latest-win64-lgpl-shared-7.1` under `vendor/ffmpeg/`, tshino/softcam built into `vendor/softcam-src/src/softcam/dist/`.

## Run
```
run.bat --decoder d3d11|cuda|cpu|auto [--label X] [--secs N] [--metrics metrics.csv] [--no-vcam] [--only-ip A.B.C.D]
```
`run.bat` puts the FFmpeg and softcam DLLs on PATH. It prints two QR codes (CA profile over HTTP :8080, camera page over HTTPS :8443).
Debug endpoints: `/debug/pli`, `/debug/loss?permille=N` (simulated RTP loss).
Metrics (`metrics.csv`): per second private working set of the spike-rx process, CPU% of all logical CPUs, fps, avg decode ms, resolution.

## Checkpoint 3 on the PC alone (headless sender, no visible browser)
```
powershell -NoProfile -File measure.ps1 -Secs 130 -Decoders d3d11,cuda,cpu -Y4m <noise1080.y4m>
```
Generate the test video: `run.bat --gen-y4m <path>.y4m`. `measure.ps1` launches Edge only as `--headless=new` with a fresh `--user-data-dir` under the Claude scratchpad, and kills only those processes.

## Virtual camera (needs admin once)
```
regsvr32 "%CD%\vendor\softcam-src\src\softcam\dist\bin\x64\softcam.dll"
regsvr32 "%CD%\vendor\softcam-src\src\softcam\dist\bin\Win32\softcam.dll"
```
Undo with `regsvr32 /u`.

## Phone (checkpoints 5-7)
See `docs/spike-results.md`.
