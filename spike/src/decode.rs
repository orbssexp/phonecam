use crate::metrics::Stats;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

extern "C" {
    fn dec_create(mode: i32, err: *mut c_char, errlen: i32) -> *mut c_void;
    fn dec_name(h: *mut c_void) -> *const c_char;
    fn dec_destroy(h: *mut c_void);
    fn dec_decode(h: *mut c_void, data: *const u8, len: i32, out: *mut u8, maxsize: i32, w: *mut i32, h: *mut i32, hw: *mut i32) -> i32;
}
#[link(name = "softcam")]
extern "C" {
    fn scCreateCamera(w: i32, h: i32, fps: f32) -> *mut c_void;
    fn scDeleteCamera(c: *mut c_void);
    fn scSendFrame(c: *mut c_void, bits: *const u8);
}

pub struct Au {
    pub data: Arc<[u8]>,
    pub key: bool,
    pub at: Instant,
}

struct Dec(*mut c_void);
impl Dec {
    fn new(mode: i32) -> Result<Dec, String> {
        let mut err = vec![0 as c_char; 256];
        let p = unsafe { dec_create(mode, err.as_mut_ptr(), 256) };
        if p.is_null() {
            Err(unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned())
        } else {
            Ok(Dec(p))
        }
    }
    fn name(&self) -> String {
        unsafe { CStr::from_ptr(dec_name(self.0)) }.to_string_lossy().into_owned()
    }
}
impl Drop for Dec {
    fn drop(&mut self) {
        unsafe { dec_destroy(self.0) }
    }
}

fn mode_of(s: &str) -> i32 {
    match s {
        "d3d11" => 1,
        "cuda" => 2,
        _ => 0,
    }
}

/// Decoder thread body. `pref` = auto|d3d11|cuda|cpu. Auto = d3d11 (NVIDIA adapter) -> cpu.
pub fn run(rx: Receiver<Au>, pref: String, vcam: bool, stats: Arc<Stats>) {
    let chain: Vec<i32> = match pref.as_str() {
        "auto" => vec![1, 0],
        p => vec![mode_of(p)],
    };
    let mut ci = 0;
    let mut dec: Option<Dec> = None;
    let mut buf = vec![0u8; 3840 * 2160 * 3];
    let mut cam: *mut c_void = std::ptr::null_mut();
    let (mut cw, mut ch) = (0, 0);
    let mut errs = 0;
    let mut first_frame_logged = false;
    while let Ok(au) = rx.recv() {
        if dec.is_none() {
            match Dec::new(chain[ci]) {
                Ok(d) => {
                    let n = d.name();
                    println!("[dec] opened: {n}");
                    *stats.decoder.lock().unwrap() = n;
                    dec = Some(d);
                }
                Err(e) => {
                    println!("[dec] init failed (mode {}): {e}", chain[ci]);
                    if ci + 1 < chain.len() {
                        ci += 1;
                        println!("[dec] falling back to next option");
                        continue;
                    }
                    println!("[dec] no decoder left; dropping AU");
                    continue;
                }
            }
        }
        let d = dec.as_ref().unwrap();
        let (mut w, mut h, mut hw) = (0i32, 0i32, 0i32);
        let t = Instant::now();
        let n = unsafe { dec_decode(d.0, au.data.as_ptr(), au.data.len() as i32, buf.as_mut_ptr(), buf.len() as i32, &mut w, &mut h, &mut hw) };
        let us = t.elapsed().as_micros() as u64;
        if n < 0 {
            errs += 1;
            if errs <= 5 || errs % 100 == 0 {
                println!("[dec] decode error {n} (#{errs}) key={} len={}", au.key, au.data.len());
            }
            if errs >= 10 && stats.frames.load(Ordering::Relaxed) == 0 && ci + 1 < chain.len() {
                println!("[dec] hw decode never produced a frame; falling back");
                dec = None;
                ci += 1;
                errs = 0;
            }
            continue;
        }
        if n > 0 {
            errs = 0;
            stats.decode_us_total.fetch_add(us, Ordering::Relaxed);
            stats.frames.fetch_add(n as u64, Ordering::Relaxed);
            stats.w.store(w as u64, Ordering::Relaxed);
            stats.h.store(h as u64, Ordering::Relaxed);
            if !first_frame_logged {
                first_frame_logged = true;
                println!("[dec] FIRST FRAME {w}x{h} hw_surface={hw} (AU->frame {} ms)", au.at.elapsed().as_millis());
            }
            if vcam {
                if cam.is_null() || cw != w || ch != h {
                    if !cam.is_null() {
                        unsafe { scDeleteCamera(cam) };
                    }
                    cam = unsafe { scCreateCamera(w, h, 0.0) };
                    cw = w;
                    ch = h;
                    println!("[vcam] softcam created {w}x{h} null={}", cam.is_null());
                }
                if !cam.is_null() {
                    unsafe { scSendFrame(cam, buf.as_ptr()) };
                }
            }
        }
    }
}
