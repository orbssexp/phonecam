use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[repr(C)]
#[allow(non_snake_case)]
struct Pmc2 {
    cb: u32,
    PageFaultCount: u32,
    PeakWorkingSetSize: usize,
    WorkingSetSize: usize,
    QuotaPeakPagedPoolUsage: usize,
    QuotaPagedPoolUsage: usize,
    QuotaPeakNonPagedPoolUsage: usize,
    QuotaNonPagedPoolUsage: usize,
    PagefileUsage: usize,
    PeakPagefileUsage: usize,
    PrivateUsage: usize,
    PrivateWorkingSetSize: u64,
    SharedCommitUsage: u64,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    lo: u32,
    hi: u32,
}
extern "system" {
    fn K32GetProcessMemoryInfo(h: isize, p: *mut Pmc2, cb: u32) -> i32;
    fn GetCurrentProcess() -> isize;
    fn GetProcessTimes(h: isize, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
}

pub fn private_ws_mb() -> f64 {
    unsafe {
        let mut m: Pmc2 = std::mem::zeroed();
        m.cb = std::mem::size_of::<Pmc2>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) == 0 {
            return -1.0;
        }
        m.PrivateWorkingSetSize as f64 / 1048576.0
    }
}

fn cpu_100ns() -> u64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let f = |t: FileTime| ((t.hi as u64) << 32) | t.lo as u64;
        f(k) + f(u)
    }
}

#[derive(Default)]
pub struct Stats {
    pub frames: AtomicU64,
    pub w: AtomicU64,
    pub h: AtomicU64,
    pub decode_us_total: AtomicU64,
    pub decoder: Mutex<String>,
}

/// Samples private working set + CPU% (of total logical CPUs) every second into a CSV.
pub fn spawn(path: String, label: String, stats: Arc<Stats>) {
    std::thread::spawn(move || {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).expect("metrics file");
        if f.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
            writeln!(f, "label,t_s,private_ws_mb,cpu_pct_total,fps,decode_ms_avg,w,h,decoder").ok();
        }
        let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let t0 = Instant::now();
        let (mut last_t, mut last_cpu, mut last_fr, mut last_us) = (Instant::now(), cpu_100ns(), 0u64, 0u64);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = Instant::now();
            let cpu = cpu_100ns();
            let dt = now.duration_since(last_t).as_secs_f64();
            let cpu_pct = (cpu - last_cpu) as f64 / 1e7 / dt / ncpu * 100.0;
            let fr = stats.frames.load(Ordering::Relaxed);
            let us = stats.decode_us_total.load(Ordering::Relaxed);
            let dfr = fr - last_fr;
            let fps = dfr as f64 / dt;
            let dec_ms = if dfr > 0 { (us - last_us) as f64 / dfr as f64 / 1000.0 } else { 0.0 };
            writeln!(
                f,
                "{},{:.0},{:.1},{:.2},{:.1},{:.2},{},{},{}",
                label,
                t0.elapsed().as_secs_f64(),
                private_ws_mb(),
                cpu_pct,
                fps,
                dec_ms,
                stats.w.load(Ordering::Relaxed),
                stats.h.load(Ordering::Relaxed),
                stats.decoder.lock().unwrap().replace(',', ";")
            )
            .ok();
            f.flush().ok();
            last_t = now;
            last_cpu = cpu;
            last_fr = fr;
            last_us = us;
        }
    });
}
