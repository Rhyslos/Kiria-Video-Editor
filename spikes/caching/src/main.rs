//! cache-bench: measures ways to make scrubbing and jumping feel instant.
//!
//! Usage: cache-bench [--decode NAME]... [--jumps N] [--preview WxH]
//!                    [--cache-mb N] [--span SECONDS] [--work FOLDER]
//!                    [--ffmpeg PROGRAM] [--quick] [--no-traces] CLIP...
//!
//! `--decode` is `software` or an FFmpeg device type (`d3d11va`, `vaapi`, ...)
//! whose frames are copied back to memory, the editor's current path. The
//! default is `d3d11va` on Windows and `software` elsewhere.
//!
//! For every clip it prints Markdown tables: jumps, the files built for the
//! proxy and picture-index approaches, decoder-level tricks, and scrub traces.
//! Paste them into docs/research/caching.md.

mod build;
mod cache;
mod decoder;
mod scrub;
mod trace;

use decoder::{Decoder, KeyIndex, Picture, Pixels, Skip, picture_bytes};
use scrub::{First, Rig, Small, median};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

struct Options {
    /// `None` is software decoding.
    decode: Vec<Option<String>>,
    jumps: usize,
    preview: (u32, u32),
    cache_mb: usize,
    /// Only the first so many seconds of each clip are used, so long
    /// recordings don't take an hour to build proxies for.
    span: f64,
    work: PathBuf,
    ffmpeg: String,
    quick: bool,
    traces: bool,
    clips: Vec<String>,
}

const USAGE: &str = "usage: cache-bench [--decode NAME]... [--jumps N] [--preview WxH] [--cache-mb N] \
[--span SECONDS] [--work FOLDER] [--ffmpeg PROGRAM] [--quick] [--no-traces] CLIP...";

fn options() -> Result<Options, String> {
    let mut o = Options {
        decode: Vec::new(),
        jumps: 100,
        preview: (1280, 720),
        cache_mb: 1024,
        span: 60.0,
        work: std::env::temp_dir().join("cache-bench"),
        ffmpeg: "ffmpeg".into(),
        quick: false,
        traces: true,
        clips: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    let value = |name: &str, args: &mut dyn Iterator<Item = String>| {
        args.next().ok_or(format!("{name} needs a value"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--decode" => {
                let name = value("--decode", &mut args)?;
                o.decode.push((name != "software").then_some(name));
            }
            "--jumps" => {
                o.jumps = value("--jumps", &mut args)?
                    .parse()
                    .map_err(|_| "--jumps needs a number")?
            }
            "--cache-mb" => {
                o.cache_mb = value("--cache-mb", &mut args)?
                    .parse()
                    .map_err(|_| "--cache-mb needs a number")?
            }
            "--span" => {
                o.span = value("--span", &mut args)?
                    .parse()
                    .map_err(|_| "--span needs seconds")?
            }
            "--preview" => {
                let text = value("--preview", &mut args)?;
                let size = text
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                o.preview = size.ok_or("--preview needs a size like 1280x720")?;
            }
            "--work" => o.work = value("--work", &mut args)?.into(),
            "--ffmpeg" => o.ffmpeg = value("--ffmpeg", &mut args)?,
            "--quick" => o.quick = true,
            "--no-traces" => o.traces = false,
            "-h" | "--help" => return Err(USAGE.into()),
            _ => o.clips.push(a),
        }
    }
    if o.clips.is_empty() {
        return Err(format!("give at least one clip\n{USAGE}"));
    }
    if o.decode.is_empty() {
        o.decode.push(cfg!(windows).then(|| "d3d11va".to_owned()));
    }
    if o.quick {
        o.jumps = o.jumps.min(5);
    }
    Ok(o)
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

/// Jump targets: the middle of random frames in the first `span` seconds.
fn targets(span: f64, fps: f64, count: usize) -> Vec<f64> {
    let mut rng = Lcg(42);
    let frames = ((span - 0.5).max(0.1) * fps).floor();
    (0..count)
        .map(|_| ((rng.next_unit() * frames).floor() + 0.5) / fps)
        .collect()
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Milliseconds for a table: one decimal below 10, none above.
fn show(ms: f64) -> String {
    if ms == 0.0 {
        "0".into()
    } else if ms < 9.95 {
        format!("{ms:.1}")
    } else {
        format!("{ms:.0}")
    }
}

/// "median | worst" of a list of milliseconds, as two table cells.
fn cells(times: &[f64]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    format!(
        "{} | {}",
        show(median(&sorted)),
        show(sorted.last().copied().unwrap_or(0.0))
    )
}

fn sum(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// How far two pictures of the same size differ: the mean difference of
/// their brightness values (0 = the same picture, 255 = black against white).
fn difference(a: &Picture, b: &Picture) -> f64 {
    let luma = (a.width * a.height) as usize;
    if a.data.len() != b.data.len() || luma == 0 {
        return f64::NAN;
    }
    let total: u64 = a.data[..luma]
        .iter()
        .zip(&b.data[..luma])
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    total as f64 / luma as f64
}

/// A file built for approach C or D, opened for fetching.
struct Source {
    recipe: &'static build::Recipe,
    built: build::Built,
    small: Small,
}

/// One clip with one way of decoding: prints its tables.
fn measure(clip: &str, hardware: &Option<String>, o: &Options) -> Result<(), String> {
    let name = std::path::Path::new(clip)
        .file_name()
        .map_or(clip.to_owned(), |n| n.to_string_lossy().into_owned());
    let decode = decoder::Options::new(hardware);
    let asked = hardware.as_deref().unwrap_or("software");

    // Opening, and where the key frames are.
    let started = Instant::now();
    let mut main = Decoder::open(clip, &decode)?;
    let open_ms = ms(started);
    let span = if main.duration > 0.0 {
        main.duration.min(o.span)
    } else {
        o.span
    };
    let fps = main.fps;
    let listed = main.container_keys(span);
    let started = Instant::now();
    let scanned = main.scan_keys(span)?;
    let scan_ms = ms(started);
    let keys = KeyIndex {
        keys: if scanned.is_empty() {
            listed.clone()
        } else {
            scanned.clone()
        },
    };
    main.seek(0.0)?;
    main.next()?;
    let method = main.method();
    let gaps = keys.gaps();
    println!("\n## {name}, decoded with {method}\n");
    if method != asked {
        println!("{asked} can't decode this file here, so it fell back to {method}.\n");
    }
    println!(
        "{} {}x{} at {:.0} frames a second; the first {span:.0} s used. Key frames: {} found by reading \
         the packets ({scan_ms:.0} ms), {} listed by the container; {:.1} s apart (median), {:.1} s at most. \
         Opening the file took {open_ms:.0} ms.\n",
        main.codec_name,
        main.width,
        main.height,
        fps,
        scanned.len(),
        listed.len(),
        median(&gaps),
        gaps.last().copied().unwrap_or(0.0),
    );
    let peek = Decoder::open(clip, &decode)?;
    let mut rig = Rig::new(main, peek, &keys, o.preview, o.cache_mb * 1024 * 1024, span);
    let (pw, ph) = decoder::fit(rig.main.width, rig.main.height, o.preview);

    // Approaches C and D: build their files.
    let folder = o.work.join(format!("{name}-{method}"));
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut sources = Vec::new();
    let mut failures = Vec::new();
    for recipe in &build::RECIPES {
        eprintln!("  building {}", recipe.name);
        let made = build::build(&o.ffmpeg, recipe, clip, span, hardware, &folder).and_then(|built| {
            let decoder = Decoder::open(&built.path, &decoder::Options::light())?;
            let small = Small {
                decoder,
                all_keys: recipe.all_keys,
            };
            Ok(Source { recipe, built, small })
        });
        match made {
            Ok(source) => sources.push(source),
            Err(e) => failures.push(format!("| {} | failed: {e} | | | | | |", recipe.name)),
        }
    }

    // Jumps: the same targets for every approach.
    eprintln!("  jumps");
    let targets = targets(span, fps, o.jumps);
    let frame_start = |t: f64| (t * fps).floor() / fps;
    // "today" and "with a key frame index" take turns on each target, each
    // with its own decoder, so a busy machine slows both alike.
    let (mut today, mut exact) = (Vec::new(), Vec::new());
    {
        let (main, peek) = (Decoder::open(clip, &decode)?, Decoder::open(clip, &decode)?);
        let mut plain = Rig::new(main, peek, &keys, o.preview, 0, span);
        plain.reset()?;
        rig.reset()?;
        for &t in &targets {
            let started = Instant::now();
            plain.exact(t, false)?;
            today.push(ms(started));
            let started = Instant::now();
            rig.exact(t, true)?;
            exact.push(ms(started));
        }
    }
    println!("### Jumps ({} random targets)\n", targets.len());
    println!(
        "| Approach | First picture median ms | worst ms | First picture off by, median ms | worst ms | Exact frame median ms | worst ms |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    println!("| today | {} | 0 | 0 | {} |", cells(&today), cells(&today));
    println!(
        "| with a key frame index | {} | 0 | 0 | {} |",
        cells(&exact),
        cells(&exact)
    );
    let (mut first, mut off) = (Vec::new(), Vec::new());
    for &t in &targets {
        let started = Instant::now();
        let stamp = rig.peek.key_frame_at(t)?.ok_or("no key frame")?;
        rig.peek.picture(o.preview, Pixels::Yuv420)?;
        first.push(ms(started));
        off.push((frame_start(t) - stamp.time).abs() * 1000.0);
    }
    println!(
        "| A key frame first | {} | {} | {} |",
        cells(&first),
        cells(&off),
        cells(&sum(&first, &exact))
    );
    let mut fetches = Vec::new();
    for source in &mut sources {
        let (mut first, mut off) = (Vec::new(), Vec::new());
        for &t in &targets {
            let started = Instant::now();
            let stamp = source.small.fetch(t)?.ok_or("nothing to fetch")?;
            source.small.decoder.picture(o.preview, Pixels::Yuv420)?;
            first.push(ms(started));
            // Containers that count in milliseconds round the times a little.
            let distance = (frame_start(t) - stamp.time).abs();
            off.push(if distance < 2e-3 { 0.0 } else { distance * 1000.0 });
        }
        println!(
            "| {} | {} | {} | {} |",
            source.recipe.name,
            cells(&first),
            cells(&off),
            cells(&sum(&first, &exact))
        );
        fetches.push(cells(&first));
    }
    println!("\nFor A, C and D the exact frame follows the first picture on the same worker.\n");

    println!("### Files built for C and D\n");
    println!(
        "| Built | Build s | Times faster than playing | MB | MB per minute | Fetch median ms | worst ms |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (source, fetch) in sources.iter().zip(&fetches) {
        let mb = source.built.bytes as f64 / 1e6;
        println!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.0} | {fetch} |",
            source.recipe.name,
            source.built.seconds,
            span / source.built.seconds,
            mb,
            mb * 60.0 / span
        );
    }
    for failure in &failures {
        println!("{failure}");
    }
    std::io::stdout().flush().ok();

    eprintln!("  decoder tricks");
    tricks(clip, &decode, &mut rig, &targets, o)?;

    if !o.traces {
        return Ok(());
    }
    // Scrub traces. Approach C uses the first proxy that was built, D the denser index.
    let mut take = |wanted: &[&str]| {
        let at = sources
            .iter()
            .position(|s| wanted.iter().any(|w| s.recipe.name.starts_with(w)))?;
        Some(sources.swap_remove(at))
    };
    let proxy = take(&["C1"]).or_else(|| take(&["C2", "C3"]));
    let index = take(&["D2"]).or_else(|| take(&["D1"]));
    println!("### Scrub traces\n");
    println!(
        "C uses \"{}\", D uses \"{}\". The memory budget for B is {} MB; a {pw}x{ph} picture takes {:.2} MB.\n",
        proxy
            .as_ref()
            .map_or("nothing (the build failed)", |s| s.recipe.name),
        index
            .as_ref()
            .map_or("nothing (the build failed)", |s| s.recipe.name),
        o.cache_mb,
        picture_bytes(pw, ph, Pixels::Yuv420) as f64 / 1e6
    );
    rig.proxy = proxy.map(|s| s.small);
    rig.index = index.map(|s| s.small);
    println!(
        "| Trace | Approach | Pictures a second | Response median ms | worst ms | Within 50 ms % | Right frame % | Exact frame % | From memory % | Settled after ms | Memory MB |"
    );
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for trace in trace::all(span, fps, o.quick) {
        eprintln!("  trace: {}", trace.name);
        for strategy in &scrub::STRATEGIES {
            let missing = match strategy.first {
                First::Proxy => rig.proxy.is_none(),
                First::Index => rig.index.is_none(),
                _ => false,
            };
            if missing {
                continue;
            }
            let shown = scrub::run(&mut rig, strategy, &trace.requests)?;
            let s = scrub::summarise(&trace.requests, &shown, strategy.refine);
            println!(
                "| {} | {} | {:.0} | {} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {} | {:.0} |",
                trace.name,
                strategy.name,
                s.pictures_per_second,
                show(s.response_median),
                show(s.response_worst),
                s.within_50ms_percent,
                s.right_frame_percent,
                s.exact_percent,
                s.from_memory_percent,
                show(s.settle),
                rig.cache.peak_bytes as f64 / 1e6
            );
        }
        std::io::stdout().flush().ok();
    }
    let memory = memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64 / 1e6);
    println!("\nThe whole program held {memory:.0} MB after this clip.");
    Ok(())
}

/// Decoder-level tricks: what a seek costs, racing to a target while
/// leaving work out, and what keeping every frame adds.
fn tricks(
    clip: &str,
    decode: &decoder::Options,
    rig: &mut Rig,
    targets: &[f64],
    o: &Options,
) -> Result<(), String> {
    let few = &targets[..targets.len().min(if o.quick { 3 } else { 20 })];
    let mut seeks = Vec::new();
    for &t in few {
        let started = Instant::now();
        rig.main.seek(t)?;
        seeks.push(ms(started));
    }

    // Racing to the same targets three ways. Each way has its own decoder
    // and they take turns on every target, so a busy machine slows all alike.
    let ways = [
        ("every frame", Skip::Nothing, false),
        ("leaving out frames nothing depends on", Skip::NonReference, false),
        ("without the deblocking filter", Skip::Nothing, true),
    ];
    let mut racers = Vec::new();
    for (label, skip, skip_loop_filter) in ways {
        let mut options = decode.clone();
        options.skip = skip;
        options.skip_loop_filter = skip_loop_filter;
        racers.push((label, Decoder::open(clip, &options)?, Vec::new(), Vec::new()));
    }
    for &t in few {
        for (_, decoder, times, pictures) in &mut racers {
            let started = Instant::now();
            decoder.frame_at(t)?.ok_or("no frame at a jump target")?;
            pictures.push(decoder.picture(o.preview, Pixels::Yuv420)?);
            times.push(ms(started));
        }
    }
    println!("### Decoder tricks\n");
    println!(
        "A seek alone, no decoding: {} ms (median | worst).\n",
        cells(&seeks)
    );
    println!(
        "| Racing to {} jump targets | Median ms | worst ms | Frames decoded | Picture differs by |",
        few.len()
    );
    println!("| --- | ---: | ---: | ---: | ---: |");
    let exact_pictures: &Vec<Picture> = &racers[0].3;
    for (label, decoder, times, pictures) in &racers {
        let differs: f64 = exact_pictures
            .iter()
            .zip(pictures)
            .map(|(x, y)| difference(x, y))
            .sum();
        println!(
            "| {label} | {} | {} | {:.2} |",
            cells(times),
            decoder.frames_out,
            differs / pictures.len().max(1) as f64
        );
    }
    drop(racers);

    // What approach B adds to every decoded frame: copying it out at preview
    // size. Three stretches of 40 frames for each way, in turns.
    let from = rig.keys.keys.get(1).copied().unwrap_or(0.0);
    let (rounds, stretch) = if o.quick { (1, 30) } else { (3, 40) };
    let mut run = |keep: Option<Pixels>| -> Result<(f64, u32), String> {
        rig.main.seek(from)?;
        let started = Instant::now();
        let mut done = 0;
        while done < stretch && rig.main.next()?.is_some() {
            if let Some(pixels) = keep {
                rig.main.picture(o.preview, pixels)?;
            }
            done += 1;
        }
        Ok((ms(started), done))
    };
    let kinds = [None, Some(Pixels::Yuv420), Some(Pixels::Rgba)];
    let mut totals = [(0.0, 0u32); 3];
    for _ in 0..rounds {
        for (kind, total) in kinds.iter().zip(&mut totals) {
            let (time, frames) = run(*kind)?;
            *total = (total.0 + time, total.1 + frames);
        }
    }
    let [alone, yuv, rgba] = totals.map(|(time, frames)| time / f64::from(frames.max(1)));
    let count = totals[0].1;
    let (w, h) = decoder::fit(rig.main.width, rig.main.height, o.preview);
    let mb = |pixels| picture_bytes(w, h, pixels) as f64 / 1e6;
    let fps = rig.main.fps;
    println!("\n| Decoding {count} frames in a row | ms per frame | MB per frame | MB per second of video |");
    println!("| --- | ---: | ---: | ---: |");
    println!("| decoding only | {alone:.2} | | |");
    println!(
        "| and keeping each at {w}x{h}, YUV 4:2:0 | {yuv:.2} | {:.2} | {:.0} |",
        mb(Pixels::Yuv420),
        mb(Pixels::Yuv420) * fps
    );
    println!(
        "| and keeping each at {w}x{h}, RGBA | {rgba:.2} | {:.2} | {:.0} |\n",
        mb(Pixels::Rgba),
        mb(Pixels::Rgba) * fps
    );
    std::io::stdout().flush().ok();
    Ok(())
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
    println!(
        "System: {} {}, {cores} CPU threads. Pictures are fitted into {}x{}.",
        std::env::consts::OS,
        std::env::consts::ARCH,
        o.preview.0,
        o.preview.1
    );
    let mut failed = false;
    for clip in &o.clips {
        for hardware in &o.decode {
            eprintln!("{clip} ({})", hardware.as_deref().unwrap_or("software"));
            if let Err(e) = measure(clip, hardware, &o) {
                println!("\n{clip}: failed: {e}");
                failed = true;
            }
            std::io::stdout().flush().ok();
        }
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_targets_are_the_same_every_time_and_inside_the_clip() {
        let a = targets(20.0, 60.0, 100);
        assert_eq!(a, targets(20.0, 60.0, 100));
        assert!(a.iter().all(|t| *t > 0.0 && *t < 19.5));
        // The middle of a frame.
        assert!(a.iter().all(|t| ((t * 60.0).fract() - 0.5).abs() < 1e-6));
    }

    #[test]
    fn milliseconds_get_a_decimal_only_when_small() {
        assert_eq!(show(0.0), "0");
        assert_eq!(show(0.64), "0.6");
        assert_eq!(show(9.96), "10");
        assert_eq!(show(431.4), "431");
        assert_eq!(cells(&[3.0, 1.0, 20.0]), "3.0 | 20");
    }

    #[test]
    fn the_same_picture_differs_by_nothing() {
        let stamp = decoder::Stamp { time: 0.0, end: 1.0 };
        let a = Picture {
            stamp,
            width: 2,
            height: 2,
            data: vec![10, 20, 30, 40, 0, 0],
        };
        let mut b = a.clone();
        assert_eq!(difference(&a, &b), 0.0);
        b.data[0] = 14;
        assert_eq!(difference(&a, &b), 1.0);
    }
}
