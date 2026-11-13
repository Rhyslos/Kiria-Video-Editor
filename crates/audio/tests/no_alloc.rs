//! The mixer never allocates or frees: 32 tracks with every effect on, a
//! command every block, swaps (lines, noise, a track replaced, one taken
//! out and put back) every few blocks, the analyser listening. Counted by
//! an allocator that counts what the mixing thread asks for while marked.
//!
//! In a program of its own: the counting allocator is the global one here.

#![allow(clippy::expect_used)]

mod common;

use audio::dsp::cleanup::NoiseProfile;
use audio::dsp::filters::FLAT;
use audio::{BLOCK, Command, TrackChange};
use common::{RATE, TRACKS, full, full_mix, lines};

#[global_allocator]
static COUNTING: os::alloc_count::Counting = os::alloc_count::Counting;

#[test]
fn mixing_never_allocates_or_frees() {
    // The counter counts (one test, so nothing else is marked meanwhile).
    let (v, n) = os::alloc_count::counted(|| Vec::<u8>::with_capacity(64));
    drop(v);
    assert_eq!(n, 1);
    let seconds = 2;
    let mut mix = full_mix(seconds);
    mix.controller.send(Command::Analyse(Some(3))).expect("room");
    let profile = NoiseProfile([-90.0; 64]);
    let mut out = [0f32; 2 * BLOCK];
    let mut counted = 0;
    let blocks = RATE as usize * seconds / BLOCK;
    for b in 0..blocks {
        // The interface's side: a change every block, a swap now and then.
        let mut band = FLAT[2];
        band.freq = 800.0 + (b % 40) as f32 * 10.0;
        mix.controller
            .change_track(b % TRACKS, TrackChange::EqBand(2, band))
            .expect("room");
        match b % 50 {
            10 => mix
                .controller
                .set_lines(b % TRACKS, lines(b, seconds))
                .expect("room"),
            20 => mix
                .controller
                .set_noise(b % TRACKS, Some(&profile))
                .expect("room"),
            30 => {
                mix.controller
                    .rebuild_track(5, full(5), lines(5, seconds), Some(&profile), 4 * BLOCK)
                    .map(|(feed, _)| mix.feeds[5] = feed)
                    .expect("room");
            }
            40 => mix.controller.remove_track(7).expect("room"),
            45 => {
                mix.controller
                    .add_track(7, full(7), lines(7, seconds), None, 4 * BLOCK)
                    .map(|(feed, _)| mix.feeds[7] = feed)
                    .expect("room");
            }
            _ => {}
        }
        mix.feed();
        let ((), n) = os::alloc_count::counted(|| mix.mixer.process(&mut out));
        counted += n;
        // Freed here, on the interface's side.
        mix.controller.collect();
    }
    assert!(out.iter().any(|v| *v != 0.0), "something was mixed");
    assert_eq!(counted, 0, "allocations and frees on the mixing thread");
}
