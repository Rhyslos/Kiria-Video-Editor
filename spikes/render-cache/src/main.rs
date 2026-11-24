//! Compares ways of keeping rendered frames on disk for the render cache.
//!
//! Usage: render-cache-bench <frames.rgba> <width> <height> <work folder>
//!
//! `frames.rgba` is raw RGBA frames one after another (see `make-frames.sh`).
//! Each way writes every frame to the work folder and reads it back, timed,
//! and reports the bytes on disk per frame. Video encodings are timed by
//! `video.sh` with FFmpeg itself.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Way {
    name: &'static str,
    pack: fn(&[u8]) -> Vec<u8>,
    unpack: fn(&[u8], usize) -> Vec<u8>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("usage: render-cache-bench <frames.rgba> <width> <height> <work folder>");
        std::process::exit(2);
    }
    let (w, h): (usize, usize) = (args[2].parse().unwrap(), args[3].parse().unwrap());
    let frame_bytes = w * h * 4;
    WIDTH.store(w, std::sync::atomic::Ordering::Relaxed);
    let mut all = Vec::new();
    File::open(&args[1]).unwrap().read_to_end(&mut all).unwrap();
    let frames: Vec<&[u8]> = all.chunks_exact(frame_bytes).collect();
    let work = PathBuf::from(&args[4]);
    fs::create_dir_all(&work).unwrap();
    println!("{} frames of {w}x{h} RGBA ({:.1} MB each)", frames.len(), frame_bytes as f64 / 1e6);
    let ways = [
        Way { name: "raw RGBA", pack: |f| f.to_vec(), unpack: |b, _| b.to_vec() },
        Way { name: "raw YUV 4:2:0 (8-bit)", pack: |f| yuv420(f), unpack: |b, _| b.to_vec() },
        Way { name: "lz4 RGBA", pack: |f| lz4_flex::compress_prepend_size(f), unpack: |b, _| lz4_flex::decompress_size_prepended(b).unwrap() },
        Way { name: "lz4 YUV 4:2:0", pack: |f| lz4_flex::compress_prepend_size(&yuv420(f)), unpack: |b, _| lz4_flex::decompress_size_prepended(b).unwrap() },
        Way { name: "zstd-1 YUV 4:2:0", pack: |f| zstd::bulk::compress(&yuv420(f), 1).unwrap(), unpack: |b, n| zstd::bulk::decompress(b, n).unwrap() },
        Way { name: "zstd-3 YUV 4:2:0", pack: |f| zstd::bulk::compress(&yuv420(f), 3).unwrap(), unpack: |b, n| zstd::bulk::decompress(b, n).unwrap() },
        Way { name: "JPEG q85 (image crate)", pack: jpeg, unpack: |b, _| unjpeg(b) },
    ];
    println!("{:<24} {:>10} {:>10} {:>10} {:>10}", "way", "MB/frame", "pack ms", "write ms", "read+unpack ms");
    for way in &ways {
        let dir = work.join(way.name.replace([' ', ':', '(', ')', '.'], "-"));
        fs::create_dir_all(&dir).unwrap();
        let (mut pack_ms, mut write_ms, mut bytes) = (0.0, 0.0, 0usize);
        for (i, f) in frames.iter().enumerate() {
            let t = Instant::now();
            let packed = (way.pack)(f);
            pack_ms += t.elapsed().as_secs_f64() * 1e3;
            bytes += packed.len();
            let t = Instant::now();
            let mut out = File::create(dir.join(format!("{i}"))).unwrap();
            out.write_all(&packed).unwrap();
            out.sync_data().unwrap();
            write_ms += t.elapsed().as_secs_f64() * 1e3;
        }
        drop_cache(&dir);
        let unpacked_len = if way.name.contains("YUV") {
            w * h * 3 / 2
        } else if way.name.contains("JPEG") {
            w * h * 3
        } else {
            frame_bytes
        };
        let mut read_ms = 0.0;
        for i in 0..frames.len() {
            let t = Instant::now();
            let b = fs::read(dir.join(format!("{i}"))).unwrap();
            let out = (way.unpack)(&b, unpacked_len);
            assert_eq!(out.len(), unpacked_len);
            read_ms += t.elapsed().as_secs_f64() * 1e3;
        }
        let n = frames.len() as f64;
        println!(
            "{:<24} {:>10.2} {:>10.1} {:>10.1} {:>10.1}",
            way.name,
            bytes as f64 / n / 1e6,
            pack_ms / n,
            write_ms / n,
            read_ms / n
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}

static WIDTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// BT.709 limited range 4:2:0, as the cache would keep display values.
fn yuv420(rgba: &[u8]) -> Vec<u8> {
    let w = WIDTH.load(std::sync::atomic::Ordering::Relaxed);
    let h = rgba.len() / 4 / w;
    let mut out = Vec::with_capacity(w * h * 3 / 2);
    for p in rgba.chunks_exact(4) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        out.push((16.0 + (0.1826 * r + 0.6142 * g + 0.0620 * b)) as u8);
    }
    let (mut u, mut v) = (Vec::with_capacity(w * h / 4), Vec::with_capacity(w * h / 4));
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut sum = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y + dy) * w + x + dx) * 4;
                for c in 0..3 {
                    sum[c] += rgba[i + c] as f32 / 4.0;
                }
            }
            let [r, g, b] = sum;
            u.push((128.0 - 0.1006 * r - 0.3386 * g + 0.4392 * b) as u8);
            v.push((128.0 + 0.4392 * r - 0.3989 * g - 0.0403 * b) as u8);
        }
    }
    out.extend(u);
    out.extend(v);
    out
}

/// Asks the system to forget the files' pages, so reads come from the disk.
fn drop_cache(dir: &Path) {
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("for f in '{}'/*; do dd if=\"$f\" iflag=nocache count=0 2>/dev/null; done; sync", dir.display()))
        .status();
}

fn jpeg(rgba: &[u8]) -> Vec<u8> {
    let w = WIDTH.load(std::sync::atomic::Ordering::Relaxed);
    let h = rgba.len() / 4 / w;
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn unjpeg(bytes: &[u8]) -> Vec<u8> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .unwrap()
        .into_rgb8()
        .into_raw()
}
