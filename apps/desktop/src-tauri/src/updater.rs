//! Auto-update. Lives in Rust (not the webview) because the webview is destroyed when the app is
//! closed to the tray. State reaches the UI through `AppState.updateVersion/updateProgress`; the
//! tray menu gains an "Install update" entry while one is pending.
//!
//! Install flow on Windows: the plugin downloads the signed NSIS installer, verifies the
//! signature, launches it with `ShellExecuteW` (the installer is perMachine so Windows shows the
//! UAC prompt; passive mode shows only a progress bar) and then calls `process::exit(0)` so the
//! exe can be replaced. The installer runs our NSIS hooks (DLL retire/register, firewall rule),
//! and its `/R` flag relaunches the app non-elevated afterwards. If the user declines UAC the
//! launch fails *before* the exit, so the app simply keeps running with nothing torn down.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder};
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Status {
    update: Option<Update>,
    /// Download progress in percent while installing.
    progress: Option<u8>,
    installing: bool,
}

static STATUS: Mutex<Status> = Mutex::new(Status { update: None, progress: None, installing: false });

fn status() -> MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `(updateVersion, updateProgress)` for `AppState`.
pub fn snapshot() -> (Option<String>, Option<u8>) {
    let s = status();
    (s.update.as_ref().map(|u| u.version.clone()), s.progress)
}

pub(crate) fn percent(done: u64, total: Option<u64>) -> u8 {
    match total {
        Some(t) if t > 0 => (done.saturating_mul(100) / t).min(100) as u8,
        _ => 0,
    }
}

/// Tray menu: Open, [Install update vX], Quit.
pub fn tray_menu<R: Runtime>(app: &AppHandle<R>, version: Option<&str>) -> tauri::Result<Menu<R>> {
    let open = MenuItemBuilder::with_id("open", "Open").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let mut b = MenuBuilder::new(app).item(&open);
    if let Some(v) = version {
        let up = MenuItemBuilder::with_id("install_update", format!("Install update v{v}")).build(app)?;
        b = b.item(&up);
    }
    b.item(&quit).build()
}

fn refresh_tray(app: &AppHandle, version: Option<&str>) {
    if let Some(tray) = app.tray_by_id("tray") {
        match tray_menu(app, version) {
            Ok(m) => {
                let _ = tray.set_menu(Some(m));
            }
            Err(e) => tracing::warn!("[update] tray menu: {e}"),
        }
    }
}

async fn check_once(app: &AppHandle) {
    if status().installing {
        return;
    }
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return tracing::warn!("[update] updater unavailable: {e}"),
    };
    match updater.check().await {
        Ok(Some(u)) => {
            let version = u.version.clone();
            let changed = {
                let mut s = status();
                let changed = s.update.as_ref().map(|c| c.version.as_str()) != Some(version.as_str());
                if !s.installing {
                    s.update = Some(u);
                }
                changed
            };
            if changed {
                tracing::info!("[update] v{version} available");
                refresh_tray(app, Some(&version));
            }
        }
        Ok(None) => tracing::info!("[update] up to date"),
        Err(e) => tracing::warn!("[update] check failed: {e}"),
    }
}

/// Check shortly after start and then every 6 h.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check_once(&app).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

async fn install(app: &AppHandle) -> Result<(), String> {
    let update = {
        let mut s = status();
        if s.installing {
            return Ok(());
        }
        let Some(u) = s.update.clone() else {
            return Err("no update available".into());
        };
        s.installing = true;
        s.progress = Some(0);
        u
    };
    let result = async {
        let mut done = 0u64;
        let bytes = update
            .download(
                |chunk, total| {
                    done += chunk as u64;
                    status().progress = Some(percent(done, total));
                },
                || {},
            )
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        status().progress = Some(100);
        tracing::info!("[update] downloaded v{}, launching installer", update.version);
        // On Windows this exits the process once the installer has been launched.
        update.install(bytes).map_err(|e| format!("install failed: {e}"))
    }
    .await;
    {
        let mut s = status();
        s.installing = false;
        s.progress = None;
    }
    if let Err(e) = &result {
        tracing::warn!("[update] {e}");
        check_once(app).await;
    }
    result
}

/// Download, verify, install, relaunch. Does not return on success (the process exits).
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    install(&app).await
}

/// Tray entry point.
pub fn spawn_install(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let _ = install(&app).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_bounded() {
        assert_eq!(percent(0, Some(200)), 0);
        assert_eq!(percent(50, Some(200)), 25);
        assert_eq!(percent(300, Some(200)), 100);
        assert_eq!(percent(10, None), 0);
        assert_eq!(percent(10, Some(0)), 0);
    }
}
