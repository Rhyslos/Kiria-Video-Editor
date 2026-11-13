//! 32 tracks with every effect on, and the master: how long each block
//! takes against the time it lasts (5.3 ms at 48 kHz).
//!
//! `cargo test -p audio --test stress` runs two seconds and only fails far
//! over the budget (test builds are barely optimised and test machines are
//! busy); it prints the numbers either way. The full run, ten seconds in a
//! release build, is by hand:
//!
//! ```sh
//! cargo test -p audio --release --test stress -- --ignored --nocapture
//! ```

#![allow(clippy::expect_used)]

mod common;

use audio::BLOCK;
use common::{RATE, full_mix};
use std::time::{Duration, Instant};

struct Times {
    mean: Duration,
    median: Duration,
    p99: Duration,
    worst: Duration,
    true_peak_max: f32,
    integrated: f32,
}

fn run(seconds: usize) -> Times {
    let mut mix = full_mix(seconds);
    let blocks = RATE as usize * seconds / BLOCK;
    let mut out = [0f32; 2 * BLOCK];
    let mut times = Vec::with_capacity(blocks);
    // As the sound card's thread will.
    os::float::denormals_as_zero();
    for _ in 0..blocks {
        mix.feed();
        let start = Instant::now();
        mix.mixer.process(&mut out);
        times.push(start.elapsed());
    }
    assert_eq!(mix.mixer.dry_frames(), 0, "every ring was fed in time");
    let master = mix.controller.master().levels();
    times.sort();
    let total: Duration = times.iter().sum();
    Times {
        mean: total / blocks as u32,
        median: times[blocks / 2],
        p99: times[blocks * 99 / 100],
        worst: times[blocks - 1],
        true_peak_max: master.true_peak_max,
        integrated: master.integrated,
    }
}

fn report(t: &Times) -> f64 {
    let budget = Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
    let share = t.mean.as_secs_f64() / budget.as_secs_f64();
    println!(
        "32 tracks, every effect, {BLOCK}-frame blocks ({budget:?} each): mean {:?} ({:.1} % of a block), median \
         {:?}, 99th {:?}, worst {:?}; master: integrated {:.1} LUFS, highest true peak {:.2} dBTP",
        t.mean,
        share * 100.0,
        t.median,
        t.p99,
        t.worst,
        t.integrated,
        t.true_peak_max,
    );
    share
}

#[test]
fn thirty_two_tracks_fit_in_the_block_time() {
    let t = run(2);
    let share = report(&t);
    assert!(
        t.true_peak_max <= -1.0,
        "the limiter holds the ceiling: {}",
        t.true_peak_max
    );
    // Lenient: a release build took about a quarter of a block on a busy
    // two-core cloud machine; a barely optimised test build on a busy
    // machine is allowed the whole block on average.
    assert!(share < 1.0, "mean {:?} is over the block's time", t.mean);
}

#[test]
#[ignore = "timing run, by hand in a release build"]
fn thirty_two_tracks_for_ten_seconds() {
    let t = run(10);
    report(&t);
    assert!(t.true_peak_max <= -1.0);
}
