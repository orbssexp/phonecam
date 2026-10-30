# PhoneCam — Design Spec

Date: 2026-10-29
Status: Approved; revised after Round 0 (2026-10-30)

## 1. Goal

Use an iPhone camera as a Windows webcam, replacing Iriun Webcam. Beat Iriun on:

1. **Connection stability** — no drops in long sessions; Wi-Fi primary, USB fallback, automatic recovery.
2. **Camera control** — lens, resolution/FPS, zoom, torch, mirror, rotation.
3. **Interruption resilience** — phone calls / app backgrounding must not kill the virtual webcam in other apps. (The video itself does stop — iOS revokes the camera — but the "PhoneCam" device stays alive with a placeholder and the stream resumes without user action on the PC.)
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
- Safari `getUserMedia` requires a secure context. **Resolved in Round 0:** the phone page is hosted on Vercel (a publicly trusted certificate) and signaling goes through a Cloudflare relay (§13). There is no local certificate.
- **Verified on iOS 18.7 (Round 0):**
  - Lenses: Front, Back Camera, Back Ultra Wide, Back Dual Wide.
  - `zoom` works, 0.5–10 on Back Dual Wide.
  - `torch` works on the rear cameras.
  - 720p30, 1080p30 and 4K30 are all delivered.
  - Lens and resolution switch live via `replaceTrack`.
  - If a capability is missing on another model, its control is hidden.
  - Manual focus/exposure is Phase 2 only.
- Safari's encoder bitrate is browser-controlled; `RTCRtpSender.setParameters({maxBitrate})` is a hint. 1080p60 is best-effort.
- USB fallback in Phase 1 uses iPhone **Personal Hotspot over USB** (creates a 172.20.10.x network). Requires hotspot enabled; some carriers block it.

## 4. Stack

| Layer | Choice | Why |
|---|---|---|
| Core logic | Rust | Portable to iOS (UniFFI) and macOS; no GC; low RAM/CPU |
| WebRTC | `str0m` (sans-IO Rust WebRTC) | Lightweight, runs on Windows and iOS; we own the I/O loop |
| Decode | FFmpeg (`ffmpeg-next`), **LGPL build, dynamically linked, only the H.264 decoder + hwaccels enabled** | NVDEC, D3D11VA and software decoding behind one API; minimal build keeps RAM and licensing clean |
| Color convert | libyuv (or GPU path) | SIMD NV12→BGR, ~1% CPU |
| Virtual webcam | softcam (DirectShow), **both 32-bit and 64-bit filter DLLs registered** | Proven approach (same family as OBS VirtualCam); visible in Zoom/Teams/Discord/OBS/Chrome, including 32-bit host apps |
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
│  ├─ phonecam-transport   # SHARED: str0m WebRTC + signaling server. Phase 1 implements and tests
│  │                       # only the RECEIVER role; API is role-agnostic so Phase 2 can add the sender.
│  ├─ phonecam-ffi         # SHARED: UniFFI bindings (Phase 2 consumer; stub in Phase 1)
│  ├─ decode-win           # Windows: FFmpeg decoder with Auto/NVDEC/D3D11/CPU
│  └─ vcam-win             # Windows: softcam wrapper + placeholder frames
├─ apps/
│  ├─ desktop/             # Tauri app (src-tauri + React UI)
│  ├─ web-client/          # Safari sender page (Vercel)
│  ├─ relay/               # Cloudflare Worker signaling relay
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
   ▲ page from Vercel (HTTPS)         │ relay client, tray icon        │
   └──── signaling (KBs) ── Cloudflare relay ── signaling ────────────┘
```

1. The desktop app starts, generates a random room id, connects to the Cloudflare relay as `desktop`, and shows a QR code for `https://<page>/#room=<id>&relay=<wss url>`.
2. The iPhone scans the QR code. Safari opens the Vercel page, the user taps Start, and `getUserMedia` runs. The page joins the same relay room as `phone`, SDP/ICE are exchanged through the relay, and the WebRTC peer is established. **Media flows directly over the LAN or USB, never through the relay.**
3. Video: phone hardware H.264 → RTP → str0m → decoder → NV12 → BGR → softcam shared memory.
4. DataChannel `ctl` (reliable): controls, state, battery level, heartbeat.
5. Preview: decoder output downscaled to ≤480p at ≤15 fps, sent to UI only while the window exists and is visible.

## 7. Connection & resilience

### 7.1 Path priority (Wi-Fi first, USB fallback)
- The desktop enumerates its interfaces and classifies each as **Wi-Fi**, **USB-hotspot** (Apple Mobile Device Ethernet / 172.20.10.0/28) or **other**.
- The desktop is an **ICE-lite** endpoint (str0m supports it); Safari is the controlling agent and nominates. The desktop influences the choice through **candidate local-preference**: Wi-Fi host candidates get the highest priority, USB-hotspot next. (Mechanism to be confirmed in Round 0; see §17.)
- Safari exposes real host candidates once camera permission is granted; the desktop also accepts peer-reflexive candidates, so mDNS `.local` candidates are never required.
- **Failover:** the phone is always the offerer, including on restarts. When the heartbeat (§7.2) detects loss, the desktop sends `restart_ice` over WSS (or the phone detects loss itself). The phone then creates an ICE-restart offer, and the desktop's answer lists only candidates on interfaces that are currently up, so the session moves to USB if Wi-Fi is gone. When Wi-Fi comes back and stays stable for 30 s, the desktop sends one `restart_ice` to move back to Wi-Fi. If local-preference turns out not to steer Safari's choice (Round 0 checkpoint 5), the desktop prunes its advertised candidates down to the preferred path.
- **Runtime interface watch (Round 0 finding):** the desktop must detect interfaces that appear or disappear while running, especially the Apple Mobile Device Ethernet adapter that shows up when USB Personal Hotspot starts. It binds a UDP socket on each new interface and adds that candidate on the next ICE restart. In Round 0 the spike bound only to startup interfaces, so Wi-Fi-off was unrecoverable.
- **Signaling resilience:** signaling goes through the relay. Both sides reconnect to the relay with backoff of 1 s → 30 s. Once the DataChannel is up, `restart_ice` and the re-offer travel over the DataChannel first, falling back to the relay if it's down. Signaling during a network switch therefore doesn't depend on internet access.
- The UI shows the active path badge (Wi-Fi / USB) and the RTT measured by the heartbeat on the active path.

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
- **Fixed output size (Round 0 finding):** the virtual camera's resolution is set by the user's output setting (default 1080p) and **never changes while apps are using it**. When the phone changes resolution, frames are scaled into the fixed output on the GPU (or with libyuv). The spike re-created the device on each change, which would drop the camera in Zoom.

### 7.3a Packet-loss recovery (Round 0 finding)
- On Wi-Fi, Round 0 saw 0.4–3.6% loss bursts every 30–60 s. Recovering only by PLI gave up to 1 s freezes.
- The real app negotiates **NACK + RTX** (retransmit lost packets; recovers within ~1 RTT, which is about 5 ms on the LAN), **TWCC** (bandwidth feedback, so Safari doesn't throttle), and a **small jitter buffer** (≤ 50 ms). PLI becomes the last resort.

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
| Lens | **default: Back Dual Wide** (zoom 0.5–10, one device spans ultra-wide to tele, like the Camera app); other lenses via `deviceId` + `replaceTrack` | segmented buttons | 0.5× / 1× / 2× buttons above viewfinder (zoom presets on Dual Wide) |
| Resolution / FPS | re-acquire + `replaceTrack` (live, no renegotiation; proven in Round 0) | select | — |
| Zoom | `applyConstraints({zoom})` | slider + mouse wheel / touchpad pinch over preview | **pinch on viewfinder** (page zoom disabled) |
| Torch | `applyConstraints({torch})` | toggle | toggle |
| Mirror (output) | applied on desktop before softcam; **default off**, so others see you correctly | toggle | — |
| Mirror (preview) | CSS flip of the phone preview only; remembered per phone | — | toggle |
| Rotation 0/90/180/270 | applied on desktop before softcam | button | — |

Unsupported capabilities (per `getCapabilities()`) are hidden, not disabled.

## 9. Energy profiles

| Profile | Video | Target bitrate |
|---|---|---|
| Economy | 720p30 | ~2.5 Mbps |
| Balanced (default) | 1080p30 | ~5 Mbps |
| Quality | 4K30 (verified in Round 0) or 1080p60 (best-effort) | ~10–20 Mbps (hint via `maxBitrate`) |

Phone sends exactly the selected resolution — no scaling on the PC.

**Black-screen mode (phone):** full-screen pure-black overlay (OLED ≈ zero power), video keeps streaming. Activated by button or automatically after 10 s without touch. Tap to show controls for 5 s. Page suggests lowering brightness via Control Center (Safari can't set it).

## 10. Decoder selection

- Setting: **Auto (default)** | NVIDIA (CUDA/NVDEC) | GPU (D3D11VA) | CPU.
- Auto order (**user decision 2026-10-30: always use NVIDIA when available**): **D3D11VA on the NVIDIA adapter** if an NVIDIA GPU is present (it uses the same NVDEC hardware without loading the CUDA runtime, which saves RAM) → D3D11VA on any other GPU → CPU. Round 0 confirmed Auto picks the RTX 5060 Ti and decodes 4K30.
- "NVIDIA (CUDA/NVDEC)" is an explicit opt-in; it costs more RAM (CUDA context) and is excluded from the tray-only RAM target.
- Any hardware init/decode failure falls back to the next option automatically and the UI shows a notice.
- UI shows active decoder and app CPU/GPU usage.
- Note: encoding happens on the iPhone (always hardware). The PC only decodes.

## 11. Performance targets (measured by test)

These are **targets** that Round 0 validates; if the spike shows one is unrealistic, the number is revised in this spec before Round 1 rather than silently missed.

**Measurement method:** RAM = sum of **private working set** across the app's whole process tree (Rust main process + every WebView2 child process when a window exists). CPU = average % of total CPU across all cores over 10 min, sampled every 1 s, same process tree. Reference machine: the user's PC.

| Scenario | RAM (whole process tree) | CPU |
|---|---|---|
| Tray only, streaming 1080p30, Auto (D3D11VA) | < 60 MB | < 3% |
| Tray only, streaming 1080p30, CPU decode | < 40 MB | < 5% |
| Tray only, streaming 4K30, Auto (D3D11VA) | < 150 MB | < 5% |
| Window open, streaming 1080p30, preview on | < 200 MB | < 6% |

Round 0 baseline for the receiver alone at 1080p30: D3D11VA 44.7 MB / 1.5%, CPU 21.6 MB / 1.1%, CUDA 134 MB / 1.2%. At 4K with three softcam re-creations it reached about 226 MB; the fixed output size (§7.3) removes the re-creations.

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
- Empty state: a QR code plus "Scan with your iPhone camera". There is no setup step.

**Phone page:**
- Full-screen viewfinder with pinch zoom, lens buttons, status pill.
- Big "Start" on first load; big "Resume" after interruption.
- Black-screen button.

## 13. Hosting & first-time setup (decided after Round 0)

The local CA approach is **dropped**. It needed profile install + trust steps, and on the user's phone it failed on the first try because the spike regenerated the CA on each run.

- **Phone page:** static files in `apps/web-client/`, deployed to **Vercel** with a publicly trusted HTTPS certificate. Git pushes to `main` auto-deploy (the root `vercel.json` points the output directory at the page).
- **Signaling relay:** a **Cloudflare Worker** with a Durable Object `Room` (`apps/relay/`).
  - Each room holds at most 2 peers (desktop, phone) and needs an id of at least 128 bits.
  - Text is forwarded verbatim, with a 64 KB message cap and **nothing stored**.
  - It's free tier, using WebSocket Hibernation.
- **Setup is just scanning the QR code.** There is nothing to install or trust on the phone.
- **Needs internet only for signaling** (pairing and relay-based reconnects). Video never touches the internet. Once paired, ICE restarts go over the DataChannel when possible (§7.1).

## 14. Protocol (summary)

- Relay control (from the relay itself): `{"t":"peer","up":bool}`.
- Signaling (through the relay, or over the DataChannel once it's up; JSON): `hello{token,role,version,proof}`, `offer` (always phone → desktop), `answer`, `restart_ice` (desktop → phone), `bye`.
- Control (DataChannel `ctl`, JSON): `ping{t}` / `pong{t,battery,charging,camState}`, `set{lens|res|fps|zoom|torch|profile}`, `state{...actual values, capabilities}`, `paused{reason}`, `resumed`.
- Versioned (`version` field); Rust types are the source of truth; TS types generated.

## 14a. Security & Windows integration

- **Pairing:** the QR URL fragment carries a random 128-bit **room id** and a separate 128-bit **pairing secret** `k`. Fragments never reach Vercel or Cloudflare in HTTP requests.
  - The relay sees only the room id. The secret proves identity end-to-end: `hello.proof` is an HMAC of the desktop's nonce under `k`, checked by the desktop.
  - SDP is signed with `k` too, so a relay that tampered with it would be detected; this pins the DTLS fingerprints.
  - A successful pairing yields a long-lived device token (in the page's `localStorage` and in desktop app data) for reconnects. "Forget phone" revokes it and rotates the secret.
- **One phone at a time:** the relay already allows only one `phone` per room, and the desktop rejects an unproven peer.
- **LAN exposure:** the desktop opens **no listening TCP ports**, only UDP sockets for WebRTC on private-range interfaces.
- **Windows Firewall:** the installer adds an inbound UDP rule for the app on the **Private** profile only. If the active network is classified Public, the UI shows a tip.
- **Single instance:** a second launch focuses the existing tray app instead.

## 15. Error handling

- Every failure surfaces as a state in core, never a crash: decoder failure → fallback; softcam missing → UI error with "Repair" (re-register, needs admin once); relay unreachable → keep any live session, retry with backoff, and show "Offline: can't pair new phone" in the UI; UDP port in use → pick another port.
- Logs: rotating file in app data, 5 MB cap.

## 16. Testing

- **Unit (Rust, `cargo test`):** core reconnection state machine (all transitions), heartbeat timing, path priority, profiles, protocol serde, pairing HMAC handshake (valid/invalid/replayed), device-token revocation and secret rotation.
- **Unit (TS, Vitest):** phone page control handling, pinch→zoom mapping, interruption handling (mocked tracks).
- **Integration:** headless Chromium sender (fake camera) ↔ desktop receiver over loopback: connect, drop network, verify ICE restart, verify placeholder frames, verify recovery.
- **Performance test:** scripted 10-minute stream at 1080p30, measures RAM/CPU against §11 targets using the §11 method.
- **Virtual camera:** automated check that "PhoneCam" enumerates through DirectShow from both a 32-bit and a 64-bit test host and delivers frames in each state (Idle/Live/Reconnecting/Paused).
- **Manual checklist (real iPhone):** Wi-Fi drop → USB takeover, incoming call → paused → resume, lock/unlock, 2-hour session, each decoder mode.

## 17. Delivery plan outline (token-economy)

- **Round 0 (spike, Opus-reviewed):** a throwaway Rust binary plus a minimal Safari page. Checkpoints, each with a pass/fail result written to `docs/spike-results.md`:
  1. Safari H.264 → str0m ICE-lite: the connection establishes and frames get depacketized into complete access units.
  2. Keyframe recovery: on packet loss, str0m sends a PLI/NACK and Safari responds with a keyframe within 1 s.
  3. Decode with D3D11VA (NVIDIA adapter), CUDA/NVDEC and CPU; measure RAM/CPU for each against the §11 targets.
  4. softcam: frames visible in a 64-bit and a 32-bit DirectShow host.
  5. Multi-path: with Wi-Fi + USB hotspot both up, check which pair gets nominated, then test the ICE-restart failover from Wi-Fi to USB.
  6. On the user's iPhone: the `zoom`/`torch` capabilities, the lens list, and the maximum resolution/FPS actually delivered.
  7. On the user's iPhone: the name-constrained CA installs, gets trusted, and the HTTPS page loads with no warning. If iOS rejects the Name Constraints, drop the extension and keep the other mitigations.
  - **No-go criteria:** checkpoint 1 or 2 can't be made to pass within the spike. **Fallback** (keeps the Rust core and the portability goal): swap str0m for `webrtc-rs`, which has a fuller RTP pipeline (NACK/TWCC interceptors), behind the same `phonecam-transport` API. Electron is not a fallback.
  - The §11 targets get revised from checkpoint 3 and 6 results before Round 1.
  - **Round 0 outcome (2026-10-30): GO for str0m.**
    - Checkpoints 1, 2, 3 and 6 PASS on a real iPhone.
    - Checkpoint 7 is superseded by Vercel + relay.
    - Checkpoint 4 is pending admin registration.
    - Checkpoint 5 failed in the spike, as expected with no failover code; it is now a Round 1 requirement (§7.1, §7.3a).
    - See `docs/spike-results.md`.
- **Round 1 (≤3 agents, separate worktrees):**
  1. `decode-win` + `vcam-win` (fixed output size + scaling).
  2. `phonecam-core` + `phonecam-transport` (NACK/RTX/TWCC, interface watch, heartbeat, `restart_ice`, relay client).
  3. `web-client` + `relay` (moved out of `spike/`, with pairing proof).
- **Round 2 (≤3 agents):**
  1. Desktop React/shadcn UI + tray lifecycle.
  2. Resilience integration (§7).
  3. Tests + perf harness.
- `npm install` / `cargo build` serialized; never in parallel.

## 18. Out of scope (Phase 1)

Audio, multiple phones at once, Android, macOS receiver, manual focus/exposure/WB, auto brightness, recording, auto-updates, start-with-Windows, App Store app.
