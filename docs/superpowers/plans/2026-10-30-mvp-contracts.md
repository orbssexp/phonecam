# MVP contracts (fast track)

Goal: a Windows app with a window + tray. Scan the QR code → the iPhone video shows up as the "PhoneCam" webcam, recovers by itself from Wi-Fi loss and network switches, and there are no 1 s freezes.
Spec: `docs/superpowers/specs/2026-10-29-phonecam-design.md`. Core already done: `crates/phonecam-core` (use it; don't change its API without a test).
**Reuse the working spike code** in `spike/src/*.rs`, `spike/web/`, `spike/relay/`. Port it, don't rewrite from scratch.
Rules for every agent:
- Never open a visible browser or the user's profile (headless Edge with a temp profile only).
- Don't run two cargo builds in the same target dir.
- Commit small. Co-Authored-By line as instructed.
- No `--no-verify`.

## A. `crates/phonecam-transport` (Sonnet: needs diagnosis)
Port `spike/src/rtc.rs` + the relay client from `spike/src/main.rs`.
- `Receiver::start(cfg) -> (handle, events: mpsc::Receiver<TransportEvent>)`, where cfg = relay url, page url, room id, secret.
- `TransportEvent`: `PhoneJoined | PhoneLeft | MediaConnected | AccessUnit{data, keyframe, rtp_ts} | Ctl(phonecam_core::Ctl) | PathChanged{kind: PathKind, rtt_ms}`.
- `handle.send_ctl(Ctl)`, `handle.restart_ice()`, `handle.request_keyframe()`, `handle.shutdown()`.
- It drives `phonecam_core::{Conn, Heartbeat}` internally and emits `VcamFrame` changes as `TransportEvent::Vcam(VcamFrame)`.
- **NACK/RTX on**: find why the spike showed `nacks=0` and fix it. Then GAP→PLI happens only when a NACK fails.
- **Interface watch**: poll the interfaces every 2 s. A new private interface (e.g. 172.20.10.x) → bind a UDP socket and add the candidate. On `HeartbeatLost`, `restart_ice` goes over the DataChannel if it's open, else the relay.
- Pairing: send `Signal::Challenge{nonce}` on phone join and verify `Hello.proof` with `phonecam_core::pairing`. Reject on failure.
- Tests: `cargo test -p phonecam-transport`. Add a loopback e2e test if the spike's headless-Edge harness can be reused cheaply; otherwise unit-test the event mapping.

## B. `crates/phonecam-media-win` (agy)
Port `spike/src/decode.rs` + softcam + the ffmpeg build.rs bits.
- `Media::start(decoder: DecoderPref, out: (w, h)) -> MediaHandle`.
- `handle.push_au(data, keyframe)` and `handle.set_frame(VcamFrame)`.
- `DecoderPref`: Auto = D3D11VA on the NVIDIA adapter → D3D11VA on any GPU → CPU; plus explicit Cuda, Cpu.
- **Fixed output size**: softcam is created once at `out` and never recreated. Decoded frames are scaled (swscale) into `out`, keeping the aspect ratio with letterbox bars.
- Placeholder frames at 5 fps, a dark gray background with centered white text:
  - Placeholder: "Scan the QR code in PhoneCam"
  - Reconnecting: "Reconnecting…"
  - Paused: "Paused on iPhone"
  - Text is drawn with a tiny built-in bitmap font. No font files.
- `handle.preview_jpeg(max_w) -> Option<Vec<u8>>`: the latest frame, downscaled. Used by the UI only while the window is open.
- `handle.stats() -> {decoder_name, fps, w, h}`.
- Tests: unit-test the scaling and letterbox math. Run a build + `cargo test`.

## C. `apps/web-client` + `apps/relay` (agy)
Move `spike/web` → `apps/web-client` and `spike/relay` → `apps/relay`. Update the root `vercel.json` outputDirectory.
- Answer `challenge` with `hello{proof}`, using the secret `k` from the URL fragment (HMAC-SHA256 via WebCrypto, domain prefix `phonecam-pair-v1`, matching `phonecam_core::pairing`).
- Default lens: "Back Dual Wide Camera" if present.
- **Pinch-to-zoom** on the preview (disable page zoom), plus 0.5× / 1× / 2× buttons.
- Torch button (only if supported) and the mirror-preview toggle.
- Apple HIG (skill `apple-design-en`):
  - 44 pt targets, controls at the bottom, explicit verbs ("Start camera", "Stop").
  - State always shown as icon + text, never color alone.
- Black-screen mode: a full black overlay after 10 s idle; tap to show the controls.
- On `visibilitychange` back to visible: re-acquire the camera if needed, `replaceTrack`, and re-offer on `restart_ice`.
- Keep the stats/log panel behind a "Details" toggle, hidden by default.
- Deploy: none (the orchestrator pushes; Vercel auto-deploys).

## D. `apps/desktop` (after A+B; Sonnet wires, agy builds UI pieces)
Tauri 2 + React + shadcn/ui + lucide, dark by default, following the Windows theme if it's set to light.
- Window: the QR code when idle → live preview (≤15 fps JPEG from `preview_jpeg`) when connected. Right panel:
  - Connection card: Wi-Fi/USB badge with icon + text, RTT.
  - Camera card: lens select, profile select, zoom slider, torch toggle, output mirror toggle.
  - Decoder select.
- Minimize/close → hide from the taskbar and destroy the webview; the tray icon keeps running. Tray menu: Open, Quit.
- Single instance.
- The Rust side owns transport + media and exposes Tauri commands/events. No video frames cross IPC except the preview.
- Build: `npm run tauri build` → produces an installer.

## Order
Round 1, in parallel: A (Sonnet), B (agy), C (agy). Each works in its own git worktree/branch; the orchestrator merges.
Round 2: D.
Then the user test: run the installer, scan, open Zoom.
