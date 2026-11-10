//! Linux debug builds: lets the editor and its tests find the FFmpeg libraries
//! that `cargo xtask ffmpeg` fetched. Installed copies get FFmpeg from the
//! installer (Windows) or the AppImage (Linux).

fn main() {
    println!("cargo::rerun-if-env-changed=FFMPEG_DIR");
    let linux = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux");
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    if let (true, true, Ok(dir)) = (linux, debug, std::env::var("FFMPEG_DIR")) {
        // The old-style search path also covers the libraries FFmpeg loads itself.
        println!("cargo::rustc-link-arg=-Wl,--disable-new-dtags,-rpath,{dir}/lib");
    }
}
