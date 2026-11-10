//! decode-bench: measures decoding approaches on the same clips.
//!
//! Usage: decode-bench [--approach NAME]... [--seeks N] [--frames N] [--no-gpu] CLIP...
//! Approaches: software, hw-copy:<type> (d3d11va, d3d12va, vaapi, vulkan, ...), vulkan-direct.
//! With no --approach, every approach that makes sense on this system is tried.
//! Prints a Markdown table; paste it into docs/research/decoding.md.

mod decoder;
mod gpu;
mod vulkan_direct;

use decoder::{Decoder, Mode};
use std::time::{Duration, Instant};

struct Options {
    approaches: Vec<String>,
    seeks: usize,
    frames: usize,
    gpu: bool,
    clips: Vec<String>,
}

fn options() -> Result<Options, String> {
    let mut o = Options {
        approaches: Vec::new(),
        seeks: 30,
        frames: 600,
        gpu: true,
        clips: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--approach" => o.approaches.push(args.next().ok_or("--approach needs a name")?),
            "--seeks" => {
                o.seeks = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--seeks needs a number")?
            }
            "--frames" => {
                o.frames = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--frames needs a number")?
            }
            "--no-gpu" => o.gpu = false,
            "-h" | "--help" => {
                return Err(
                    "usage: decode-bench [--approach NAME]... [--seeks N] [--frames N] [--no-gpu] CLIP..."
                        .into(),
                );
            }
            _ => o.clips.push(a),
        }
    }
    if o.clips.is_empty() {
        return Err("give at least one clip".into());
    }
    if o.approaches.is_empty() {
        o.approaches.push("software".into());
        let hw: &[&str] = if cfg!(windows) {
            &["d3d11va", "d3d12va", "vulkan"]
        } else {
            &["vaapi", "vulkan"]
        };
        o.approaches.extend(hw.iter().map(|t| format!("hw-copy:{t}")));
        o.approaches.push("vulkan-direct".into());
    }
    Ok(o)
}

fn mode(approach: &str) -> Result<Mode, String> {
    match approach.split_once(':') {
        None if approach == "software" => Ok(Mode::Software),
        None if approach == "vulkan-direct" => Ok(Mode::VulkanDirect),
        Some(("hw-copy", kind)) => Ok(Mode::HardwareCopy(kind.into())),
        _ => Err(format!("unknown approach {approach}")),
    }
}

struct Row {
    play_fps: f64,
    cpu_percent: f64,
    seek_median_ms: f64,
    seek_worst_ms: f64,
    streams2_fps: f64,
    streams4_fps: f64,
    memory_mb: f64,
}

/// Small deterministic random numbers, so every approach jumps to the same places.
struct Lcg(u64);
impl Lcg {
    fn next_unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Plays `frames` frames straight through, uploading each. Returns frames done and wall time.
fn play(clip: &str, mode: &Mode, frames: usize, upload: bool) -> Result<(usize, Duration), String> {
    let mut dec = Decoder::open(clip, mode)?;
    let mut gpu = if upload { Some(gpu::Gpu::new()?) } else { None };
    let start = Instant::now();
    let mut n = 0;
    while n < frames {
        match dec.next()? {
            Some(p) => {
                if let Some(g) = gpu.as_mut() {
                    g.upload(&p);
                }
                n += 1;
            }
            None => break,
        }
    }
    if let Some(g) = &gpu {
        g.finish();
    }
    Ok((n, start.elapsed()))
}

fn streams(clip: &str, mode: &Mode, count: usize, frames: usize, upload: bool) -> Result<f64, String> {
    let start = Instant::now();
    let handles: Vec<_> = (0..count)
        .map(|_| {
            let (clip, mode) = (clip.to_owned(), mode.clone());
            std::thread::spawn(move || play(&clip, &mode, frames, upload))
        })
        .collect();
    let mut total = 0;
    for h in handles {
        total += h.join().map_err(|_| "stream thread panicked")??.0;
    }
    Ok(total as f64 / start.elapsed().as_secs_f64())
}

fn measure(clip: &str, approach: &str, o: &Options) -> Result<(Row, String), String> {
    let mode = mode(approach)?;
    // Playing straight through, with CPU time.
    let cpu = cpu_time::ProcessTime::now();
    let (n, wall) = play(clip, &mode, o.frames, o.gpu)?;
    let cpu_percent = cpu.elapsed().as_secs_f64() / wall.as_secs_f64() * 100.0;
    let memory_mb = memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64 / 1e6);

    // Random jumps.
    let mut dec = Decoder::open(clip, &mode)?;
    let codec = match dec.device_note() {
        Some(note) => format!("{} ({note})", dec.codec_name),
        None => dec.codec_name.clone(),
    };
    let mut gpu = if o.gpu { Some(gpu::Gpu::new()?) } else { None };
    let mut rng = Lcg(42);
    let mut times = Vec::with_capacity(o.seeks);
    let span = (dec.duration - 0.5).max(0.0);
    for _ in 0..o.seeks {
        let target = rng.next_unit() * span;
        let start = Instant::now();
        dec.seek_and_decode(target, |p| {
            if let Some(g) = gpu.as_mut() {
                g.upload(p);
                g.finish();
            }
        })?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let (seek_median_ms, seek_worst_ms) = if times.is_empty() {
        (0.0, 0.0)
    } else {
        (times[times.len() / 2], times[times.len() - 1])
    };

    let few = (o.frames / 2).max(60);
    let streams2_fps = streams(clip, &mode, 2, few, o.gpu)?;
    let streams4_fps = streams(clip, &mode, 4, few, o.gpu)?;
    Ok((
        Row {
            play_fps: n as f64 / wall.as_secs_f64(),
            cpu_percent,
            seek_median_ms,
            seek_worst_ms,
            streams2_fps,
            streams4_fps,
            memory_mb,
        },
        codec,
    ))
}

fn main() {
    let o = match options() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    unsafe { ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_ERROR) };
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let adapter = if o.gpu {
        gpu::Gpu::new()
            .map(|g| g.adapter)
            .unwrap_or_else(|e| format!("none ({e})"))
    } else {
        "not used (--no-gpu)".into()
    };
    println!(
        "System: {} {}, {cores} CPU threads; GPU: {adapter}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(
        "| Clip | Approach | Decoder | Play fps | CPU % (100 = one thread) | Jump median ms | Jump worst ms | 2 streams fps | 4 streams fps | Memory MB |"
    );
    println!("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for clip in &o.clips {
        let name = std::path::Path::new(clip)
            .file_name()
            .map_or(clip.clone(), |n| n.to_string_lossy().into_owned());
        for approach in &o.approaches {
            match measure(clip, approach, &o) {
                Ok((r, codec)) => println!(
                    "| {name} | {approach} | {codec} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
                    r.play_fps,
                    r.cpu_percent,
                    r.seek_median_ms,
                    r.seek_worst_ms,
                    r.streams2_fps,
                    r.streams4_fps,
                    r.memory_mb
                ),
                Err(e) => println!("| {name} | {approach} | – | failed: {e} | | | | | | |"),
            }
        }
    }
}
