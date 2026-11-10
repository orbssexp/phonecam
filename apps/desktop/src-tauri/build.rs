use std::path::{Path, PathBuf};

/// Stage the FFmpeg + softcam DLLs into `vendor/` so `bundle.resources` can ship them next to the
/// exe. Same lookup as `crates/phonecam-media-win/build.rs` (env override, else the dev vendor dir).
fn main() {
    let ffmpeg_dir = std::env::var("FFMPEG_DIR").unwrap_or_else(|_| {
        r"C:\Users\gusta\orca\projects\phone-cam\spike\vendor\ffmpeg\ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1".into()
    });
    let softcam_dir = std::env::var("SOFTCAM_DIR").unwrap_or_else(|_| {
        r"C:\Users\gusta\orca\projects\phone-cam\spike\vendor\softcam-phonecam".into()
    });

    let vendor = PathBuf::from("vendor");
    let win32 = vendor.join("win32");
    let _ = std::fs::create_dir_all(&win32);

    copy_dlls(&Path::new(&ffmpeg_dir).join("bin"), &vendor);
    copy_dlls(&Path::new(&softcam_dir).join("bin").join("x64"), &vendor);
    copy_dlls(&Path::new(&softcam_dir).join("bin").join("Win32"), &win32);

    for need in ["vendor/softcam.dll", "vendor/avcodec-61.dll", "vendor/win32/softcam.dll"] {
        if !Path::new(need).exists() {
            println!("cargo:warning=missing {need}: set FFMPEG_DIR / SOFTCAM_DIR (installer would lack it)");
        }
    }

    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=SOFTCAM_DIR");
    println!("cargo:rerun-if-changed=windows/hooks.nsh");
    tauri_build::build();
}

fn copy_dlls(src: &Path, dst: &Path) {
    let Ok(entries) = std::fs::read_dir(src) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("dll") {
            if let Some(name) = p.file_name() {
                let _ = std::fs::copy(&p, dst.join(name));
            }
        }
    }
}
