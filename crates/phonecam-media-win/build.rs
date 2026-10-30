fn main() {
    let default_ffmpeg = r"C:\Users\gusta\orca\projects\phone-cam\spike\vendor\ffmpeg\ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1";
    let ffmpeg_dir = std::env::var("FFMPEG_DIR").unwrap_or_else(|_| default_ffmpeg.to_string());

    let default_softcam = r"C:\Users\gusta\orca\projects\phone-cam\spike\vendor\softcam-src\src\softcam\dist";
    let softcam_dir = std::env::var("SOFTCAM_DIR").unwrap_or_else(|_| default_softcam.to_string());

    if std::env::var_os("LIBCLANG_PATH").is_none() {
        println!("cargo:rustc-env=LIBCLANG_PATH=C:\\Program Files\\LLVM\\bin");
    }

    cc::Build::new()
        .file("src/dec.c")
        .include(format!("{ffmpeg_dir}/include"))
        .warnings(false)
        .compile("dec");

    println!("cargo:rustc-link-search=native={ffmpeg_dir}/lib");
    for l in ["avcodec", "avutil", "swscale"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    for l in ["dxgi", "d3d11", "ole32"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }

    println!("cargo:rustc-link-search=native={softcam_dir}/lib/x64");
    println!("cargo:rustc-link-lib=dylib=softcam");

    // Copy runtime DLLs to target output directory and deps directory so that
    // cargo test / cargo run can locate them without manual PATH setup on Windows.
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let out_path = std::path::Path::new(&out_dir);
        if let Some(target_dir) = out_path.ancestors().nth(3) {
            let deps_dir = target_dir.join("deps");
            let _ = std::fs::create_dir_all(&deps_dir);

            let ffmpeg_bin = std::path::Path::new(&ffmpeg_dir).join("bin");
            let softcam_bin = std::path::Path::new(&softcam_dir).join("bin").join("x64");

            let copy_dlls = |src_dir: &std::path::Path| {
                if let Ok(entries) = std::fs::read_dir(src_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("dll") {
                            if let Some(filename) = path.file_name() {
                                let _ = std::fs::copy(&path, target_dir.join(filename));
                                let _ = std::fs::copy(&path, deps_dir.join(filename));
                            }
                        }
                    }
                }
            };
            copy_dlls(&ffmpeg_bin);
            copy_dlls(&softcam_bin);
        }
    }

    println!("cargo:rerun-if-changed=src/dec.c");
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=SOFTCAM_DIR");
}
