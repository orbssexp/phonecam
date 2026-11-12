//! PhoneCam Windows media engine: hardware decode, DirectShow softcam, letterboxing,
//! preview generation, and placeholder rendering.

pub mod ffi;
pub mod font;
pub mod letterbox;
pub mod placeholder;
pub mod preview;
pub mod rotate;

use std::ffi::{c_char, c_void, CStr};
use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub use letterbox::{calculate_letterbox, LetterboxRect};
pub use phonecam_core::VcamFrame;
pub use placeholder::render_placeholder;
pub use preview::encode_preview_jpeg;

/// Decoder preference setting (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DecoderPref {
    /// D3D11VA on NVIDIA adapter -> D3D11VA on any GPU -> CPU.
    #[default]
    Auto,
    /// D3D11VA on any GPU adapter -> CPU.
    D3d11va,
    /// Explicit NVIDIA CUDA / NVDEC -> CPU.
    Cuda,
    /// Software decoding only (libavcodec h264 slice threads).
    Cpu,
}

impl FromStr for DecoderPref {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(DecoderPref::Auto),
            "d3d11va" | "d3d11" | "gpu" => Ok(DecoderPref::D3d11va),
            "cuda" | "nvdec" => Ok(DecoderPref::Cuda),
            "cpu" => Ok(DecoderPref::Cpu),
            other => Err(format!("unknown decoder preference: {other}")),
        }
    }
}

impl fmt::Display for DecoderPref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecoderPref::Auto => write!(f, "Auto"),
            DecoderPref::D3d11va => write!(f, "D3D11VA"),
            DecoderPref::Cuda => write!(f, "CUDA"),
            DecoderPref::Cpu => write!(f, "CPU"),
        }
    }
}

/// Statistics reported by the media engine.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaStats {
    pub decoder_name: String,
    pub fps: f32,
    pub w: u32,
    pub h: u32,
}

struct StatsState {
    decoder_name: String,
    w: u32,
    h: u32,
    fps: f32,
    frames_in_interval: u32,
    interval_start: Instant,
    last_frame_at: Option<Instant>,
}

impl StatsState {
    fn new() -> Self {
        Self {
            decoder_name: "none".to_string(),
            w: 0,
            h: 0,
            fps: 0.0,
            frames_in_interval: 0,
            interval_start: Instant::now(),
            last_frame_at: None,
        }
    }

    /// Counts one DECODED frame (called once per frame the decoder returns, nowhere else).
    fn record_frame(&mut self, w: u32, h: u32) {
        let now = Instant::now();
        // After a pause the window would span the silence and under-report: start afresh.
        if self
            .last_frame_at
            .is_none_or(|t| now.duration_since(t) > Duration::from_millis(1500))
        {
            self.interval_start = now;
            self.frames_in_interval = 0;
        }
        self.w = w;
        self.h = h;
        self.last_frame_at = Some(now);
        self.frames_in_interval += 1;

        let elapsed = now.duration_since(self.interval_start);
        if elapsed >= Duration::from_millis(1000) {
            self.fps = self.frames_in_interval as f32 / elapsed.as_secs_f32();
            self.frames_in_interval = 0;
            self.interval_start = now;
        }
    }

    fn snapshot(&self) -> MediaStats {
        let is_stale = self
            .last_frame_at
            .is_none_or(|t| t.elapsed() > Duration::from_millis(1500));
        let current_fps = if is_stale { 0.0 } else { self.fps };

        MediaStats {
            decoder_name: self.decoder_name.clone(),
            fps: current_fps,
            w: self.w,
            h: self.h,
        }
    }
}

struct LatestFrame {
    w: u32,
    h: u32,
    bgr: Vec<u8>,
}

pub struct Au {
    pub data: Arc<[u8]>,
    pub keyframe: bool,
    pub at: Instant,
}

enum MediaMsg {
    Au(Au),
    SetFrame(VcamFrame),
    SetDecoder(DecoderPref),
    Shutdown,
}

struct DecWrapper(*mut c_void);
unsafe impl Send for DecWrapper {}

impl DecWrapper {
    fn new(mode: i32) -> Result<Self, String> {
        let mut err = vec![0 as c_char; 256];
        let p = unsafe { ffi::dec_create(mode, err.as_mut_ptr(), 256) };
        if p.is_null() {
            let msg = unsafe { CStr::from_ptr(err.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            Err(msg)
        } else {
            Ok(DecWrapper(p))
        }
    }

    fn name(&self) -> String {
        if self.0.is_null() {
            "none".to_string()
        } else {
            unsafe { CStr::from_ptr(ffi::dec_name(self.0)) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for DecWrapper {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::dec_destroy(self.0) };
            self.0 = std::ptr::null_mut();
        }
    }
}

struct ScalerWrapper {
    raw: *mut c_void,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
}
unsafe impl Send for ScalerWrapper {}

impl ScalerWrapper {
    fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Self {
        let raw =
            unsafe { ffi::scaler_create(src_w as i32, src_h as i32, dst_w as i32, dst_h as i32) };
        Self {
            raw,
            src_w,
            src_h,
            dst_w,
            dst_h,
        }
    }

    fn scale(&self, src: *const u8, src_stride: i32, dst: *mut u8, dst_stride: i32, h: i32) {
        if !self.raw.is_null() {
            unsafe { ffi::scaler_scale(self.raw, src, src_stride, dst, dst_stride, h) };
        }
    }
}

impl Drop for ScalerWrapper {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { ffi::scaler_destroy(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

struct SoftcamCamera(*mut c_void);
unsafe impl Send for SoftcamCamera {}

impl SoftcamCamera {
    fn new(width: u32, height: u32) -> Self {
        let ptr = unsafe { ffi::scCreateCamera(width as i32, height as i32, 0.0) };
        if ptr.is_null() {
            tracing::warn!("scCreateCamera({width}, {height}) returned null");
        } else {
            tracing::info!("scCreateCamera({width}, {height}) created successfully");
        }
        SoftcamCamera(ptr)
    }

    fn send_frame(&self, bgr_bits: &[u8]) {
        if !self.0.is_null() {
            unsafe { ffi::scSendFrame(self.0, bgr_bits.as_ptr()) };
        }
    }
}

impl Drop for SoftcamCamera {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::scDeleteCamera(self.0) };
            self.0 = std::ptr::null_mut();
        }
    }
}

/// Handle controlling the background media engine.
pub struct MediaHandle {
    tx: Sender<MediaMsg>,
    stats: Arc<Mutex<StatsState>>,
    latest_frame: Arc<Mutex<Option<LatestFrame>>>,
    shutdown_flag: Arc<AtomicBool>,
    mirror: Arc<AtomicBool>,
    rotation: Arc<Rotation>,
    need_keyframe: Arc<AtomicBool>,
    preview: Arc<PreviewDemand>,
    worker_thread: Option<JoinHandle<()>>,
}

/// Rotation applied to the decoded picture: `auto` (from the stream: RTP video-orientation or the
/// phone's hint) plus `manual` (the user's Rotate button), both degrees clockwise.
#[derive(Default)]
struct Rotation {
    auto: std::sync::atomic::AtomicU32,
    manual: std::sync::atomic::AtomicU32,
}

impl Rotation {
    fn total(&self) -> u16 {
        let a = self.auto.load(Ordering::Relaxed) as i32;
        let m = self.manual.load(Ordering::Relaxed) as i32;
        rotate::normalize_deg(a + m)
    }
}

/// Tracks whether anyone is looking at the preview, so the media thread only keeps a copy of the
/// last frame (6 MB at 1080p) while the window is open.
struct PreviewDemand {
    epoch: Instant,
    last_ms: std::sync::atomic::AtomicU64,
}

impl PreviewDemand {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        let ms = self.epoch.elapsed().as_millis() as u64 + 1;
        self.last_ms.store(ms, Ordering::Relaxed);
    }

    fn wanted(&self) -> bool {
        let last = self.last_ms.load(Ordering::Relaxed);
        last != 0
            && (self.epoch.elapsed().as_millis() as u64 + 1).saturating_sub(last)
                < PREVIEW_DEMAND_MS
    }
}

const PREVIEW_DEMAND_MS: u64 = 3000;

impl MediaHandle {
    /// Pushes an encoded H.264 Access Unit (AU) slice to the decoder.
    pub fn push_au(&self, data: &[u8], keyframe: bool) {
        self.push_au_arc(Arc::from(data), keyframe);
    }

    /// Pushes an encoded H.264 Access Unit (AU) Arc slice to the decoder without reallocation.
    pub fn push_au_arc(&self, data: Arc<[u8]>, keyframe: bool) {
        let _ = self.tx.send(MediaMsg::Au(Au {
            data,
            keyframe,
            at: Instant::now(),
        }));
    }

    /// Updates what the "PhoneCam" virtual camera outputs (spec §7.3).
    pub fn set_frame(&self, frame: VcamFrame) {
        let _ = self.tx.send(MediaMsg::SetFrame(frame));
    }

    /// Horizontally flips the camera output (applied after letterboxing). Takes effect on the next frame.
    pub fn set_mirror(&self, on: bool) {
        self.mirror.store(on, Ordering::Relaxed);
    }

    /// User rotation in degrees clockwise (0/90/180/270), applied on top of the automatic one,
    /// before letterboxing. Takes effect on the next frame.
    pub fn set_rotation(&self, deg: u16) {
        self.rotation.manual.store(rotate::normalize_deg(deg as i32) as u32, Ordering::Relaxed);
    }

    /// Automatic rotation (RTP video-orientation, or the phone's hint) in degrees clockwise.
    pub fn set_auto_rotation(&self, deg: u16) {
        self.rotation.auto.store(rotate::normalize_deg(deg as i32) as u32, Ordering::Relaxed);
    }

    /// True (once) when the decoder is closed or broken and needs a keyframe to continue; the
    /// caller should send a PLI.
    pub fn take_keyframe_request(&self) -> bool {
        self.need_keyframe.swap(false, Ordering::Relaxed)
    }

    /// Switches the decoder preference at runtime. The softcam device is kept; only the
    /// decoder is torn down and lazily re-opened on the next AU.
    pub fn set_decoder(&self, pref: DecoderPref) {
        let _ = self.tx.send(MediaMsg::SetDecoder(pref));
    }

    /// Returns the latest frame encoded as JPEG, downscaled to `max_w` if wider.
    ///
    /// Returns `None` if no frame is available yet or encoding fails.
    pub fn preview_jpeg(&self, max_w: u32) -> Option<Vec<u8>> {
        self.preview.touch();
        let guard = self.latest_frame.lock().ok()?;
        let frame = guard.as_ref()?;
        encode_preview_jpeg(&frame.bgr, frame.w, frame.h, max_w)
    }

    /// Returns snapshot stats including active decoder name, FPS, and stream resolution.
    pub fn stats(&self) -> MediaStats {
        self.stats
            .lock()
            .map(|s| s.snapshot())
            .unwrap_or_else(|_| MediaStats {
                decoder_name: "error".to_string(),
                fps: 0.0,
                w: 0,
                h: 0,
            })
    }

    /// Shuts down the media thread.
    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        let _ = self.tx.send(MediaMsg::Shutdown);
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
    }
}

pub struct Media;

impl Media {
    /// Starts the media subsystem with fixed output dimensions `out = (w, h)`.
    ///
    /// Softcam is created once at `out` and never recreated. Incoming frames are
    /// scaled to fit `out` while preserving aspect ratio with letterbox bars.
    pub fn start(decoder: DecoderPref, out: (u32, u32)) -> MediaHandle {
        let (out_w, out_h) = (out.0.max(16), out.1.max(16));
        let (tx, rx) = mpsc::channel::<MediaMsg>();
        let stats = Arc::new(Mutex::new(StatsState::new()));
        let latest_frame = Arc::new(Mutex::new(None));
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        let stats_clone = Arc::clone(&stats);
        let latest_clone = Arc::clone(&latest_frame);
        let shutdown_clone = Arc::clone(&shutdown_flag);
        let mirror = Arc::new(AtomicBool::new(false));
        let mirror_clone = Arc::clone(&mirror);
        let rotation = Arc::new(Rotation::default());
        let rotation_clone = Arc::clone(&rotation);
        let need_keyframe = Arc::new(AtomicBool::new(false));
        let need_keyframe_clone = Arc::clone(&need_keyframe);
        let preview = Arc::new(PreviewDemand::new());
        let preview_clone = Arc::clone(&preview);

        let worker_thread = thread::Builder::new()
            .name("phonecam-media".to_string())
            .spawn(move || {
                run_media_loop(
                    rx,
                    decoder,
                    out_w,
                    out_h,
                    stats_clone,
                    latest_clone,
                    shutdown_clone,
                    mirror_clone,
                    rotation_clone,
                    need_keyframe_clone,
                    preview_clone,
                );
            })
            .expect("failed to spawn media thread");

        MediaHandle {
            tx,
            stats,
            latest_frame,
            shutdown_flag,
            mirror,
            rotation,
            need_keyframe,
            preview,
            worker_thread: Some(worker_thread),
        }
    }
}

/// Access units waiting for the decoder beyond which the backlog is dropped (about 100 ms of 30 fps).
const MAX_BACKLOG: usize = 3;

/// Per-window (10 s) pipeline measurements for the `[media]` line.
#[derive(Default)]
struct StageWindow {
    frames: u64,
    queue_ms: f64,
    queue_max_ms: f64,
    decode_ms: f64,
    download_ms: f64,
    convert_ms: f64,
    rotate_ms: f64,
    scale_ms: f64,
    total_ms: f64,
    skipped: u64,
}

/// Drops the access units that would only play old video. With a keyframe in the queue everything
/// before the newest one goes (decoding resumes there); with none, every access unit goes and
/// `await_key` is set so deltas are skipped until a keyframe arrives. Other messages are kept in
/// order. Returns how many access units were dropped.
fn trim_backlog(q: &mut VecDeque<MediaMsg>, await_key: &mut bool) -> usize {
    let last_key = q.iter().rposition(|m| matches!(m, MediaMsg::Au(a) if a.keyframe));
    let mut dropped = 0;
    let kept: VecDeque<MediaMsg> = q
        .drain(..)
        .enumerate()
        .filter(|(i, m)| {
            let stale = matches!(m, MediaMsg::Au(_)) && last_key.is_none_or(|k| *i < k);
            if stale {
                dropped += 1;
            }
            !stale
        })
        .map(|(_, m)| m)
        .collect();
    *q = kept;
    if last_key.is_none() {
        if dropped > 0 {
            *await_key = true;
        }
    } else {
        *await_key = false;
    }
    dropped
}
/// No access unit for this long: drop the decoder (and its GPU surfaces / driver state).
const DEC_IDLE_TEARDOWN: Duration = Duration::from_secs(3);

fn decoder_chain(pref: DecoderPref) -> Vec<i32> {
    match pref {
        DecoderPref::Auto | DecoderPref::D3d11va => vec![1, 3, 0],
        DecoderPref::Cuda => vec![2, 0],
        DecoderPref::Cpu => vec![0],
    }
}

fn update_latest_frame(
    latest_frame: &Arc<Mutex<Option<LatestFrame>>>,
    preview: &PreviewDemand,
    w: u32,
    h: u32,
    bgr: &[u8],
) {
    if let Ok(mut guard) = latest_frame.lock() {
        if !preview.wanted() {
            // Nobody is looking (tray mode): do not keep a frame copy around.
            *guard = None;
            return;
        }
        if let Some(existing) = guard.as_mut() {
            if existing.w == w && existing.h == h && existing.bgr.len() == bgr.len() {
                existing.bgr.copy_from_slice(bgr);
                return;
            }
        }
        *guard = Some(LatestFrame {
            w,
            h,
            bgr: bgr.to_vec(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn run_media_loop(
    rx: Receiver<MediaMsg>,
    decoder_pref: DecoderPref,
    out_w: u32,
    out_h: u32,
    stats: Arc<Mutex<StatsState>>,
    latest_frame: Arc<Mutex<Option<LatestFrame>>>,
    shutdown_flag: Arc<AtomicBool>,
    mirror: Arc<AtomicBool>,
    rotation: Arc<Rotation>,
    need_keyframe: Arc<AtomicBool>,
    preview: Arc<PreviewDemand>,
) {
    let mut decoder_pref = decoder_pref;
    let cam = SoftcamCamera::new(out_w, out_h);

    // Decoded frames are downscaled inside the decoder's colour conversion to fit this square box
    // (aspect kept, never upscaled), so no 4K BGR frame ever exists. A square box is enough for
    // both landscape and portrait streams, before or after the 90 degree rotation.
    let fit = out_w.max(out_h) as i32;
    let dec_buf_bytes = fit as usize * fit as usize * 3;
    let mut pending: VecDeque<MediaMsg> = VecDeque::new();
    let mut await_key = false;
    let mut win = StageWindow::default();

    let mut out_buf = vec![0u8; (out_w * out_h * 3) as usize];
    // Allocated only while a decoder is open (a 4K BGR buffer is 25 MB of commit).
    let mut dec_buf: Vec<u8> = Vec::new();
    let mut last_au_at = Instant::now();
    // Once a frame matches the output size exactly, decode straight into `out_buf` and drop
    // `dec_buf` (saves a 6 MB copy at 1080p).
    let mut direct_ok = false;

    let mut current_vcam_frame = VcamFrame::Placeholder;

    // Initial placeholder frame
    placeholder::render_placeholder(current_vcam_frame, out_w, out_h, &mut out_buf);
    cam.send_frame(&out_buf);
    update_latest_frame(&latest_frame, &preview, out_w, out_h, &out_buf);

    let mut dec: Option<DecWrapper> = None;
    let mut chain = decoder_chain(decoder_pref);
    let mut ci = 0;
    let mut scaler: Option<ScalerWrapper> = None;
    let mut decode_errors = 0;
    let mut total_frames_decoded = 0u64;
    let mut frames_window = 0u64;
    let mut last_res = (0u32, 0u32);
    let mut rot_buf: Vec<u8> = Vec::new();
    let mut last_log = Instant::now();

    let mut last_tick = Instant::now();
    let tick_interval = Duration::from_millis(200); // 5 fps placeholder interval

    while !shutdown_flag.load(Ordering::Relaxed) {
        let elapsed = last_tick.elapsed();
        let timeout = if elapsed >= tick_interval {
            Duration::from_millis(0)
        } else {
            tick_interval - elapsed
        };

        if pending.is_empty() {
            match rx.recv_timeout(timeout) {
                Ok(m) => pending.push_back(m),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(m) = rx.try_recv() {
            pending.push_back(m);
        }
        // The decoder fell behind: keep only the newest picture instead of playing old video late.
        if pending.iter().filter(|m| matches!(m, MediaMsg::Au(_))).count() > MAX_BACKLOG {
            let dropped = trim_backlog(&mut pending, &mut await_key);
            win.skipped += dropped as u64;
            if await_key {
                need_keyframe.store(true, Ordering::Relaxed);
            }
        }
        let msg = pending.pop_front();

        match msg {
            Some(MediaMsg::Shutdown) => break,
            Some(MediaMsg::SetDecoder(pref)) => {
                if pref != decoder_pref {
                    decoder_pref = pref;
                    chain = decoder_chain(pref);
                    ci = 0;
                    dec = None;
                    dec_buf = Vec::new();
                    rot_buf = Vec::new();
                    last_res = (0, 0);
                    direct_ok = false;
                    scaler = None;
                    decode_errors = 0;
                    total_frames_decoded = 0;
                    await_key = false;
                }
            }
            Some(MediaMsg::SetFrame(new_frame)) => {
                if current_vcam_frame != new_frame {
                    current_vcam_frame = new_frame;
                    if current_vcam_frame != VcamFrame::Camera {
                        placeholder::render_placeholder(
                            current_vcam_frame,
                            out_w,
                            out_h,
                            &mut out_buf,
                        );
                        cam.send_frame(&out_buf);
                        update_latest_frame(&latest_frame, &preview, out_w, out_h, &out_buf);
                        last_tick = Instant::now();
                    }
                }
            }
            Some(MediaMsg::Au(au)) => {
                last_au_at = Instant::now();
                let au_start = Instant::now();
                let queue_ms = au_start.saturating_duration_since(au.at).as_secs_f64() * 1000.0;
                if await_key {
                    if !au.keyframe {
                        need_keyframe.store(true, Ordering::Relaxed);
                        win.skipped += 1;
                        continue;
                    }
                    await_key = false;
                }
                // The decoder is torn down when the stream stops; reopen only on a keyframe so the
                // hardware-fallback logic never mistakes missing reference frames for a broken decoder.
                if dec.is_none() && !au.keyframe {
                    // Nothing to decode with: ask the transport for a keyframe (PLI).
                    need_keyframe.store(true, Ordering::Relaxed);
                    continue;
                }
                // Initialize decoder lazily
                if dec.is_none() && ci < chain.len() {
                    match DecWrapper::new(chain[ci]) {
                        Ok(d) => {
                            let name = d.name();
                            tracing::info!("Opened decoder: {name}");
                            if let Ok(mut s) = stats.lock() {
                                s.decoder_name = name;
                            }
                            dec = Some(d);
                        }
                        Err(e) => {
                            tracing::warn!("Decoder init failed (mode {}): {e}", chain[ci]);
                            ci += 1;
                            if ci < chain.len() {
                                continue;
                            }
                        }
                    }
                }

                if let Some(d) = dec.as_ref() {
                    let mut w = 0i32;
                    let mut h = 0i32;
                    let mut hw = 0i32;

                    let rot = rotation.total();
                    let direct = direct_ok && current_vcam_frame == VcamFrame::Camera && rot == 0;
                    if !direct && dec_buf.is_empty() {
                        dec_buf = vec![0u8; dec_buf_bytes];
                    }
                    let (dst_ptr, dst_len) = if direct {
                        (out_buf.as_mut_ptr(), out_buf.len())
                    } else {
                        (dec_buf.as_mut_ptr(), dec_buf.len())
                    };

                    let mut t_dec = [0f64; 3];
                    let n = unsafe {
                        ffi::dec_decode(
                            d.0,
                            au.data.as_ptr(),
                            au.data.len() as i32,
                            dst_ptr,
                            dst_len as i32,
                            fit,
                            fit,
                            t_dec.as_mut_ptr(),
                            &mut w,
                            &mut h,
                            &mut hw,
                        )
                    };

                    if n < 0 {
                        direct_ok = false;
                        decode_errors += 1;
                        if decode_errors >= 10 && total_frames_decoded == 0 && ci + 1 < chain.len()
                        {
                            tracing::warn!("Hardware decode produced no frames, falling back");
                            dec = None;
                            dec_buf = Vec::new();
                            direct_ok = false;
                            ci += 1;
                            decode_errors = 0;
                        } else if decode_errors >= 30 && total_frames_decoded > 0 {
                            // Was working, now every AU fails (e.g. a resolution switch the decoder
                            // could not follow): reopen on the next keyframe and ask for one.
                            tracing::warn!("[media] {decode_errors} decode errors in a row; reopening decoder");
                            dec = None;
                            dec_buf = Vec::new();
                            scaler = None;
                            last_res = (0, 0);
                            direct_ok = false;
                            decode_errors = 0;
                            total_frames_decoded = 0;
                            need_keyframe.store(true, Ordering::Relaxed);
                        }
                    } else if n > 0 && w > 0 && h > 0 {
                        decode_errors = 0;
                        win.decode_ms += t_dec[0];
                        win.download_ms += t_dec[1];
                        win.convert_ms += t_dec[2];
                        win.queue_ms += queue_ms;
                        win.queue_max_ms = win.queue_max_ms.max(queue_ms);
                        win.frames += 1;
                        total_frames_decoded += n as u64;
                        frames_window += n as u64;

                        if let Ok(mut s) = stats.lock() {
                            s.record_frame(w as u32, h as u32);
                        }
                        if last_res != (w as u32, h as u32) {
                            tracing::info!(
                                "[media] decoded stream {}x{} (was {}x{}), rotation {rot} deg, decoder {}",
                                w, h, last_res.0, last_res.1, d.name()
                            );
                            last_res = (w as u32, h as u32);
                        }

                        // If VcamFrame is Camera, scale and send to softcam!
                        if direct && (w as u32, h as u32) != (out_w, out_h) {
                            // Resolution changed under us: the frame landed in out_buf's prefix.
                            direct_ok = false;
                            let bytes = (w as usize * h as usize * 3).min(out_buf.len());
                            dec_buf = vec![0u8; dec_buf_bytes];
                            dec_buf[..bytes].copy_from_slice(&out_buf[..bytes]);
                        }
                        if direct && (w as u32, h as u32) == (out_w, out_h) {
                            if mirror.load(Ordering::Relaxed) {
                                letterbox::mirror_bgr_in_place(&mut out_buf, out_w, out_h);
                            }
                            cam.send_frame(&out_buf);
                            update_latest_frame(&latest_frame, &preview, out_w, out_h, &out_buf);
                            last_tick = Instant::now();
                            win.total_ms += au_start.elapsed().as_secs_f64() * 1000.0;
                        } else if current_vcam_frame == VcamFrame::Camera {
                            // Rotate (auto + manual) before letterboxing.
                            let need = w as usize * h as usize * 3;
                            let t_rot = Instant::now();
                            let (sw, sh) = if rot == 0 {
                                rot_buf = Vec::new();
                                (w as u32, h as u32)
                            } else {
                                if rot_buf.len() < need {
                                    rot_buf.resize(need, 0);
                                }
                                rotate::rotate_bgr(
                                    &dec_buf[..need],
                                    w as u32,
                                    h as u32,
                                    rot,
                                    &mut rot_buf[..need],
                                )
                            };
                            let src: &[u8] = if rot == 0 { &dec_buf } else { &rot_buf };
                            win.rotate_ms += t_rot.elapsed().as_secs_f64() * 1000.0;
                            let t_scale = Instant::now();

                            let rect = calculate_letterbox(sw, sh, out_w, out_h);

                            if rect.scaled_w != out_w || rect.scaled_h != out_h {
                                out_buf.fill(0);
                            }

                            let need_new_scaler = match &scaler {
                                Some(s) => {
                                    s.src_w != sw
                                        || s.src_h != sh
                                        || s.dst_w != rect.scaled_w
                                        || s.dst_h != rect.scaled_h
                                }
                                None => true,
                            };
                            if need_new_scaler {
                                scaler = Some(ScalerWrapper::new(
                                    sw,
                                    sh,
                                    rect.scaled_w,
                                    rect.scaled_h,
                                ));
                            }

                            if let Some(s) = &scaler {
                                let dst_offset =
                                    ((rect.offset_y * out_w + rect.offset_x) * 3) as usize;
                                let dst_ptr = unsafe { out_buf.as_mut_ptr().add(dst_offset) };
                                let dst_stride = (out_w * 3) as i32;
                                s.scale(src.as_ptr(), (sw * 3) as i32, dst_ptr, dst_stride, sh as i32);
                            }

                            win.scale_ms += t_scale.elapsed().as_secs_f64() * 1000.0;
                            if mirror.load(Ordering::Relaxed) {
                                letterbox::mirror_bgr_in_place(&mut out_buf, out_w, out_h);
                            }
                            cam.send_frame(&out_buf);
                            update_latest_frame(&latest_frame, &preview, out_w, out_h, &out_buf);
                            last_tick = Instant::now();
                            win.total_ms += au_start.elapsed().as_secs_f64() * 1000.0;
                            if rot == 0 && (w as u32, h as u32) == (out_w, out_h) {
                                direct_ok = true;
                                dec_buf = Vec::new();
                            }
                        }
                    }
                }
            }
            None => {
                // Timeout elapsed (no AU arrived or not streaming)
                if dec.is_some() && last_au_at.elapsed() >= DEC_IDLE_TEARDOWN {
                    tracing::info!("[media] stream idle for 3 s, releasing decoder");
                    dec = None;
                    dec_buf = Vec::new();
                    rot_buf = Vec::new();
                    last_res = (0, 0);
                    direct_ok = false;
                    scaler = None;
                    decode_errors = 0;
                    total_frames_decoded = 0;
                    unsafe { ffi::dec_trim_memory() };
                    await_key = false;
                }
                if current_vcam_frame != VcamFrame::Camera || total_frames_decoded == 0 {
                    placeholder::render_placeholder(current_vcam_frame, out_w, out_h, &mut out_buf);
                    cam.send_frame(&out_buf);
                    update_latest_frame(&latest_frame, &preview, out_w, out_h, &out_buf);
                    last_tick = Instant::now();
                }
            }
        }

        if last_log.elapsed() >= Duration::from_secs(10) {
            let secs = last_log.elapsed().as_secs_f32();
            if dec.is_some() || frames_window > 0 {
                tracing::info!(
                    "[media] last {:.0}s: decoded {} frames ({:.1}/s), stream {}x{}, rotation {} deg, decoder {}",
                    secs,
                    frames_window,
                    frames_window as f32 / secs,
                    last_res.0,
                    last_res.1,
                    rotation.total(),
                    dec.as_ref().map(|d| d.name()).unwrap_or_else(|| "closed".into()),
                );
            }
            if win.frames > 0 || win.skipped > 0 {
                let n = win.frames.max(1) as f64;
                tracing::info!(
                    "[media] stages avg ms/frame: queue {:.1} (max {:.1}), decode {:.1}, download {:.1}, convert+downscale {:.1}, rotate {:.1}, letterbox {:.1}, total {:.1}; {} AUs skipped (backlog > {MAX_BACKLOG})",
                    win.queue_ms / n,
                    win.queue_max_ms,
                    win.decode_ms / n,
                    win.download_ms / n,
                    win.convert_ms / n,
                    win.rotate_ms / n,
                    win.scale_ms / n,
                    win.total_ms / n,
                    win.skipped,
                );
            }
            win = StageWindow::default();
            frames_window = 0;
            last_log = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn au(keyframe: bool) -> MediaMsg {
        MediaMsg::Au(Au { data: Arc::from(&[0u8][..]), keyframe, at: Instant::now() })
    }

    #[test]
    fn backlog_trim_keeps_from_newest_keyframe() {
        let mut q: VecDeque<MediaMsg> = [au(true), au(false), au(true), au(false), au(false)].into();
        let mut await_key = true;
        assert_eq!(trim_backlog(&mut q, &mut await_key), 2);
        assert_eq!(q.len(), 3);
        assert!(matches!(&q[0], MediaMsg::Au(a) if a.keyframe));
        assert!(!await_key);
    }

    #[test]
    fn backlog_trim_without_keyframe_drops_all_and_waits() {
        let mut q: VecDeque<MediaMsg> = [au(false), MediaMsg::SetFrame(VcamFrame::Camera), au(false), au(false)].into();
        let mut await_key = false;
        assert_eq!(trim_backlog(&mut q, &mut await_key), 3);
        assert_eq!(q.len(), 1);
        assert!(matches!(q[0], MediaMsg::SetFrame(_)));
        assert!(await_key);
    }

    #[test]
    fn test_decoder_pref_display_and_parse() {
        assert_eq!(DecoderPref::from_str("auto").unwrap(), DecoderPref::Auto);
        assert_eq!(
            DecoderPref::from_str("d3d11va").unwrap(),
            DecoderPref::D3d11va
        );
        assert_eq!(DecoderPref::from_str("cuda").unwrap(), DecoderPref::Cuda);
        assert_eq!(DecoderPref::from_str("cpu").unwrap(), DecoderPref::Cpu);
        assert!(DecoderPref::from_str("unknown").is_err());

        assert_eq!(DecoderPref::Auto.to_string(), "Auto");
    }

    #[test]
    fn test_media_lifecycle_and_preview() {
        let handle = Media::start(DecoderPref::Cpu, (640, 360));
        let stats = handle.stats();
        assert_eq!(stats.w, 0);
        assert_eq!(stats.h, 0);

        // The frame copy is only kept while someone asks for the preview: the first call registers
        // interest, the placeholder tick (200 ms) then fills it.
        assert!(handle.preview_jpeg(320).is_none());
        thread::sleep(Duration::from_millis(500));
        let preview = handle.preview_jpeg(320);
        assert!(
            preview.is_some(),
            "Preview JPEG should be produced from initial placeholder"
        );
        let bytes = preview.unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(bytes[0], 0xFF);
        assert_eq!(bytes[1], 0xD8);

        // Switch frame to Paused
        handle.set_frame(VcamFrame::Paused);
        thread::sleep(Duration::from_millis(50));
        let paused_preview = handle.preview_jpeg(320);
        assert!(paused_preview.is_some());

        // Push empty AU (won't decode, but shouldn't panic)
        handle.push_au(&[0x00, 0x00, 0x00, 0x01, 0x09, 0x10], false);

        handle.shutdown();
    }
}
