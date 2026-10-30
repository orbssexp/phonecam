//! PhoneCam Windows media engine: hardware decode, DirectShow softcam, letterboxing,
//! preview generation, and placeholder rendering.

pub mod ffi;
pub mod font;
pub mod letterbox;
pub mod placeholder;
pub mod preview;

use std::ffi::{c_char, c_void, CStr};
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

    fn record_frame(&mut self, w: u32, h: u32) {
        let now = Instant::now();
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
        let raw = unsafe {
            ffi::scaler_create(src_w as i32, src_h as i32, dst_w as i32, dst_h as i32)
        };
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
    worker_thread: Option<JoinHandle<()>>,
}

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

    /// Returns the latest frame encoded as JPEG, downscaled to `max_w` if wider.
    ///
    /// Returns `None` if no frame is available yet or encoding fails.
    pub fn preview_jpeg(&self, max_w: u32) -> Option<Vec<u8>> {
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
                );
            })
            .expect("failed to spawn media thread");

        MediaHandle {
            tx,
            stats,
            latest_frame,
            shutdown_flag,
            worker_thread: Some(worker_thread),
        }
    }
}

fn update_latest_frame(
    latest_frame: &Arc<Mutex<Option<LatestFrame>>>,
    w: u32,
    h: u32,
    bgr: &[u8],
) {
    if let Ok(mut guard) = latest_frame.lock() {
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

fn run_media_loop(
    rx: Receiver<MediaMsg>,
    decoder_pref: DecoderPref,
    out_w: u32,
    out_h: u32,
    stats: Arc<Mutex<StatsState>>,
    latest_frame: Arc<Mutex<Option<LatestFrame>>>,
    shutdown_flag: Arc<AtomicBool>,
) {
    let cam = SoftcamCamera::new(out_w, out_h);

    let mut out_buf = vec![0u8; (out_w * out_h * 3) as usize];
    let mut dec_buf = vec![0u8; (3840 * 2160 * 3) as usize];

    let mut current_vcam_frame = VcamFrame::Placeholder;

    // Initial placeholder frame
    placeholder::render_placeholder(current_vcam_frame, out_w, out_h, &mut out_buf);
    cam.send_frame(&out_buf);
    update_latest_frame(&latest_frame, out_w, out_h, &out_buf);

    let mut dec: Option<DecWrapper> = None;
    let chain: Vec<i32> = match decoder_pref {
        DecoderPref::Auto => vec![1, 3, 0],
        DecoderPref::D3d11va => vec![1, 3, 0],
        DecoderPref::Cuda => vec![2, 0],
        DecoderPref::Cpu => vec![0],
    };
    let mut ci = 0;
    let mut scaler: Option<ScalerWrapper> = None;
    let mut decode_errors = 0;
    let mut total_frames_decoded = 0u64;

    let mut last_tick = Instant::now();
    let tick_interval = Duration::from_millis(200); // 5 fps placeholder interval

    while !shutdown_flag.load(Ordering::Relaxed) {
        let elapsed = last_tick.elapsed();
        let timeout = if elapsed >= tick_interval {
            Duration::from_millis(0)
        } else {
            tick_interval - elapsed
        };

        let msg = match rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        match msg {
            Some(MediaMsg::Shutdown) => break,
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
                        update_latest_frame(&latest_frame, out_w, out_h, &out_buf);
                        last_tick = Instant::now();
                    }
                }
            }
            Some(MediaMsg::Au(au)) => {
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

                    let n = unsafe {
                        ffi::dec_decode(
                            d.0,
                            au.data.as_ptr(),
                            au.data.len() as i32,
                            dec_buf.as_mut_ptr(),
                            dec_buf.len() as i32,
                            &mut w,
                            &mut h,
                            &mut hw,
                        )
                    };

                    if n < 0 {
                        decode_errors += 1;
                        if decode_errors >= 10 && total_frames_decoded == 0 && ci + 1 < chain.len()
                        {
                            tracing::warn!("Hardware decode produced no frames, falling back");
                            dec = None;
                            ci += 1;
                            decode_errors = 0;
                        }
                    } else if n > 0 && w > 0 && h > 0 {
                        decode_errors = 0;
                        total_frames_decoded += n as u64;

                        if let Ok(mut s) = stats.lock() {
                            s.record_frame(w as u32, h as u32);
                        }

                        // If VcamFrame is Camera, scale and send to softcam!
                        if current_vcam_frame == VcamFrame::Camera {
                            let rect = calculate_letterbox(w as u32, h as u32, out_w, out_h);

                            if rect.scaled_w != out_w || rect.scaled_h != out_h {
                                out_buf.fill(0);
                            }

                            let need_new_scaler = match &scaler {
                                Some(s) => {
                                    s.src_w != w as u32
                                        || s.src_h != h as u32
                                        || s.dst_w != rect.scaled_w
                                        || s.dst_h != rect.scaled_h
                                }
                                None => true,
                            };
                            if need_new_scaler {
                                scaler = Some(ScalerWrapper::new(
                                    w as u32,
                                    h as u32,
                                    rect.scaled_w,
                                    rect.scaled_h,
                                ));
                            }

                            if let Some(s) = &scaler {
                                let dst_offset =
                                    ((rect.offset_y * out_w + rect.offset_x) * 3) as usize;
                                let dst_ptr = unsafe { out_buf.as_mut_ptr().add(dst_offset) };
                                let dst_stride = (out_w * 3) as i32;
                                s.scale(dec_buf.as_ptr(), w * 3, dst_ptr, dst_stride, h);
                            }

                            cam.send_frame(&out_buf);
                            update_latest_frame(&latest_frame, out_w, out_h, &out_buf);
                            last_tick = Instant::now();
                        }
                    }
                }
            }
            None => {
                // Timeout elapsed (no AU arrived or not streaming)
                if current_vcam_frame != VcamFrame::Camera || total_frames_decoded == 0 {
                    placeholder::render_placeholder(
                        current_vcam_frame,
                        out_w,
                        out_h,
                        &mut out_buf,
                    );
                    cam.send_frame(&out_buf);
                    update_latest_frame(&latest_frame, out_w, out_h, &out_buf);
                    last_tick = Instant::now();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_pref_display_and_parse() {
        assert_eq!(DecoderPref::from_str("auto").unwrap(), DecoderPref::Auto);
        assert_eq!(DecoderPref::from_str("d3d11va").unwrap(), DecoderPref::D3d11va);
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

        // Preview should be available immediately from initial placeholder
        // Give background thread a short moment to render
        thread::sleep(Duration::from_millis(50));
        let preview = handle.preview_jpeg(320);
        assert!(preview.is_some(), "Preview JPEG should be produced from initial placeholder");
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
