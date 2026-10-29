// Builds the tiny C FFmpeg decode shim (bindgen-based ffmpeg-sys-next fails with libclang 23 here).
fn main() {
    let dir = std::env::var("FFMPEG_DIR").expect("set FFMPEG_DIR to the BtbN ffmpeg-*-win64-lgpl-shared dir");
    cc::Build::new()
        .file("src/dec.c")
        .include(format!("{dir}/include"))
        .warnings(false)
        .compile("dec");
    println!("cargo:rustc-link-search=native={dir}/lib");
    for l in ["avcodec", "avutil", "swscale"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    for l in ["dxgi", "d3d11", "ole32"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    println!("cargo:rustc-link-search=native=vendor/softcam-src/src/softcam/dist/lib/x64");
    println!("cargo:rerun-if-changed=src/dec.c");
}
