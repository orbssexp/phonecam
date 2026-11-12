//! PhoneCam desktop app. Rust owns transport + media + settings; the webview is a thin view over
//! the IPC below. No video frames cross IPC except the (<= 15 fps, 640 px) preview JPEG.

mod logging;
mod settings;
mod state;
mod updater;

use base64::Engine;
use phonecam_core::Ctl;
use phonecam_media_win::{Media, MediaHandle};
use phonecam_transport::native::{NativeHandle, NativeReceiver};
use phonecam_transport::{Config, Receiver, ReceiverHandle, TransportEvent};
use settings::{Diff, Pairing, Settings, SettingsPatch};
use state::{AppState, View};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokio::sync::mpsc::Receiver as EventRx;

const DEFAULT_RELAY: &str = "wss://phonecam-relay.phonecam-relay.workers.dev";
const DEFAULT_PAGE: &str = "https://phonecam-eta.vercel.app/";
/// Fixed virtual-camera size: softcam is created once per process (spec 7.3).
const VCAM_SIZE: (u32, u32) = (1920, 1080);
const WINDOW_LABEL: &str = "main";

struct Inner {
    settings: Settings,
    pairing: Pairing,
    view: View,
    recv: Option<ReceiverHandle>,
    native: Option<NativeHandle>,
    phone_url: String,
    qr_svg: String,
}

pub struct Shared {
    dir: PathBuf,
    relay_url: String,
    page_url: String,
    media: MediaHandle,
    /// Bumped on every receiver (re)start so a stale event thread stops.
    generation: AtomicU64,
    /// Last RTP video-orientation seen on the stream (degrees clockwise), -1 when none.
    cvo: AtomicI32,
    /// Rotation hint from the phone's caps, used only while `cvo` is -1.
    rot_hint: AtomicU32,
    /// Which source owns the session (`SRC_*`): the first to authenticate, native preferred.
    active: AtomicU8,
    inner: Mutex<Inner>,
}

const SRC_NONE: u8 = 0;
const SRC_WEBRTC: u8 = 1;
const SRC_NATIVE: u8 = 2;

impl Shared {
    fn new() -> Arc<Self> {
        let dir = settings::data_dir();
        let settings = settings::load_settings(&dir);
        let pairing = settings::load_or_create_pairing(&dir);
        let media = Media::start(settings.decoder.pref(), VCAM_SIZE);
        media.set_mirror(settings.mirror);
        media.set_rotation(settings.rotation);
        let relay_url = std::env::var("PHONECAM_RELAY").unwrap_or_else(|_| DEFAULT_RELAY.into());
        let page_url = std::env::var("PHONECAM_PAGE").unwrap_or_else(|_| DEFAULT_PAGE.into());
        let shared = Arc::new(Shared {
            dir,
            relay_url,
            page_url,
            media,
            generation: AtomicU64::new(0),
            cvo: AtomicI32::new(-1),
            rot_hint: AtomicU32::new(0),
            active: AtomicU8::new(SRC_NONE),
            inner: Mutex::new(Inner {
                settings,
                pairing,
                view: View::default(),
                recv: None,
                native: None,
                phone_url: String::new(),
                qr_svg: String::new(),
            }),
        });
        shared.restart_receiver(&mut shared.lock());
        shared
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// (Re)start the transport receiver for the current pairing. Caller holds the lock.
    fn restart_receiver(self: &Arc<Self>, inner: &mut Inner) {
        if let Some(old) = inner.recv.take() {
            old.shutdown();
        }
        if let Some(old) = inner.native.take() {
            old.shutdown();
        }
        self.active.store(SRC_NONE, Ordering::SeqCst);
        let cfg = Config {
            relay_url: self.relay_url.clone(),
            page_url: self.page_url.clone(),
            room_id: inner.pairing.room_id.clone(),
            secret: inner.pairing.secret(),
        };
        inner.phone_url = cfg.phone_url();
        inner.qr_svg = state::qr_svg(&inner.phone_url);
        inner.view = View::default();
        self.cvo.store(-1, Ordering::Relaxed);
        self.rot_hint.store(0, Ordering::Relaxed);
        self.media.set_auto_rotation(0);
        // Only a short room prefix is logged; never the secret or the full URL.
        tracing::info!(
            "[app] starting receiver for room {}...",
            inner.pairing.room_id.chars().take(6).collect::<String>()
        );
        let (nhandle, nrx) = NativeReceiver::start(inner.pairing.secret());
        inner.native = Some(nhandle);
        let (handle, rx) = Receiver::start(cfg);
        inner.recv = Some(handle);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.media.set_frame(phonecam_core::VcamFrame::Placeholder);
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("phonecam-events".into())
            .spawn(move || me.event_loop(rx, generation, SRC_WEBRTC))
            .expect("spawn event thread");
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("phonecam-native-events".into())
            .spawn(move || me.event_loop(nrx, generation, SRC_NATIVE))
            .expect("spawn native event thread");
    }

    fn event_loop(&self, mut rx: EventRx<TransportEvent>, generation: u64, src: u8) {
        while let Some(ev) = rx.blocking_recv() {
            if self.generation.load(Ordering::SeqCst) != generation {
                break;
            }
            if self.admit(src, &ev) {
                self.handle_event(ev);
            }
        }
    }

    /// Source arbitration. The native source reports `PhoneJoined` only after it authenticated, so
    /// it takes the session (preempting WebRTC). WebRTC claims it on `MediaConnected` (which
    /// follows its pairing check) unless native already owns it; while native owns it, WebRTC
    /// events are dropped. Returns whether the event should be handled.
    fn admit(&self, src: u8, ev: &TransportEvent) -> bool {
        let reset_rotation = || {
            self.cvo.store(-1, Ordering::Relaxed);
            self.rot_hint.store(0, Ordering::Relaxed);
        };
        let cur = self.active.load(Ordering::SeqCst);
        match (src, ev) {
            (SRC_NATIVE, TransportEvent::PhoneJoined) => {
                if cur != SRC_NATIVE {
                    tracing::info!("[app] native source took the session");
                    self.active.store(SRC_NATIVE, Ordering::SeqCst);
                    reset_rotation();
                }
                true
            }
            (SRC_NATIVE, TransportEvent::PhoneLeft) => {
                if cur == SRC_NATIVE {
                    self.active.store(SRC_NONE, Ordering::SeqCst);
                }
                true
            }
            (SRC_NATIVE, _) => cur == SRC_NATIVE,
            (_, _) if cur == SRC_NATIVE => false,
            (_, TransportEvent::MediaConnected) => {
                if cur != SRC_WEBRTC {
                    tracing::info!("[app] webrtc source took the session");
                    self.active.store(SRC_WEBRTC, Ordering::SeqCst);
                }
                true
            }
            (_, TransportEvent::PhoneLeft) => {
                if cur == SRC_WEBRTC {
                    self.active.store(SRC_NONE, Ordering::SeqCst);
                }
                true
            }
            _ => true,
        }
    }

    /// Send a control message to the source that owns the session (both while none does).
    fn send_active(&self, g: &Inner, ctl: Ctl) {
        let cur = self.active.load(Ordering::SeqCst);
        if cur != SRC_NATIVE {
            if let Some(h) = &g.recv {
                h.send_ctl(ctl.clone());
            }
        }
        if cur != SRC_WEBRTC {
            if let Some(h) = &g.native {
                h.send_ctl(ctl);
            }
        }
    }

    fn handle_event(&self, ev: TransportEvent) {
        match ev {
            TransportEvent::AccessUnit { data, keyframe, rotation, .. } => {
                let cvo = rotation.map_or(-1, i32::from);
                if self.cvo.swap(cvo, Ordering::Relaxed) != cvo {
                    self.apply_auto_rotation();
                }
                self.media.push_au_arc(Arc::from(data), keyframe);
            }
            ev => {
                if let TransportEvent::Vcam(f) = &ev {
                    self.media.set_frame(*f);
                }
                let mut g = self.lock();
                let first_caps = g.view.on_event(&ev);
                match &ev {
                    TransportEvent::Ctl(Ctl::Caps { current, .. }) => {
                        self.rot_hint.store(u32::from(g.view.rotation_hint), Ordering::Relaxed);
                        self.apply_auto_rotation();
                        self.sync_after_caps(&mut g, current, first_caps);
                    }
                    TransportEvent::PhoneLeft => {
                        self.cvo.store(-1, Ordering::Relaxed);
                        self.rot_hint.store(0, Ordering::Relaxed);
                        self.apply_auto_rotation();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Automatic rotation = the stream's RTP orientation when it carries one, else the phone's hint.
    fn auto_rotation(&self) -> u16 {
        match self.cvo.load(Ordering::Relaxed) {
            c if c >= 0 => c as u16,
            _ => self.rot_hint.load(Ordering::Relaxed) as u16,
        }
    }

    fn apply_auto_rotation(&self) {
        let r = self.auto_rotation();
        tracing::info!(
            "[app] automatic rotation now {r} deg (stream orientation {}, phone hint {})",
            self.cvo.load(Ordering::Relaxed),
            self.rot_hint.load(Ordering::Relaxed)
        );
        self.media.set_auto_rotation(r);
    }

    /// Phone reported its capabilities. First time in a session: push our persisted settings.
    /// Later (lens switch): the phone is the truth for the lens, keep zoom inside the new range.
    fn sync_after_caps(&self, g: &mut Inner, current: &str, first: bool) {
        let range = g.view.zoom_range();
        let known = g.view.lenses.iter().any(|l| Some(&l.id) == g.settings.lens.as_ref());
        if let Some((lo, hi)) = range {
            g.settings.zoom = g.settings.zoom.clamp(lo, hi);
        }
        if first {
            {
                let mut ctl = g.settings.ctl_set(Diff::default(), true);
                let phone_size = (g.view.phone_width, g.view.phone_height);
                if let Ctl::Set { lens, profile, .. } = &mut ctl {
                    if !known || lens.as_deref() == Some(current) {
                        *lens = None;
                    }
                    // The phone already delivers this profile's size: do not make it re-acquire
                    // the camera (which stalls the video for seconds on iOS) for nothing.
                    if let Some(p) = *profile {
                        let m = p.mode();
                        if phone_size == (m.width, m.height) || phone_size == (m.height, m.width) {
                            *profile = None;
                        }
                    }
                }
                tracing::info!("[app] first caps of the session: pushing saved settings {ctl:?}");
                self.send_active(g, ctl);
            }
        }
        if g.settings.lens.as_deref() != Some(current) && (!first || !known) {
            g.settings.lens = Some(current.to_string());
        }
        settings::save_settings(&self.dir, &g.settings);
    }

    fn snapshot(&self) -> AppState {
        let stats = self.media.stats();
        let auto_rotation = self.auto_rotation();
        let g = self.lock();
        let v = &g.view;
        let (update_version, update_progress) = updater::snapshot();
        AppState {
            phone_url: g.phone_url.clone(),
            qr_svg: g.qr_svg.clone(),
            conn: v.conn,
            pause_reason: if v.conn == state::ConnUi::Paused { v.pause_reason } else { None },
            path: v.path,
            rtt_ms: v.rtt_ms,
            battery: v.battery,
            charging: v.charging,
            decoder_name: (stats.decoder_name != "none").then_some(stats.decoder_name),
            fps: stats.fps.round() as u32,
            width: VCAM_SIZE.0,
            height: VCAM_SIZE.1,
            src_width: stats.w,
            src_height: stats.h,
            src_fps: stats.fps.round() as u32,
            phone_width: v.phone_width,
            phone_height: v.phone_height,
            phone_fps: v.phone_fps.round() as u32,
            auto_rotation,
            lenses: v.lenses.clone(),
            zoom_min: v.zoom_min,
            zoom_max: v.zoom_max,
            torch_available: v.torch_available,
            settings: g.settings.clone(),
            update_version,
            update_progress,
        }
    }

    fn set_settings(&self, patch: SettingsPatch) -> AppState {
        {
            let mut g = self.lock();
            let range = g.view.zoom_range();
            let d = g.settings.apply(patch, range);
            settings::save_settings(&self.dir, &g.settings);
            if d.mirror {
                self.media.set_mirror(g.settings.mirror);
            }
            if d.decoder {
                self.media.set_decoder(g.settings.decoder.pref());
            }
            if d.rotation {
                tracing::info!("[app] manual rotation set to {} deg", g.settings.rotation);
                self.media.set_rotation(g.settings.rotation);
            }
            if d.any_phone() {
                self.send_active(&g, g.settings.ctl_set(d, false));
            }
        }
        self.snapshot()
    }

    fn new_pairing(self: &Arc<Self>) -> AppState {
        {
            let mut g = self.lock();
            g.pairing = Pairing::generate();
            settings::save_pairing(&self.dir, &g.pairing);
            self.restart_receiver(&mut g);
        }
        self.snapshot()
    }

    fn preview(&self) -> Option<String> {
        let jpeg = self.media.preview_jpeg(640)?;
        Some(base64::engine::general_purpose::STANDARD.encode(jpeg))
    }

    /// The decoder asked for a keyframe (closed after a stall, or lost sync): send a PLI.
    fn poll_keyframe_request(&self, last: &mut std::time::Instant) {
        if self.media.take_keyframe_request() && last.elapsed() >= Duration::from_secs(1) {
            *last = std::time::Instant::now();
            let g = self.lock();
            let cur = self.active.load(Ordering::SeqCst);
            if cur != SRC_NATIVE {
                if let Some(h) = &g.recv {
                    h.request_keyframe();
                }
            }
            if cur != SRC_WEBRTC {
                if let Some(h) = &g.native {
                    h.request_keyframe();
                }
            }
        }
    }

    fn shutdown(&self) {
        tracing::info!("[app] shutting down");
        let mut g = self.lock();
        if let Some(h) = g.recv.take() {
            h.shutdown();
        }
        if let Some(h) = g.native.take() {
            h.shutdown();
        }
        drop(g);
        self.media.shutdown();
    }
}

#[tauri::command(async)]
fn get_state(s: State<'_, Arc<Shared>>) -> AppState {
    s.snapshot()
}

#[tauri::command(async)]
fn set_settings(s: State<'_, Arc<Shared>>, patch: SettingsPatch) -> AppState {
    s.set_settings(patch)
}

#[tauri::command(async)]
fn new_pairing(s: State<'_, Arc<Shared>>) -> AppState {
    s.inner().new_pairing()
}

#[tauri::command(async)]
fn get_preview(s: State<'_, Arc<Shared>>) -> Option<String> {
    s.preview()
}

/// Show the window, recreating the webview if it was destroyed.
fn open_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::default())
        .title("PhoneCam")
        .inner_size(1000.0, 720.0)
        .min_inner_size(900.0, 640.0)
        .build();
    match built {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => tracing::error!("could not create window: {e}"),
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = updater::tray_menu(app, None)?;
    let mut tray = TrayIconBuilder::with_id("tray")
        .tooltip("PhoneCam")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => open_window(app),
            "install_update" => updater::spawn_install(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                open_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Push `state` to the webview whenever it changed; polling at 4 Hz is the throttle.
fn spawn_state_emitter(app: AppHandle, shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("phonecam-state".into())
        .spawn(move || {
            let mut last: Option<AppState> = None;
            let mut last_pli = std::time::Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(250));
                shared.poll_keyframe_request(&mut last_pli);
                let now = shared.snapshot();
                if last.as_ref() != Some(&now) {
                    let _ = app.emit("state", &now);
                    last = Some(now);
                }
            }
        })
        .expect("spawn state thread");
}

pub fn run() {
    logging::init(&settings::data_dir());
    let shared = Shared::new();
    let for_exit = Arc::clone(&shared);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_window(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::clone(&shared))
        .invoke_handler(tauri::generate_handler![get_state, set_settings, new_pairing, get_preview, updater::install_update])
        .on_window_event(|window, event| {
            // Minimize == hide to tray: destroy the webview (frees its RAM). Close already destroys it.
            if let WindowEvent::Resized(_) = event {
                if window.is_minimized().unwrap_or(false) {
                    let _ = window.destroy();
                }
            }
        })
        .setup(move |app| {
            setup_tray(app.handle())?;
            spawn_state_emitter(app.handle().clone(), Arc::clone(&shared));
            updater::spawn(app.handle().clone());
            open_window(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building PhoneCam")
        .run(move |_app, event| match event {
            // Last window closed: keep running in the tray. Only an explicit exit(code) quits.
            RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
            RunEvent::Exit => for_exit.shutdown(),
            _ => {}
        });
}
