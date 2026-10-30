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
Execute the phonecam-core plan with superpowers:subagent-driven-development (1 task at a time: Sonnet implementer + review), then write the transport plan.

## Previous next step
Relay DNS live → user runs the relay command and scans the QR on the iPhone → record checkpoints 5–6 → update the spec → write the plan.
