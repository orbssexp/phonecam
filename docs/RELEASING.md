# Releasing PhoneCam (desktop)

Installed copies check `https://github.com/orbssexp/phonecam/releases/latest/download/latest.json`
at start and every 6 h, and show an "[ UPDATE ] vX.Y.Z available" banner plus a tray entry.

## Cut a release

1. Bump the version to X.Y.Z in all three places (the script refuses otherwise):
   `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/Cargo.toml`,
   `apps/desktop/package.json`. Commit and push to `main`; the tree must be clean.
2. Rehearse (builds and signs, writes `dist-release\`, publishes nothing):
   `pwsh scripts/release.ps1 -Version X.Y.Z -DryRun`
3. Publish:
   `pwsh scripts/release.ps1 -Version X.Y.Z [-Ipa path\to\PhoneCam.ipa] [-Notes "text"]`

Assets of release `vX.Y.Z`: `PhoneCam_X.Y.Z_x64-setup.exe`, `PhoneCam_X.Y.Z_x64-setup.exe.sig`,
`latest.json` (fixed name, this is what `latest/download` resolves), optional `.ipa`.
Do not mark the release as a pre-release: `latest` ignores those.

## Signing key

`%USERPROFILE%\.phonecam\updater.key` (no password), outside the repo; never commit or paste it.
The public half is in `tauri.conf.json` (`plugins.updater.pubkey`). Back the private key up
somewhere safe (password manager).

**Losing the key means installed apps can never verify another update: every user must download
and install the new build by hand, and the new build must ship a new public key.** Do not
regenerate the key casually for the same reason.

## What happens on the user's PC

The updater downloads the signed installer, verifies the signature, starts it (Windows shows the
UAC prompt because the installer is per-machine; the install itself is passive, only a progress
bar) and the app exits. The installer re-registers the camera DLL, refreshes the firewall rule
(`windows/hooks.nsh`) and relaunches PhoneCam. Declining UAC leaves the running app untouched.
