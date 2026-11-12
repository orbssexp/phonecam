use std::ffi::{c_char, c_void};

extern "C" {
    pub fn dec_create(mode: i32, err: *mut c_char, errlen: i32) -> *mut c_void;
    pub fn dec_name(h: *mut c_void) -> *const c_char;
    pub fn dec_destroy(h: *mut c_void);
    /// Give freed heap pages back to the OS and trim the working set.
    pub fn dec_trim_memory();
    pub fn dec_decode(
        h: *mut c_void,
        data: *const u8,
        len: i32,
        out: *mut u8,
        maxsize: i32,
        fit_w: i32,
        fit_h: i32,
        t_ms: *mut f64,
        w: *mut i32,
        h_out: *mut i32,
        hw_used: *mut i32,
    ) -> i32;

    pub fn scaler_create(src_w: i32, src_h: i32, dst_w: i32, dst_h: i32) -> *mut c_void;
    pub fn scaler_scale(
        s: *mut c_void,
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        h: i32,
    );
    pub fn scaler_destroy(s: *mut c_void);
}

#[link(name = "softcam")]
extern "C" {
    pub fn scCreateCamera(width: i32, height: i32, framerate: f32) -> *mut c_void;
    pub fn scDeleteCamera(camera: *mut c_void);
    pub fn scSendFrame(camera: *mut c_void, image_bits: *const u8);
    pub fn scIsConnected(camera: *mut c_void) -> bool;
}
