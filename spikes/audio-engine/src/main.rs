//! Measures the audio engine designs from docs/research/audio-engine.md on
//! the same synthetic 32-track input and prints Markdown tables.
//!
//! `audio-bench` runs everything; `audio-bench quick` runs a shorter set.

mod alloc;
mod bench;
mod dsp;
mod graph;
mod os;
mod params;
mod settings;
mod signal;

#[cfg(test)]
mod checks;

use bench::{Stats, TRACKS, build, full_expand, full_gate, profile, run};
use dsp::Tap;
use graph::{Graph, Parts, Strip, chain_a, chain_b, chain_c};
use params::Way;
use settings::{Clean, PROBE, TrackSettings};
use signal::RATE;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

struct Analyser {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Analyser {
    fn start() -> (Analyser, Tap) {
        let (to, from) = rtrb::RingBuffer::new(48_000);
        let stop = Arc::new(AtomicBool::new(false));
        let bands = Arc::new(std::array::from_fn::<_, 64, _>(|_| AtomicU32::new(0)));
        let s = Arc::clone(&stop);
        let thread = std::thread::spawn(move || dsp::analyser(from, bands, s));
        (
            Analyser {
                stop,
                thread: Some(thread),
            },
            Tap { to },
        )
    }
}

impl Drop for Analyser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn period_us(block: usize) -> f64 {
    block as f64 / f64::from(RATE) * 1e6
}

fn main() {
    let quick = std::env::args().any(|a| a == "quick");
    let repeats = if quick { 1 } else { 5 };
    alloc::flush_denormals();
    let len = (RATE * bench::SECONDS) as usize;
    eprintln!("making the input…");
    let signals = Arc::new(signal::all(len));
    let profile = profile();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    println!("# Audio engine spike\n");
    println!(
        "{TRACKS} tracks, 48 kHz stereo, {} s of input, {cores} cores. Times in µs per block on one thread.\n",
        bench::SECONDS
    );

    // 1. How a track's effects are held.
    println!("## A, B, C: how a track's effects are held (full chain on every track, master on)\n");
    println!(
        "| Block | Design | Clean-up | Mean | Median | 99th | Worst | Worst, own processor time | Mean / block time | Worst / block time | Allocations | Output |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut tp_note = String::new();
    for block in [256usize, 512] {
        type Settings = fn(usize) -> TrackSettings;
        let cases: [(&str, &str, Settings); 4] = [
            ("A enum", "gate", full_gate),
            ("B boxed trait", "gate", full_gate),
            ("C fixed strip", "gate", full_gate),
            ("C fixed strip", "expander", full_expand),
        ];
        // The designs take turns, so a busy moment on the machine doesn't
        // fall on one of them only.
        let mut results: Vec<Vec<bench::Run>> = cases.iter().map(|_| Vec::new()).collect();
        for _ in 0..repeats {
            for (i, &(name, _, settings)) in cases.iter().enumerate() {
                let (analyser, tap) = Analyser::start();
                let r = match name {
                    "A enum" => run(
                        &mut build(&chain_a, &settings, &signals, &profile, Some(tap), true),
                        block,
                    ),
                    "B boxed trait" => run(
                        &mut build(&chain_b, &settings, &signals, &profile, Some(tap), true),
                        block,
                    ),
                    _ => run(
                        &mut build(&chain_c, &settings, &signals, &profile, Some(tap), true),
                        block,
                    ),
                };
                drop(analyser);
                results[i].push(r);
            }
        }
        for ((name, clean, _), runs) in cases.iter().zip(&results) {
            let Some(best) = runs.iter().min_by(|a, b| a.stats.mean.total_cmp(&b.stats.mean)) else {
                continue;
            };
            let mut p99s: Vec<f64> = runs.iter().map(|r| r.stats.p99).collect();
            p99s.sort_by(|a, b| a.total_cmp(b));
            let worst = runs.iter().map(|r| r.stats.max).fold(0.0, f64::max);
            let worst_cpu = runs
                .iter()
                .filter_map(|r| r.cpu.as_ref().map(|c| c.max))
                .reduce(f64::max);
            let allocs: usize = runs.iter().map(|r| r.allocs).sum();
            if *clean == "gate" && name.starts_with('C') && block == 256 {
                tp_note = format!(
                    "Master after the limiter: highest true peak {:.2} dBTP (ceiling -1.0), momentary loudness at the end {:.1} LUFS.",
                    best.tp_max_db, best.master_momentary
                );
            }
            let p = period_us(block);
            println!(
                "| {block} | {name} | {clean} | {:.0} | {:.0} | {:.0} | {:.0} | {} | {:.1} % | {:.1} % | {allocs} | {:.6e} |",
                best.stats.mean,
                best.stats.p50,
                p99s[p99s.len() / 2],
                worst,
                worst_cpu.map_or("–".into(), |c| format!("{c:.0}")),
                100.0 * best.stats.mean / p,
                100.0 * worst / p,
                best.sum
            );
        }
    }
    println!(
        "\nMean and median: the best of {repeats} runs; 99th: the median run's; worst: the worst block of all runs, by the clock and by the thread's own processor time (which leaves out time the thread was not running)."
    );
    println!("Output: a fingerprint of everything mixed; the same number means the same sound.\n");
    println!("{tp_note}\n");

    // 2. What each effect costs.
    println!("## What each effect costs (C, 256 samples, all 32 tracks)\n");
    println!("| On every track | Mean | Worst | Mean / block time |");
    println!("| --- | --- | --- | --- |");
    let roles = signal::role_of;
    let only = |f: fn(&mut TrackSettings, usize)| {
        move |t: usize| {
            let mut s = TrackSettings::none();
            f(&mut s, t);
            s
        }
    };
    type Edit = fn(&mut TrackSettings, usize);
    let rows: [(&str, Edit, bool); 9] = [
        (
            "nothing (reading, fader, meter, sum), master off",
            |_, _| {},
            false,
        ),
        (
            "nothing, master on (compressor, true-peak limiter, meters)",
            |_, _| {},
            true,
        ),
        ("high pass (24 dB/octave)", |s, _| s.hp = Some(80.0), true),
        ("EQ, 6 bands", |s, t| s.eq = full_gate(t).eq, true),
        ("compressor", |s, t| s.comp = full_gate(t).comp, true),
        (
            "clean-up: spectral gate",
            |s, _| s.clean = Clean::Gate(settings::GATE),
            true,
        ),
        (
            "clean-up: expander",
            |s, _| s.clean = Clean::Expand(settings::EXPAND),
            true,
        ),
        (
            "ducking (on 24 tracks, key from 8)",
            |s, t| s.duck = full_gate(t).duck,
            true,
        ),
        ("everything, gate", |s, t| *s = full_gate(t), true),
    ];
    let _ = roles;
    for (name, edit, master) in rows {
        let settings = only(edit);
        let r = run(
            &mut build(&chain_c, &settings, &signals, &profile, None, master),
            256,
        );
        println!(
            "| {name} | {:.0} | {:.0} | {:.1} % |",
            r.stats.mean,
            r.stats.max,
            100.0 * r.stats.mean / period_us(256)
        );
    }
    println!();

    // Clean-up quality.
    println!("## Clean-up: gate against expander on speech over steady noise\n");
    println!(
        "How far the sound goes down well inside pauses (more is better) and well inside words (less is better).\n"
    );
    println!("| Noise peaks | Method | Pauses down (dB) | Words down (dB) |");
    println!("| --- | --- | --- | --- |");
    for noise in [0.003f32, 0.02] {
        for gate in [true, false] {
            let (p, w) = bench::cleanup_quality(noise, gate);
            println!(
                "| {:.0} dBFS | {} | {p:.1} | {w:.2} |",
                20.0 * noise.log10(),
                if gate { "spectral gate" } else { "expander" }
            );
        }
    }
    println!();

    // 3. What a change costs.
    println!("## S1, S2, S3: what changing settings costs (256 samples, one thread)\n");
    println!(
        "Each block, the interface side changes the third EQ band's frequency of one track, or of all 32, then the real-time side takes the change and mixes the block.\n"
    );
    println!(
        "| Way | Changes | Real-time mean | Real-time worst | Interface per change | Real-time allocations |"
    );
    println!("| --- | --- | --- | --- | --- | --- |");
    for way in Way::ALL {
        for (label, tracks) in [
            ("none", 0usize),
            ("1 track every block", 1),
            ("32 tracks every block", 32),
        ] {
            let (mut g, mut tx, mut rx) = boxed(way, &signals, &profile, None);
            let blocks = (RATE * if quick { 2.0 } else { 5.0 }) as usize / 256;
            let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
            let mut rt = Vec::with_capacity(blocks);
            let mut ui = Vec::with_capacity(blocks);
            alloc::reset();
            for b in 0..blocks {
                let t0 = Instant::now();
                for t in 0..tracks {
                    tx.change((t * 7 + b) % TRACKS, PROBE, 1000.0 + (b % 500) as f32);
                }
                if tracks > 0 {
                    ui.push(t0.elapsed().as_secs_f64() * 1e6 / tracks as f64);
                }
                let t1 = Instant::now();
                alloc::on_rt(|| {
                    rx.poll(&mut g);
                    g.process(&mut l, &mut r);
                });
                rt.push(t1.elapsed().as_secs_f64() * 1e6);
                tx.collect();
            }
            let allocs = alloc::count();
            let rs = Stats::of(&mut rt);
            let us = Stats::of(&mut ui);
            println!(
                "| {} | {label} | {:.0} | {:.0} | {} | {allocs} |",
                way.name(),
                rs.mean,
                rs.max,
                if tracks == 0 {
                    "–".to_string()
                } else {
                    format!("{:.1}", us.mean)
                }
            );
        }
    }
    println!();

    // 4. Latency and wake-ups, paced in real time.
    println!("## Paced in real time: how soon a change is heard, how late the thread wakes\n");
    println!(
        "The real-time thread sleeps until each block is due, takes changes, mixes all 32 tracks. Another thread changes one setting 400 times at random moments 1 to 10 ms apart. \"Change seen\" is from the change to the start of the first block that uses it; what is heard adds the output buffer. \"Late\" is how long after a block was due the thread woke up. \"Over\" counts blocks finished more than one block time after they were due (a dropout with one block of buffer) and more than four (a dropout with four). Busy threads spin at ordinary priority on every core, standing in for decoding and the interface; \"raised\" asks the system to run the mixing thread first, as a sound card's own thread is run.\n"
    );
    println!(
        "| Way | Block | Busy threads | Priority | Change seen: median | 99th | worst (ms) | Late: median | 99th | worst (ms) | Over 1 | Over 4 | Blocks | Allocations |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut runs: Vec<(Way, usize, usize, bool)> = Way::ALL
        .iter()
        .flat_map(|&w| [(w, 256, 0, false), (w, 512, 0, false)])
        .collect();
    for block in [256, 512] {
        runs.push((Way::Commands, block, cores, false));
        runs.push((Way::Commands, block, cores, true));
    }
    for (way, block, busy, raise) in runs {
        paced(
            way,
            block,
            busy,
            raise,
            &signals,
            &profile,
            if quick { 1.5 } else { 3.0 },
        );
    }
}

/// A graph of boxed strips (design C) with both ends of a settings way.
fn boxed(
    way: Way,
    signals: &Arc<Vec<signal::Signal>>,
    profile: &[f32],
    tap: Option<Tap>,
) -> (
    Graph<Box<Strip>>,
    Box<dyn params::Sender>,
    Box<dyn params::Receiver>,
) {
    let settings: Vec<TrackSettings> = (0..TRACKS).map(full_gate).collect();
    let parts: Vec<Parts> = (0..TRACKS)
        .map(|t| Parts {
            profile: profile.to_vec(),
            fader: graph::automation(t, bench::SECONDS),
            tap: None,
        })
        .collect();
    let (tx, rx) = params::make(way, &settings, &parts);
    let g = build(
        &|s, p| Box::new(chain_c(s, p)),
        &full_gate,
        signals,
        profile,
        tap,
        true,
    );
    (g, tx, rx)
}

#[allow(clippy::too_many_arguments)]
fn paced(
    way: Way,
    block: usize,
    busy: usize,
    raise: bool,
    signals: &Arc<Vec<signal::Signal>>,
    profile: &[f32],
    seconds: f64,
) {
    const CHANGES: usize = 400;
    const PROBE_TRACK: usize = 5;
    let (mut g, mut tx, mut rx) = boxed(way, signals, profile, None);
    let base = Instant::now();
    let sent: Arc<Vec<AtomicU64>> = Arc::new((0..CHANGES).map(|_| AtomicU64::new(0)).collect());
    let stop = Arc::new(AtomicBool::new(false));
    let period = Duration::from_secs_f64(block as f64 / f64::from(RATE));
    let blocks = (seconds / period.as_secs_f64()) as usize;

    let burners: Vec<_> = (0..busy)
        .map(|_| {
            let s = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = 1.0f64;
                while !s.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = (x * 1.000_001).sin() + 1.0;
                    }
                }
                std::hint::black_box(x);
            })
        })
        .collect();

    let s2 = Arc::clone(&sent);
    let st = Arc::clone(&stop);
    let ui = std::thread::spawn(move || {
        let mut rng = signal::Rng::new(4);
        std::thread::sleep(Duration::from_millis(50));
        for seq in 0..CHANGES {
            if st.load(Ordering::Relaxed) {
                break;
            }
            s2[seq].store(base.elapsed().as_nanos() as u64, Ordering::Release);
            tx.change(PROBE_TRACK, PROBE, 1000.0 + seq as f32);
            tx.collect();
            std::thread::sleep(Duration::from_micros(1000 + (rng.f() * 9000.0) as u64));
        }
        // Keep freeing what comes back until the end.
        while !st.load(Ordering::Relaxed) {
            tx.collect();
            std::thread::sleep(Duration::from_millis(5));
        }
        tx.collect();
    });

    let s3 = Arc::clone(&sent);
    let rt = std::thread::spawn(move || {
        alloc::flush_denormals();
        let raised = raise && os::raise_priority();
        let mut seen = vec![0f64; CHANGES];
        let mut seen_n = 0usize;
        let mut late = Vec::with_capacity(blocks);
        let (mut over1, mut over4) = (0, 0);
        let (mut l, mut r) = (vec![0f32; block], vec![0f32; block]);
        let mut last = usize::MAX;
        let start = Instant::now();
        alloc::reset();
        alloc::on_rt(|| {
            for b in 0..blocks {
                let due = start + period * b as u32;
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
                let woke = Instant::now();
                late.push(woke.saturating_duration_since(due).as_secs_f64() * 1e3);
                rx.poll(&mut g);
                let probe = g.tracks[PROBE_TRACK].chain.settings.eq[2].freq;
                let seq = (probe - 1000.0).round();
                if seq >= 0.0 && (seq as usize) < CHANGES && seq as usize != last {
                    let seq = seq as usize;
                    last = seq;
                    let at = base.elapsed().as_nanos() as u64;
                    let sent_at = s3[seq].load(Ordering::Acquire);
                    if sent_at > 0 {
                        seen[seen_n] = (at.saturating_sub(sent_at)) as f64 / 1e6;
                        seen_n += 1;
                    }
                }
                g.process(&mut l, &mut r);
                let done = Instant::now();
                if done > due + period {
                    over1 += 1;
                }
                if done > due + period * 4 {
                    over4 += 1;
                }
            }
        });
        let allocs = alloc::count();
        seen.truncate(seen_n);
        (seen, late, over1, over4, allocs, raised)
    });
    let (mut seen, mut late, over1, over4, allocs, raised) = rt.join().unwrap_or_default();
    stop.store(true, Ordering::Relaxed);
    let _ = ui.join();
    for b in burners {
        let _ = b.join();
    }
    let s = Stats::of(&mut seen);
    let w = Stats::of(&mut late);
    println!(
        "| {} | {block} | {busy} | {} | {:.2} | {:.2} | {:.2} | {:.3} | {:.3} | {:.2} | {over1} | {over4} | {} | {allocs} |",
        way.name(),
        match (raise, raised) {
            (false, _) => "ordinary",
            (true, true) => "raised",
            (true, false) => "raised (refused)",
        },
        s.p50,
        s.p99,
        s.max,
        w.p50,
        w.p99,
        w.max,
        w.count
    );
}
