# HANDOFF — PhoneCam

Updated: 2026-10-30

## Done
- Public repo: https://github.com/orbssexp/phonecam (branch `main`, MIT). History rewritten so every commit is by `orbssexp`; the `spike/round-0` branch was merged into `main` and deleted.
- Spec approved: `docs/superpowers/specs/2026-10-29-phonecam-design.md`.
- Round 0 spike (`spike/`): checkpoint 1 PASS, 2 PASS, 3 measured. Rust receiver at 1080p30: cpu 21.6 MB / 1.1%, d3d11 44.7 MB / 1.5%, cuda 134 MB / 1.2%. str0m = GO. See `docs/spike-results.md`.
- **Relay mode** (replaces the local CA/profile setup for phase 1):
  - Sender page on Vercel: https://phonecam-eta.vercel.app (project `phonecam`, deploy from `spike/web`: `npx vercel deploy --prod`).
  - Signaling relay on Cloudflare: Worker `phonecam-relay`, https://phonecam-relay.phonecam-relay.workers.dev (deploy from `spike/relay`: `npx wrangler deploy`). Durable Object `Room`, at most 2 peers, stores nothing.
  - Local e2e PASS (first 1080p frame decoded through the relay).
  - Run: `spike\run.bat --relay wss://phonecam-relay.phonecam-relay.workers.dev --page https://phonecam-eta.vercel.app/`

## Pending
- **Blocked on user:**
  - Admin `regsvr32` for softcam (checkpoint 4).
  - iPhone test in relay mode (checkpoints 5 and 6; 7 is obsolete if relay mode is adopted).
- **Decision needed from user:** Auto decoder order. CPU decode measured lighter than D3D11VA at 1080p30.
- **Not started:**
  - Update the spec (§13 cert → Vercel page + relay signaling; §10/§11 numbers).
  - Implementation plan (superpowers:writing-plans) for Rounds 1–2. Round 1 must add NACK/RTX and TWCC (bandwidth feedback).
- In relay mode spike-rx still starts the local HTTPS/CA servers. Harmless, but remove them in the real app.
- spike-rx recreates the softcam device when the resolution changes (decode.rs). Apps like Zoom lose the camera when that happens. The real app must keep a fixed output size and scale frames into it (spec §7.3).

## Rules learned
- Agents NEVER open a visible browser or the user's profile: headless Edge only, with a temp `--user-data-dir`.
- Route grep, log reading and metric parsing to `agy` (token-economy §1). Subagents decline agy's skip-permissions flag when relayed, so run agy from the main session. `agy -p` sometimes returns empty output; check it and fall back to doing the work directly.
- Stray worktree `C:\Users\gusta\orca\workspaces\phone-cam\redfish` (branch `zxgusst/redfish`, at the initial commit) belongs to another tool. It was not pushed; don't touch it.

## Update 2026-10-30 (later)
- iPhone tests (results in `docs/spike-results.md`):
  - Live lens/resolution switching works, and so do 4K30, torch, and zoom 0.5–10 on Back Dual Wide.
  - Wi-Fi-off did NOT fail over: the spike has no failover. This is now a Round 1 requirement.
- Decisions:
  - Decoder: **always NVIDIA when available** (user choice B).
  - Vercel + Cloudflare relay replaces the local CA; the spec is revised (a9a458b).
  - The user delegates technical decisions (see memory).
- Plan 1 of 5 written: `docs/superpowers/plans/2026-10-30-phonecam-core.md` (portable core, 9 TDD tasks, 40 tests). It was reviewed and the fixes are applied (e6c0518).
- Next plans: transport → decode/vcam → web-client + relay → desktop UI.
- Still blocked on the user: `regsvr32` (admin) to make "PhoneCam" visible in apps.

## Next step
phonecam-core DONE (9135fe5): 40 tests, clippy clean, iOS check ok (verified by agy). Next: transport plan as SHORT contracts (Opus), implementation by agy (Haiku fallback).

## Previous next step
Relay DNS live → user runs the relay command and scans the QR on the iPhone → record checkpoints 5–6 → update the spec → write the plan.

## Update 2026-10-30 (round 1 merged)
- main c732a57+: phonecam-transport (NACK fix, interface watch, pairing), phonecam-media-win (fixed-size vcam, letterbox, placeholders), apps/web-client + apps/relay. Workspace tests + clippy clean. Pushed (Vercel redeploys apps/web-client). Relay unchanged, no redeploy.
- Round 2 running: D1 Sonnet (apps/desktop/src-tauri, worktree phonecam-wt-desktop), D2 agy (React UI, phonecam-wt-ui, log ../agy-ui.log), D3 agy (page caps/set, phonecam-wt-web2, log ../agy-web2.log). Contract: plans/2026-10-30-mvp-contracts.md "D contract detail".
- Round 2 merged (4b1cc4d+). Installer built: target/release/bundle/nsis/PhoneCam_0.1.0_x64-setup.exe (41.6 MB). Next: user installs + tests (iPhone, Zoom, Wi-Fi off → USB, call/lock). agy quota out until ~2026-11-05 → Haiku fallback.

## Update 2026-11-10 (finalization)
- Installed build (e2cd178, TLS fix) validated end to end with `scripts/dev/e2e.sh` (headless fake phone, temp profile): relay connects, video decodes, vcam frame 1920x1080, "Reconnecting…" placeholder after phone leaves, tray mode works, CPU 0.8% at 1080p30.
- Found: vcam is named "DirectShow Softcam" (not "PhoneCam"); memory 59 MB idle → 205 MB after first session, never released; UI: "No path"/no latency while live, right panel scrolls, no QR while reconnecting.
- Running (worktrees ../phonecam-wt-f1-ui, -f2-vcam, -f3-engine; branches fin/*): F1 agy UI (log ../agy-f1.log), F2 agy softcam rename → spike/vendor/softcam-phonecam (log ../agy-f2.log), F3 Sonnet memory + path diagnosis. Contracts: plans/2026-10-30-mvp-contracts.md "Finalization round".
- Next: verify each → merge → `cd apps/desktop && npm run tauri build` → user reinstalls (admin; re-registers the camera as "PhoneCam") → rerun e2e with CAM=PhoneCam → user tests with the real iPhone.
- `git stash@{0}` holds a stray duplicate `Profile` in core/controls.rs written by a resumed old agent; it breaks the build — drop it, don't apply.
- agy quota is back (checked 2026-11-10). User rule: stop and hand off before the session passes 70% usage.

## Update 2026-11-10 (finalization merged)
- main: F1 UI (QR while reconnecting, panel fits 1000x720), F2 vcam renamed "PhoneCam" (new CLSID {4CD75097-5A06-4341-9D29-8C12662AC2C0}, vendor dir spike/vendor/softcam-phonecam, `scripts/build-softcam.ps1`), F3 decoder released 3 s after stream stops + heartbeat ping fix (latency), battery % fix. 80 tests pass, clippy clean.
- New build tested from target/release with temp APPDATA: idle 52 MB; live 1080p30 window open 115 MB, 1.85% CPU; 12 s after phone leaves 20 MB. UI shows Wi-Fi / 1 ms / d3d11va NVIDIA; QR returns on reconnecting.
- Installer rebuilt: target/release/bundle/nsis/PhoneCam_0.1.0_x64-setup.exe. NOT yet installed by the user.
- Blocked on user: uninstall the old PhoneCam (unregisters "DirectShow Softcam"), install the new one (admin), then: `CAM=PhoneCam bash scripts/dev/e2e.sh` must grab a frame; then real-iPhone tests (lenses/zoom/torch from PC, lock/call recovery, Wi-Fi off → USB).
- User's settings.json has decoder "cuda" (≈205 MB); "auto" is lighter (≈110 MB) and still NVIDIA.

## Update 2026-11-10 (real iPhone connects)
- Root cause of "phone live / desktop reconnecting": no inbound firewall rule for phonecam.exe on a Public network. Installer now adds it (netsh, UDP in) and swaps an in-use softcam.dll by renaming it (no need to close Brave etc.). User reinstalled over the old one → iPhone connects, vcam "PhoneCam" registered.
- User-found bugs, now "Round G" in plans/2026-10-30-mvp-contracts.md: sideways video + no rotate control; 4K shows 1080; unstable/disconnects; fps display wrong (60). App has NO log file yet → G1 adds %APPDATA%\PhoneCam\phonecam.log; instability can only be diagnosed from that log after the user's next real session.
- Next: G1 Sonnet (engine), G2 agy (UI), G3 agy (page) in worktrees ../phonecam-wt-g1..g3 (branches fin/g1-engine, fin/g2-ui, fin/g3-page) → verify (tests, clippy, scripts/dev/e2e.sh with CAM=PhoneCam) → merge → `npm run tauri build` → user reinstalls over the top → user tests → read phonecam.log for disconnect causes.

## Update 2026-11-10 (Round G merged)

Done: G1 (engine: `phonecam.log`, rotation auto CVO + manual, real stream stats, heartbeat/resume
tolerance), G2 (UI: Rotate button, rows "Phone sends / Received / Camera output"), G3 (page: caps with
width/height/fps/rotation, exact constraints, no fake 60 fps) merged into `main` (440b1cd).
Checked: 91 tests pass, clippy 0 errors, installer built
(`target/release/bundle/nsis/PhoneCam_0.1.0_x64-setup.exe`). Fake-phone e2e was SKIPPED for this build
because the user's installed app was running; Round G is unverified on a real iPhone.

Pending (start a fresh session):
1. User installs over the top (no uninstall, no closing apps) and tests with the iPhone: rotation,
   4K, fps rows, stability.
2. Read `%APPDATA%\PhoneCam\phonecam.log` (parse with agy) to find the real disconnect cause; fix.
3. Known unfixed: if str0m closes the session (DTLS close_notify / SCTP lost) the page needs a reload.
4. Unverified on hardware: Wi-Fi -> USB failover, lock/call recovery, PC-side lens/zoom/torch/mirror.
5. Cleanup: worktrees `phonecam-wt-*`, `git stash drop` (stash@{0} is a stray duplicate, do not apply).

## Update 2026-11-10 (real iPhone test of round G + Iriun inspection)

User report: still unstable, 4K does not work, front camera breaks when settings are changed a lot.

Evidence from `%APPDATA%\PhoneCam\phonecam.log`:
- Phone caps report 3840x2160 but the decoded stream is always 1920x1080: Safari's WebRTC encoder
  caps at 1080p. 4K is not reachable from the Safari page.
- Freeze storms: 10 s windows with 0 AUs, +10 gaps, +10 PLI, 0 keyframes, NACK ~0 (119x "no keyframe
  after PLI", 79x GAP). Loss is not being repaired by NACK and keyframes never complete. Not yet
  diagnosed (suspects: small UDP receive buffer, RTX/NACK not active after track replace).
- After many profile/lens changes: heartbeat lost for 60 s+, ConnectTimedOut, 14x restart_ice.

Iriun (strings/imports of the installed binaries only): native iOS app, discovery by mDNS
`_iriunwebcam._tcp`, video over plain TCP (TCP_NODELAY, enlarged socket buffers), USB via usbmuxd
(tcp 4697/4699), FFmpeg decode, up to 3840x2160, native controls (ISO, exposure time/bias, white
balance, focus lock, zoom, flash, flip).

Direction proposed to the user (awaiting answer): bring phase 2 (native Swift sender) forward.
UI reference from the user: near-black background, thin 1px borders, square corners, small
bracketed mono labels ("[ JOIN SESSION ]"), large light heading, preview left + settings card right.
