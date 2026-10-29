# HANDOFF — PhoneCam

Updated: 2026-10-29

## Done
- Spec approved: `docs/superpowers/specs/2026-10-29-phonecam-design.md` (master: f5d2f3c, c029461, 58a5cea)
- Branch `spike/round-0`: plan (06c205e), spike-rx scaffold (ccfa30e), headless-only browser fix (090f4fa, dfc0d99), results (513fd7e)
- Toolchain on this PC: VS 2022 Build Tools (C++), Rust 1.99.1 MSVC + i686, LLVM. GPU RTX 5060 Ti.
- Spike results (`docs/spike-results.md`): checkpoint 1 PASS, 2 PASS, 3 measured. Rust receiver at 1080p30: cpu 21.6 MB / 1.1%, d3d11 44.7 MB / 1.5%, cuda 134 MB / 1.2%. str0m = GO.

## Pending
- **Blocked on user:** admin `regsvr32` for softcam (checkpoint 4); iPhone checkpoints 5–7 (steps in spike-results.md §5-7).
- **Decision needed from user:** Auto decoder order. CPU decode measured lighter than D3D11VA at 1080p30 on this PC.
- **Not started:** revise spec §10/§11 from the results; implementation plan (superpowers:writing-plans) for Rounds 1–2. Round 1 must add NACK/RTX and TWCC (bandwidth feedback).

## Rules learned
- Agents NEVER open a visible browser or the user's profile: headless Edge only, with a temp `--user-data-dir`.
- Route grep, log reading and metric parsing to `agy` (token-economy §1). Subagents decline to run agy's skip-permissions flag when the instruction is relayed, so run agy from the main session.

## Next step
User registers softcam (admin) → verify checkpoint 4 → user runs iPhone checkpoints 5–7 → update spec → write plan.
