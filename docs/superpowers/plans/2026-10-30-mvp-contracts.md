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
