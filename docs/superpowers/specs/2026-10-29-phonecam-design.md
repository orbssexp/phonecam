# PhoneCam — Design Spec

Date: 2026-10-29
Status: Draft (awaiting user review)

## 1. Goal

Use an iPhone camera as a Windows webcam, replacing Iriun Webcam. Beat Iriun on:

1. **Connection stability** — no drops in long sessions; Wi-Fi primary, USB fallback, automatic recovery.
2. **Camera control** — lens, resolution/FPS, zoom, torch, mirror, rotation.
3. **Interruption resilience** — phone calls / app backgrounding must not kill the virtual webcam in other apps.
4. **Low resource usage** — "light" means **RAM + CPU/GPU at runtime**, not install size.
5. **Battery on the phone** — black-screen mode, energy profiles.

Clean, dark-mode UI built with shadcn/ui and lucide icons.

## 2. Phases

- **Phase 1 (this spec):** Windows desktop app (receiver) + iPhone Safari web page (sender).
- **Phase 2 (future, out of scope here):** native iOS app in Swift/SwiftUI following Apple HIG, published on the App Store. It replaces the Safari page and reuses the shared Rust core via UniFFI. Adds: manual focus/exposure/white balance, automatic screen brightness control.

All Phase 1 code that isn't platform-specific must be portable to Phase 2 (and to a future macOS receiver).

## 3. Hard platform constraints (accepted, not solvable)

- iOS revokes camera access when the app/page goes to background, the phone locks, or a call comes in. No app can bypass this. We mitigate (§7) rather than prevent.
- Safari cannot control screen brightness. Phase 1 uses a black overlay instead; Phase 2 sets brightness natively.
- Safari `getUserMedia` requires a secure context → the desktop app serves HTTPS with a local certificate the user trusts once on the iPhone.
- Safari exposes lens choice (via `enumerateDevices`), resolution/FPS, zoom and torch (via `applyConstraints`). Manual focus/exposure is Phase 2 only.
- USB fallback in Phase 1 uses iPhone **Personal Hotspot over USB** (creates a 172.20.10.x network). Requires hotspot enabled; some carriers block it.

## 4. Stack

| Layer | Choice | Why |
|---|---|---|
| Core logic | Rust | Portable to iOS (UniFFI) and macOS; no GC; low RAM/CPU |
| WebRTC | `str0m` (sans-IO Rust WebRTC) | Lightweight, runs on Windows and iOS; we own the I/O loop |
| Decode | FFmpeg (`ffmpeg-next`) | NVDEC, D3D11VA and software decoding behind one API |
| Color convert | libyuv (or GPU path) | SIMD NV12→BGR, ~1% CPU |
| Virtual webcam | softcam (DirectShow) | Proven approach (same family as OBS VirtualCam); visible in Zoom/Teams/Discord/OBS/Chrome |
| Desktop shell | Tauri 2 | Rust backend + WebView2 UI |
| UI | React + TypeScript + Tailwind + shadcn/ui + lucide-react | Requested design system; dark mode only |
| Phone page | TypeScript, vanilla (no framework) | Smallest, fastest page on the phone |
| Protocol schema | Rust types as source of truth → TS via `ts-rs`, Swift via UniFFI | One schema, no drift |

Rejected: Electron (no decoder control, ~200–350 MB RAM), .NET/WinUI (no shadcn, not portable to iOS), Flutter (limited decoder control), Qt/C++ (very high effort), Slint native UI (no real shadcn; reconsider only if window-open RAM becomes a problem).

## 5. Repository layout

```
phonecam/
├─ crates/
│  ├─ phonecam-core        # SHARED, pure: protocol, controls, profiles, heartbeat,
│  │                       # reconnection state machine, path priority. No I/O, no threads.
│  ├─ phonecam-transport   # SHARED: str0m WebRTC + signaling client/server; sender & receiver roles
│  ├─ phonecam-ffi         # SHARED: UniFFI bindings (Phase 2 consumer; stub in Phase 1)
│  ├─ decode-win           # Windows: FFmpeg decoder with Auto/NVDEC/D3D11/CPU
│  └─ vcam-win             # Windows: softcam wrapper + placeholder frames
├─ apps/
│  ├─ desktop/             # Tauri app (src-tauri + React UI)
│  ├─ web-client/          # Safari sender page
│  └─ ios/                 # Phase 2 (empty placeholder README only)
└─ protocol/               # generated TS types + protocol docs
```

**Portability rule:** `phonecam-core` has zero platform dependencies, no networking, no threads. It is a state machine: `fn handle(event) -> Vec<Action>`. Runtimes (tokio on Windows, GCD on iOS) drive it. `phonecam-transport` may depend on std networking but nothing Windows-specific.

## 6. Architecture & data flow

```
iPhone (Safari)                          Windows (Tauri)
┌──────────────┐  WebRTC H.264        ┌────────────────────────────────┐
│ camera page  │ ───────────────────► │ transport (str0m) → decode-win │
│ pinch zoom   │ ◄── DataChannel ──── │   (Auto/NVDEC/D3D11/CPU)       │
│ black screen │   controls + ping    │   → vcam-win "PhoneCam" ───────┼─► Zoom/OBS/Teams
└──────────────┘                      │   → 15fps preview → React UI   │
       ▲  HTTPS/WSS + QR (pairing)    │ HTTPS/WSS server, tray icon    │
       └──────────────────────────────┴────────────────────────────────┘
```

1. Desktop app starts → generates/loads local CA + cert → starts HTTPS/WSS server on all interfaces → shows QR with URL.
2. iPhone scans QR → Safari opens page → user taps Start → `getUserMedia` → page opens WSS → SDP/ICE exchange → WebRTC peer established.
3. Video: phone hardware H.264 → RTP → str0m → decoder → NV12 → BGR → softcam shared memory.
4. DataChannel `ctl` (reliable): controls, state, battery level, heartbeat.
5. Preview: decoder output downscaled to ≤480p at ≤15 fps, sent to UI only while the window exists and is visible.

## 7. Connection & resilience

### 7.1 Path priority
- ICE gathers candidates on every interface: Wi-Fi and USB-hotspot (172.20.10.x).
- Core assigns priority: **Wi-Fi > USB** (configurable). Nominated pair follows priority; on failure of the active pair, switch to the next valid pair without renegotiating the session.
- UI shows the active path badge (Wi-Fi / USB) and latency.

### 7.2 Heartbeat & recovery
- Ping every 2 s over DataChannel; pong carries phone battery and camera state.
- 3 missed pongs (6 s) → **ICE restart**.
- ICE restart fails within 10 s → full re-signal over WSS.
- WSS unreachable → exponential backoff 1 s → 2 s → 4 s … capped at 30 s, forever, until the user stops.
- Phone page reconnects WSS on its own with the same backoff, reusing a session token so the desktop treats it as the same device.

### 7.3 Virtual webcam never disappears
- softcam device "PhoneCam" stays registered and producing frames for as long as the app runs.
- States → frame shown:
  - `Live` → camera frames
  - `Reconnecting` → last frame blurred + "Reconnecting…" overlay
  - `Paused` (camera revoked: call/background/lock) → last frame blurred + "Paused on phone"
  - `Idle` (never connected) → branded placeholder "Scan QR to connect"
- Placeholder frames are rendered at the negotiated resolution, at 5 fps (cheap).

### 7.4 Phone-side interruptions
- Detect `track.onended` / `track.onmute` / `visibilitychange`.
- Send `state: paused` if the DataChannel is still alive.
- On return to foreground: re-acquire camera automatically if Safari allows; otherwise show one large "Resume" button. Then `replaceTrack` on the existing peer (no renegotiation) or re-signal if the peer died.

### 7.5 Long-session drops (Windows)
- On start, check the Wi-Fi adapter's power-saving setting; if enabled, show a dismissible tip in the UI explaining how to disable it.
- Phone page holds a Screen Wake Lock while streaming; re-requests it on `visibilitychange`.

## 8. Camera controls (Phase 1)

Sent desktop → phone over `ctl`; phone applies and echoes actual values back (single source of truth = phone).

| Control | Phone implementation | Desktop UI | Phone UI |
|---|---|---|---|
| Lens (0.5× / 1× / 2–3×) | switch `deviceId` + `replaceTrack` | segmented buttons | buttons above viewfinder |
| Resolution / FPS | `applyConstraints`, fallback to re-acquire | select | — |
| Zoom | `applyConstraints({zoom})` | slider + mouse wheel / touchpad pinch over preview | **pinch on viewfinder** (page zoom disabled) |
| Torch | `applyConstraints({torch})` | toggle | toggle |
| Mirror | applied on desktop before softcam | toggle | — |
| Rotation 0/90/180/270 | applied on desktop before softcam | button | — |

Unsupported capabilities (per `getCapabilities()`) are hidden, not disabled.

## 9. Energy profiles

| Profile | Video | Target bitrate |
|---|---|---|
| Economy | 720p30 | ~2.5 Mbps |
| Balanced (default) | 1080p30 | ~5 Mbps |
| Quality | 1080p60 (4K30 if supported) | ~10–20 Mbps |

Phone sends exactly the selected resolution — no scaling on the PC.

**Black-screen mode (phone):** full-screen pure-black overlay (OLED ≈ zero power), video keeps streaming. Activated by button or automatically after 10 s without touch. Tap to show controls for 5 s. Page suggests lowering brightness via Control Center (Safari can't set it).

## 10. Decoder selection

- Setting: **Auto (default)** | NVIDIA (NVDEC) | GPU (D3D11VA) | CPU.
- Auto order: NVDEC if an NVIDIA GPU is present → D3D11VA → CPU.
- Any hardware init/decode failure falls back to the next option automatically and the UI shows a notice.
- UI shows active decoder and app CPU/GPU usage.
- Note: encoding happens on the iPhone (always hardware). The PC only decodes.

## 11. Performance budgets (enforced by test)

| Scenario | RAM (whole app) | CPU |
|---|---|---|
| Tray only, streaming 1080p30, NVDEC | < 40 MB | < 3% |
| Tray only, streaming 1080p30, CPU decode | < 50 MB | < 10% |
| Window open, streaming, preview on | < 150 MB | < 6% |

Techniques:
- Full-resolution frames never cross into the UI.
- Reused frame buffer pool; no per-frame allocation.
- Event-driven loop; no polling timers except heartbeat.
- Preview off when the window is hidden.
- **Window lifecycle:** closing or minimizing the window **destroys the WebView2** and **removes the app from the taskbar**; only the tray icon remains. Clicking the tray icon recreates the window (< 1 s). Tray menu: Show, current status, Quit.

## 12. UI

Dark mode only. shadcn/ui components, lucide icons, neutral palette, one accent color.

**Desktop (single window):**
- Center: preview (aspect-fit), status overlay when not Live.
- Right panel (cards):
  - Connection — path badge (Wi-Fi/USB), latency, bitrate, phone battery, Reconnect button.
  - Camera — lens segmented control, resolution/FPS select, zoom slider, torch/mirror/rotate toggles.
  - Performance — energy profile, decoder select + active decoder, CPU/GPU usage.
- Empty state: QR code + first-time setup steps (certificate trust).

**Phone page:**
- Full-screen viewfinder with pinch zoom, lens buttons, status pill.
- Big "Start" on first load; big "Resume" after interruption.
- Black-screen button.

## 13. First-time setup (HTTPS trust)

1. App generates a local root CA (stored in app data) and a leaf cert for its LAN/USB IPs (regenerated when IPs change).
2. Empty state shows QR #1 → downloads the CA as an iOS configuration profile.
3. User installs it, then enables it in Settings → General → About → Certificate Trust Settings (instructions shown with screenshots).
4. QR #2 → opens the camera page. After setup, only QR #2 is shown.

## 14. Protocol (summary)

- Signaling (WSS, JSON): `hello{token,role,version}`, `offer`, `answer`, `ice`, `bye`.
- Control (DataChannel `ctl`, JSON): `ping{t}` / `pong{t,battery,charging,camState}`, `set{lens|res|fps|zoom|torch|profile}`, `state{...actual values, capabilities}`, `paused{reason}`, `resumed`.
- Versioned (`version` field); Rust types are the source of truth; TS types generated.

## 15. Error handling

- Every failure surfaces as a state in core, never a crash: decoder failure → fallback; softcam missing → UI error with "Repair" (re-register, needs admin once); cert expired → regenerate + show QR #1 again; port in use → pick next free port and update QR.
- Logs: rotating file in app data, 5 MB cap.

## 16. Testing

- **Unit (Rust, `cargo test`):** core reconnection state machine (all transitions), heartbeat timing, path priority, profiles, protocol serde.
- **Unit (TS, Vitest):** phone page control handling, pinch→zoom mapping, interruption handling (mocked tracks).
- **Integration:** headless Chromium sender (fake camera) ↔ desktop receiver over loopback: connect, drop network, verify ICE restart, verify placeholder frames, verify recovery.
- **Performance test:** scripted 10-minute stream at 1080p30, measures RAM/CPU against §11 budgets.
- **Manual checklist (real iPhone):** Wi-Fi drop → USB takeover, incoming call → paused → resume, lock/unlock, 2-hour session, each decoder mode.

## 17. Delivery plan outline (token-economy)

- **Round 0 (spike, Opus-reviewed):** Safari → str0m → NVDEC/CPU → softcam over Wi-Fi. Go/no-go for str0m. Fallback if no-go: Electron with the same React UI.
- **Round 1 (≤3 agents, separate worktrees):** (1) `decode-win` + `vcam-win`; (2) `phonecam-core` + `phonecam-transport` + HTTPS/WSS + cert; (3) `web-client`.
- **Round 2 (≤3 agents):** (1) desktop React/shadcn UI + tray lifecycle; (2) resilience integration (§7); (3) tests + perf harness.
- `npm install` / `cargo build` serialized; never in parallel.

## 18. Out of scope (Phase 1)

Audio, multiple phones at once, Android, macOS receiver, manual focus/exposure/WB, auto brightness, recording, auto-updates, App Store app.
