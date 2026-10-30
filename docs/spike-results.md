# Round 0 spike results

Date: 2026-10-29. Machine: Windows 11, RTX 5060 Ti, 12 logical CPUs, FFmpeg 7.1 LGPL shared (BtbN), str0m ICE-lite, softcam (tshino).
Sender for checkpoints 1-3: **headless** Edge 156 (`--headless=new`, throwaway profile, fake camera fed by a generated 1080p30 Y4M, hardware H.264 encoder: NVIDIA MFT). No visible browser was ever opened by this round of work. No iPhone involved, so Safari-specific behaviour is NOT yet proven (see checkpoints 5-7 and the caveat under 1).

## Verdicts

| # | Checkpoint | Verdict |
|---|---|---|
| 1 | H.264 -> str0m ICE-lite connects, complete AUs, frames decode | **PASS** (Edge; Safari unverified) |
| 2 | PLI -> keyframe < 1 s | **PASS** |
| 3 | d3d11 / cuda / cpu RAM + CPU at 1080p30 | **PASS (measured)**, one target missed (d3d11 RAM) |
| 4 | softcam visible in 64-bit + 32-bit DirectShow host | **BLOCKED-ON-USER** (needs admin regsvr32); sender side PASS |
| 5-7 | multipath/ICE restart, iPhone caps, CA trust | **BLOCKED-ON-USER** (instructions below) |

## 1. Connect + complete AUs: PASS
Evidence (`run-*.log`, not committed): ICE Checking -> Completed in 71 ms, DTLS Connected at +64..81 ms, `media added mid=0 kind=Video dir=RecvOnly`, datachannel `ctl` opened and the sender's `getCapabilities` JSON arrived. First `MediaData` (H264, whole IDR access unit, 15 KB) at 360-518 ms; FFmpeg decoded it ("FIRST FRAME 1920x1080") in every decoder mode. Over ~120 s per run: 30.0 fps, 0 NACKs, 0 loss on loopback-LAN (host-host over the PC's own 192.168.x NIC). Candidate pair nominated and logged (Udp host-host).
Caveat: Edge is not Safari. Safari's SDP (profile-level-id, packetization-mode, rtcp-fb, `a=extmap`) must be confirmed in checkpoint 6.

## 2. PLI -> keyframe: PASS
- Initial PLI (sent on MediaAdded) -> first IDR: 291 / 299 / 346 / 437 ms across runs (this includes the sender's encoder start-up and PLI round trip).
- Mid-stream manual PLI (`/debug/pli`, x3): keyframe in **19, 22, 7 ms**.
- Loss test (`/debug/loss?permille=30`, 3% simulated RTP drop): `GAP: non-contiguous AU` detected each time, PLI issued, sender keyframe counter rose 4 -> 6+ while loss persisted; stream recovered after loss removed.
- Finding: with loss, `nacks=0` - str0m did not issue NACK in this configuration (recovery relies on PLI/keyframes only). Round 1 should enable NACK/RTX in the answer (or verify what Safari negotiates), otherwise 1-3% Wi-Fi loss means a keyframe per loss event.

## 3. Decode RAM/CPU: measured
Setup: sender 1920x1080 @ 30 fps, ~6 Mbps (91.6 MB over 123 s, `limit=none`), H.264 from NVIDIA hardware encoder. Received: **1920x1080 @ 29.8-30.0 fps** in all three runs. 130 s per decoder, rows from t>=15 s (115 samples). Sampled every 1 s.
Metric scope: the `spike-rx` process only (private working set, via `K32GetProcessMemoryInfo`). It has no child processes, so process tree == process. No WebView2 here, so the "window open" row of §11 is not measured. CPU% = share of all 12 logical CPUs. Softcam sink active (frames written to shared memory) in all runs.

| Decoder | Private WS avg (max) | CPU avg (max) | Decode ms/frame | §11 target |
|---|---|---|---|---|
| d3d11va (NVIDIA adapter) | **44.7 MB** (45.3) | 1.54% (2.86) | 4.8 | RAM < 40 MB: **missed by ~5 MB**; CPU < 3%: ok |
| cuda/nvdec | **134.1 MB** (134.2) | 1.18% (2.47) | 4.7 | opt-in only, excluded: confirms CUDA context costs ~90 MB |
| cpu (libavcodec, 4 slice threads) | **21.6 MB** (21.6) | 1.11% (2.34) | 3.6 | RAM < 50 MB: ok; CPU < 10%: ok |

Observations:
- CPU decode is the cheapest in RAM and not more costly in CPU at 1080p30 on this 12-thread CPU; d3d11 pays for the D3D11 device/driver DLLs (~23 MB extra) and GPU->system copy (NV12 download) for the BGR conversion.
- The 24 s smoke runs (not kept) and an earlier 720p run (sender throttled by bandwidth estimate; d3d11 45.6 MB, cuda 122.7 MB, cpu 16.9 MB) agree.
- These numbers exclude the Tauri/WebView2 part and the real tray app's other threads; add them in Round 1.
- Sender bandwidth: the default WebRTC estimate drops to 720p with a noisy source; the page now sets `degradationPreference=maintain-resolution` + `maxBitrate`. The receiver must send TWCC/REMB feedback reliably; check in Round 1 what str0m sends (probably the cause of `limit=bandwidth` in the first run).

## 4. softcam: BLOCKED-ON-USER (partially verified)
Verified without admin: softcam x64 `scCreateCamera(1920x1080)` returns a non-null camera and `scSendFrame` runs for 130 s at 30 fps in all three decoder runs; resolution change (to 720p) re-creates the camera without crash. Both `softcam.dll` (x64 and Win32) were built (`spike/vendor/softcam-src/src/softcam/dist/bin/{x64,Win32}`).
Not verified: appearing in DirectShow enumeration for 64-bit and 32-bit hosts. No admin elevation was attempted or bypassed. `ffmpeg -list_devices true -f dshow -i dummy` (64-bit) currently does not list "DirectShow Softcam" (other virtual cams are present: Iriun, ByteCast), i.e. not registered yet.

**User, in an elevated (Run as administrator) Command Prompt:**
```
cd /d <repo>\spike\vendor\softcam-src\src\softcam\dist\bin
C:\Windows\System32\regsvr32.exe x64\softcam.dll
C:\Windows\SysWOW64\regsvr32.exe Win32\softcam.dll
```
Then (non-admin) run `spike\run.bat --decoder cpu` plus the Edge command from `spike/README.md`, or just `spike\run.bat --decoder cpu` with any sender, and verify:
- 64-bit: `ffmpeg -list_devices true -f dshow -i dummy` (the FFmpeg in `spike\vendor\ffmpeg\...\bin`) must list "DirectShow Softcam"; or open Windows Camera / OBS (Video Capture Device).
- 32-bit: open a 32-bit DirectShow app (e.g. 32-bit OBS Studio or old Skype/32-bit GraphEdit) and confirm the same device shows a picture.
Undo: same commands with `/u`.
Report back: which hosts showed the device and whether the image moved.

## 5-7. Needs the iPhone (not attempted). Exact steps
Prereq: PC and iPhone on the same Wi-Fi (private IP 192.168.15.145 at time of writing; the spike prints the correct URLs). Windows Firewall: allow inbound for `spike-rx.exe` on Private profile when prompted (or ports 8080/8443 TCP + the UDP port printed as `UDP socket`).
1. On the PC: `spike\run.bat --decoder d3d11` (leave running; it prints two QR codes).
2. **Checkpoint 7** (CA trust): on the iPhone camera-scan QR #1 (`http://<ip>:8080/ca.mobileconfig`), tap Allow, then Settings > General > VPN & Device Management > tap the profile > Install. Then Settings > General > About > Certificate Trust Settings > enable full trust for "PhoneCam" root. Then scan QR #2 (`https://<ip>:8443/`) in Safari. PASS = page loads with no certificate warning and the padlock. Note whether iOS rejected the profile because of Name Constraints.
3. **Checkpoint 6**: on that page choose lens + 1080p@30, tap Start, allow the camera. Confirm the PC log shows `first MediaData codec=H264`, `FIRST FRAME`, and the picture in softcam (after checkpoint 4 registration) or at least `fps~30`. Screenshot or copy the on-screen capabilities box (zoom min/max, torch, lens list, max width/height/fps). Tap the zoom and torch test buttons and note the reported success/failure. Try each resolution in the select (720p/1080p/4K) and each lens; note the frame rate reached.
4. **Checkpoint 5**: enable USB tethering/Personal Hotspot (USB cable to PC) while Wi-Fi is on; reload the page so both networks exist. Watch the PC log lines `[ice] ... NOMINATED pair` (expect Wi-Fi). Turn Wi-Fi OFF on the iPhone; the page should ICE-restart (button "ICE restart" exists if not automatic). Expected log: `re-offer (ICE restart)` then a new NOMINATED pair on the USB subnet and video resuming. Report the interruption time (seconds of frozen video) and paste the `[ice]` lines.
5. Paste `spike\run-*.log` (or the terminal text) back to the agent.

## str0m go / no-go: GO (with two Round 1 tasks)
Works as an ICE-lite H.264 receiver on first attempt: connect < 100 ms, correct AU depacketization (`MediaData` with `is_keyframe`, `contiguous`), PLI API works, per-session stats events usable, runs 130 s x 3 with no errors. Open items for Round 1: (a) NACK/RTX not observed (`nacks=0` under 3% loss) - enable and verify; (b) TWCC/bandwidth feedback so the sender does not throttle to 720p; (c) confirm Safari SDP compatibility (checkpoint 6); (d) multipath / restart (checkpoint 5).

## Proposed §11 revisions
- Keep the table but state measurement honestly: the Rust receiver alone is 22 MB (cpu) / 45 MB (d3d11) / 134 MB (cuda) private WS at 1080p30; Tauri + tray adds unknown overhead (WebView2 destroyed when hidden).
- Auto order (§10): D3D11VA costs ~23 MB more than software decode on this machine. Either raise the tray-only D3D11VA RAM target to **< 60 MB** (and keep CPU < 3%), or make Auto prefer CPU for <= 1080p30 and use D3D11VA for higher loads (4K / 60 fps). Recommended: Auto = D3D11VA, target < 60 MB, because it protects CPU headroom for games while the camera is on; measure the real tray-app overhead in Round 1 before fixing numbers.
- CPU decode target (< 10%) is very conservative: measured ~1.1% of 12 threads. Tighten to < 5%.
- CUDA row: state ~135 MB, opt-in only, as already described.
- Add a note that on loss the receiver depends on PLI unless NACK/RTX is enabled.
