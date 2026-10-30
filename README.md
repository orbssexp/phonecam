# PhoneCam

Use your iPhone as a Windows webcam. It's a lightweight, open-source alternative to Iriun, built for:

- **A stable connection.** Wi-Fi first with automatic USB fallback, a heartbeat, and auto-reconnect.
- **A webcam that never disappears.** Zoom, Teams and OBS keep the "PhoneCam" device during calls, phone locks and network drops; it shows a placeholder until video comes back.
- **Low RAM/CPU.** A Rust core with a choice of decoder (Auto / NVIDIA / D3D11 / CPU), and a UI that unloads completely when the app is minimized to the tray.
- **Real camera control.** Lens, resolution/FPS, pinch-to-zoom, torch, mirror and rotation.

> **Status:** early development. Round 0 (technical spike) is done; its throwaway code lives in [`spike/`](spike/). See [`docs/spike-results.md`](docs/spike-results.md).

## How it works

```
iPhone (Safari page)  ──WebRTC H.264, direct over Wi-Fi/USB──►  Windows app (Rust)
                                                                  └─► virtual webcam "PhoneCam"
          └── signaling only (a few KB) via a tiny Cloudflare Worker relay ──┘
```

Video never leaves your local network. The relay only helps the two devices find each other.

## Stack

- **Desktop:** Tauri 2, Rust (`str0m` WebRTC, FFmpeg decode, `softcam` DirectShow virtual camera), React + shadcn/ui.
- **Phone, phase 1:** a Safari web page.
- **Phone, phase 2:** a native SwiftUI app that reuses the Rust core via UniFFI.

## Docs

- Design spec: [`docs/superpowers/specs/2026-10-29-phonecam-design.md`](docs/superpowers/specs/2026-10-29-phonecam-design.md)
- Spike plan and results: [`docs/spike-plan.md`](docs/spike-plan.md), [`docs/spike-results.md`](docs/spike-results.md)

## License

MIT. FFmpeg is used as an LGPL shared library, and softcam is MIT.
