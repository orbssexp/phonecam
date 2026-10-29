# Round 0 Spike Plan

Spec: `docs/superpowers/specs/2026-10-29-phonecam-design.md` (§17). Throwaway code under `spike/`. Results go to `docs/spike-results.md` (one section per checkpoint: PASS / FAIL / PARTIAL, evidence, numbers).

Machine: Windows 11, RTX 5060 Ti (driver 617.42). The iPhone is available only through the user, so any step that needs the phone must end with exact instructions for the user.

## Layout

```
spike/
├─ Cargo.toml          # one binary crate: spike-rx
├─ src/main.rs         # HTTPS+WSS server, str0m ICE-lite receiver, decoder, softcam sink
├─ web/index.html      # minimal Safari sender (getUserMedia → RTCPeerConnection, H.264 only)
├─ vendor/             # ffmpeg (LGPL shared, dev), softcam build output — git-ignored
└─ README.md           # how to run
```

## Steps

1. **Deps.** Get an FFmpeg 7.x **LGPL shared** Windows build with headers (BtbN `win64-lgpl-shared`) into `spike/vendor/ffmpeg`. Set `FFMPEG_DIR`. Build softcam (github.com/tshino/softcam) for x64 and Win32 into `spike/vendor/softcam`.
2. **Server.** Use axum + tokio-rustls. Generate a self-signed CA + leaf with rcgen. The leaf carries SAN IPs for all private interfaces; the CA has Name Constraints for private ranges. Serve:
   - `http://<ip>:8080/ca.mobileconfig` (setup)
   - `https://<ip>:8443/` (page)
   - `wss://<ip>:8443/ws` (signaling)
   Print both URLs and render them as terminal QR codes (`qrcode` crate).
3. **WebRTC.** Use str0m in ICE-lite mode, H.264 recvonly + DataChannel `ctl`. The phone offers, the desktop answers, the desktop adds host candidates with local-preference Wi-Fi > USB. Log every candidate pair and the nominated pair.
4. **Depacketize** into H.264 access units (str0m sample API). Request a keyframe (PLI) at start and whenever a gap shows up. Log keyframe latency.
5. **Decode** with `ffmpeg-next`. The flag `--decoder auto|d3d11|cuda|cpu` selects hwaccel: d3d11 picks the NVIDIA adapter, cuda uses hw device type CUDA. Convert NV12 → BGR (libyuv or swscale).
6. **Sink.** Send frames to softcam "PhoneCam".
7. **Measure.** Log the sum of private working set + CPU% for the process every 1 s to `spike/metrics.csv`.
8. **Page.** Implement `web/index.html`:
   - Start button, lens select (enumerateDevices), resolution select.
   - Show `track.getCapabilities()` on screen and send it over `ctl`.
   - Buttons to try zoom and torch via `applyConstraints`, reporting success or failure.

## Checkpoints → results file

| # | What | How verified |
|---|---|---|
| 1 | Safari H.264 → str0m ICE-lite connects, complete AUs | log + frames decoded > 0 |
| 2 | PLI → keyframe < 1 s | log timing |
| 3 | d3d11 / cuda / cpu decode RAM+CPU | metrics.csv, 2 min each, 1080p30 |
| 4 | softcam visible in 64-bit + 32-bit DirectShow host | ffmpeg `-list_devices` (64) + 32-bit graphedit/ffplay, or a tiny test host |
| 5 | Multi-path nomination + ICE-restart Wi-Fi→USB | user toggles Wi-Fi with USB hotspot on |
| 6 | iPhone capabilities (zoom/torch/lenses/max res+fps) | page report |
| 7 | Name-constrained CA trusted by iOS, no warning | user installs profile |

Checkpoints 1–4 can be partly verified on the PC alone, with Chrome desktop as the sender for a first smoke test. Checkpoints 5–7 need the user's iPhone.

## Done when
`docs/spike-results.md` has a verdict for each checkpoint and a go/no-go recommendation for str0m, with revised §11 numbers.
