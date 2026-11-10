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

## D contract detail (round 2)

New wire message (core, add a serde round-trip test). Phone → PC on DC open and after every lens switch:
`Ctl::Caps { lenses: Vec<LensInfo{id,label}>, current: String, zoom_min: f32, zoom_max: f32, torch: bool }`
JSON: `{"type":"caps","lenses":[{"id":"..","label":".."}],"current":"..","zoom_min":1,"zoom_max":10,"torch":true}`

IPC (Tauri 2). Rust owns everything; UI is a thin view.
- `get_state() -> AppState`
- `set_settings(patch: Partial<Settings>) -> AppState` — persists to `%APPDATA%\PhoneCam\settings.json`; lens/profile/zoom/torch → `Ctl::Set`; mirror/decoder → media.
- `new_pairing() -> AppState` — new room id + 32-byte secret, restarts the receiver.
- `get_preview() -> Option<String>` — base64 JPEG (max_w 640). The UI polls at ≤15 fps only while the document is visible and conn == "live".
- Event `"state"` (payload AppState) on every change; throttle to ≤4/s.
```ts
type AppState = {
  phoneUrl: string; qrSvg: string;                       // qrcode crate, SVG string
  conn: "idle"|"connecting"|"live"|"paused"|"reconnecting";
  pauseReason: "call"|"background"|"locked"|"unknown"|null;
  path: "lan"|"usb"|null; rttMs: number|null; battery: number|null; charging: boolean|null;
  decoderName: string|null; fps: number; width: number; height: number;
  lenses: {id:string,label:string}[]; zoomMin: number; zoomMax: number; torchAvailable: boolean;
  settings: { lens: string|null; profile: "economy"|"balanced"|"quality"; zoom: number;
              torch: boolean; mirror: boolean; decoder: "auto"|"cuda"|"cpu" };
}
```
Output mirror: media-win gets `handle.set_mirror(bool)` (horizontal flip after letterbox, unit test).
Decoder change restarts Media without recreating softcam? softcam is created once per process — keep the vcam object, restart only the decoder thread.

Split:
- D1 (Sonnet, `apps/desktop/src-tauri` + core Caps + media mirror): Tauri app, tray (Open/Quit, left-click opens), close/minimize → hide window + `skip_taskbar` + destroy webview (recreate on Open), single-instance plugin, settings file, the IPC above. Mock-free: wire real transport + media. Default relay/page URLs = the live ones.
- D2 (agy, `apps/desktop/src` React + Vite + Tailwind + shadcn + lucide): UI against the IPC above via `@tauri-apps/api/core` `invoke` and `listen`. Dark default, follows `prefers-color-scheme`. Layout ≈ 900×600: left = QR (idle/connecting) or preview `<img>` (live) with an overlay pill (icon + text) for paused/reconnecting; right = cards Connection / Camera / Decoder. Desktop HIG: 13 px body, 28 px min controls, state = icon + text, explicit verbs ("New pairing code"). No custom theme toggle.
- D3 (agy, `apps/web-client`): send `caps` on DC open and after each lens switch; apply incoming `ctl set` {profile,zoom,torch,lens}; send `paused`/`resumed` on visibilitychange.

## Finalization round (2026-11-10) — found by e2e (`scripts/dev/e2e.sh`)
E2E PASS: headless fake phone → Vercel page → relay → installed app → vcam frame 1920x1080; tray mode works; CPU 0.8% (1080p30, tray).
Env for e2e.sh: `APP_EXE` (default installed exe), `APPDATA` (pairing.json is read from it), `CAM` (dshow name), `RES`, `WAIT`, `SHOT=<file>`, `CPU=1`, `OUT=<dir>`.
- F1 UI (agy): (a) show the QR whenever conn is not live/paused — in `reconnecting` show QR + pill "Reconnecting… scan again if the page was closed"; (b) right panel must fit with no scrollbar at the default window size: window 1000x720 (min 900x640) in tauri.conf.json + tighter card spacing; Decoder card fully visible.
- F2 vcam name (agy): build softcam from `spike/vendor/softcam-src` into NEW dir `spike/vendor/softcam-phonecam/` (do not touch existing vendor output): friendly name "PhoneCam", NEW CLSID, renamed shared-memory/mutex names (no clash with other softcam apps), x64 + Win32 DLL + sender lib. Repo gets `scripts/build-softcam.ps1` + patch file; build.rs defaults of media-win and desktop point to the new dir.
- F3 engine (Sonnet, diagnosis): (a) memory: 59 MB idle → 205 MB WS / 354 MB private after the first session, never released (spike D3D11 was 44.7 MB). Find the cause; target tray 1080p30 ≤ 110 MB WS and back to ≈ idle within 10 s after the phone leaves. (b) UI shows "No path" and no latency while live: make PathChanged/rtt reach AppState (loopback/same-host counts as "lan").

## Round G (2026-11-10) — first real-iPhone session (after firewall fix it connects)
User report: (1) video is sideways, no way to rotate; (2) "Quality 4K" selected but Resolution shows 1920×1080; (3) still disconnects / unstable; (4) Frame rate shows 60 fps, user says it is not really 1080p60.
Contract changes (all additive, serde default so old peers still parse):
- `Settings.rotation: 0|90|180|270` (default 0), `set_settings({rotation})`. Applied in media-win before letterbox, ON TOP of automatic rotation.
- Automatic rotation: honour the RTP video-orientation (CVO, `urn:3gpp:video-orientation`) header extension if Safari negotiates it; otherwise use `Ctl::Caps.rotation` hint if present.
- `Ctl::Caps` gains optional `width`, `height`, `fps` (what the phone's track is really capturing: `track.getSettings()`), and the page re-sends caps after every applied `set`.
- `AppState` gains `srcWidth`, `srcHeight`, `srcFps` (decoded stream, measured on the PC) and `phoneWidth`, `phoneHeight`, `phoneFps` (from caps). `width/height` stay = virtual camera output size.
- Logging: `%APPDATA%\PhoneCam\phonecam.log` (tracing, INFO, truncate at 2 MB): every conn state change with its cause (heartbeat lost / ICE state / phone left / connect timeout / relay closed), decoder open/close, resolution changes, NACK/PLI/gap counters once per 10 s while live. No secrets (never log `k`, proofs or the full phone URL).
Tasks:
- G1 engine (Sonnet): logging; rotation (auto + manual) in media-win with unit tests; src stats; find why 4K shows 1080 (does `Ctl::Set{profile:quality}` reach the page? does the decoder/vcam path downscale — expected, vcam is fixed 1920×1080 — or is the stream itself 1080?); fps counter correctness; review reconnect/heartbeat logic for false disconnects on a real Wi-Fi phone (pings now really sent since F3: HB_INTERVAL 2 s, miss limit 3 — is 6 s too tight when iOS throttles timers/DC in background? does `alive()` cover it?).
- G2 UI (agy): "Rotate" button (icon + text, cycles 0→90→180→270) in the Camera card; Decoder card rows: "Phone sends" (phoneW×phoneH @ phoneFps), "Received" (srcW×srcH @ srcFps), "Camera output" (width×height). Remove the single "Resolution"/"Frame rate" rows.
- G3 page (agy): caps with width/height/fps from `track.getSettings()`, re-sent after each applied set and after orientation change; make `set profile` apply exact constraints and log the real result; res list must only offer modes the page can actually deliver (if 60 fps is offered but Safari delivers 30, drop the 60 entries).

## Round H (2026-11-10) — native iPhone sender (phase 2 brought forward)

Why: Safari caps WebRTC at 1080p, reopens the camera on every change, and stalls (see HANDOFF).
Iriun's model works: native app, plain TCP on the LAN. The Safari page + relay stay as fallback.

### Wire protocol "native v1" (both sides implement exactly this)
- The PHONE listens (TCP, preferred port 47810, any free port otherwise) and advertises Bonjour
  `_phonecam._tcp` with TXT `v=1`, `id=<first 16 hex chars of SHA-256("phonecam-id-v1" || k)>`,
  `name=<device name>`. The PC browses mDNS, connects to the service whose `id` matches its pairing
  secret `k` (32 raw bytes, the same hex `k` already in the pairing QR). No inbound rule on the PC.
- Frame: `u32 BE len` (covers type + payload, max 8 MiB) | `u8 type` | payload.
  - `0x01` control: UTF-8 JSON.
  - `0x03` video AU: `u64 BE pts_us` | `u8 flags` (bit0 = keyframe) | `u8 rot` (0..3, x90 deg
    clockwise needed to display upright) | H.264 Annex-B (start codes; SPS+PPS inline on keyframes).
- Handshake (control frames; nonces are 16 random bytes as 32 hex chars; HMAC-SHA256 keyed by `k`,
  output hex; message is the ASCII concatenation shown):
  1. PC: `{"t":"hello","v":1,"nonce":"<pc>"}`
  2. Phone: `{"t":"auth","nonce":"<ph>","proof":HMAC("phonecam-native-v1|phone|"+pc+ph)}`
  3. PC verifies, sends `{"t":"auth_ok","proof":HMAC("phonecam-native-v1|desktop|"+ph+pc)}`
  4. Phone verifies, then sends `caps` and starts video with a keyframe. Any failure: close.
- After the handshake control JSON is the existing `phonecam_core::protocol::Ctl` (same serde
  shapes: ping/pong/set/paused/resumed/caps) plus PC -> phone `{"type":"keyframe"}`.
- v1 authenticates but does NOT encrypt the stream (LAN only). Encryption is a later round.

### H1 — desktop native receiver (Sonnet; crates/phonecam-transport, core, src-tauri wiring)
- New module in transport: mDNS browse (`mdns-sd`), TCP client (TCP_NODELAY, 4 MiB recv buffer),
  handshake, frame parser, reconnect with the core backoff. Emits the SAME events the WebRTC engine
  emits (state, AUs to media, caps, battery, path = Lan) so media-win and the UI are untouched.
- Runs alongside the relay/WebRTC engine; whichever source authenticates first owns the session,
  native preferred. Heartbeat via ping/pong; `Set` goes over the active source.
- Profiles gain real `3840x2160@30` and `1920x1080@60` for the native source.
- Tests: frame codec round-trip, handshake proofs (fixed vectors written into this doc's test so the
  Swift side can copy them), rejects bad proof, a fake-phone TCP server integration test that
  streams a recorded H.264 file and reaches Live. `cargo test --workspace`, clippy `-D warnings`.
- Also log `[native]` lines to phonecam.log. Never log `k`.

### H2 — iOS app (Sonnet; apps/ios, .github/workflows/ios.yml)
- SwiftUI app "PhoneCam", iOS 16+, XcodeGen `project.yml` (no hand-written pbxproj), no third-party
  deps. Info.plist: camera usage, local network usage, `NSBonjourServices = [_phonecam._tcp]`.
- Pairing: scan the desktop QR (the existing https URL; read `k` from the fragment), keep `k` in
  the Keychain. Afterwards opening the app is enough: it listens and the PC connects.
- Capture AVFoundation -> VideoToolbox H.264 (real time, no frame reordering, High profile,
  keyframe every 2 s and on request). Bitrates: 4K30 25 Mbps, 1080p60 16, 1080p30 10, 720p30 5.
  Lens list from the discovery session, zoom, torch, front/back; applying a `set` reconfigures the
  running session without dropping the TCP connection.
- Backpressure: if more than ~1 s of video is queued, drop until the queue drains, then send a
  keyframe. Screen stays awake; on background/interruption send `paused`, on return `resumed`.
- Screen: dark, minimal — live preview, status line, "Pair" (scanner), nothing else.
- CI: macOS runner, `xcodegen` + `xcodebuild` with signing disabled, zip `Payload/` into an
  unsigned `PhoneCam.ipa` uploaded as an artifact (user signs with Sideloadly). The agent iterates
  on a pushed branch until the workflow is green; that is the only compile check available.

### H3 — desktop UI restyle (Haiku; apps/desktop/src only)
- Match the user's reference: near-black background, 1px hairline borders, square corners, small
  uppercase mono labels in brackets, large light heading, preview left / settings card right,
  footer hints in muted text. No behaviour or IPC change. `npm run build` must pass.

### Round I (next, not now)
Manual camera controls (ISO, shutter, exposure bias, white balance, focus lock), virtual camera
output above 1080p, USB via usbmuxd, stream encryption.
