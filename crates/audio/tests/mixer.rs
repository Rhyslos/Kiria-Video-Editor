//! The mixer as the engine will use it: commands heard within a block,
//! mute and solo, ducking between tracks, dry rings, jumps, swaps, the
//! fixed delay, and the same sound however the device cuts it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use audio::dsp::cleanup;
use audio::dsp::duck::DuckSettings;
use audio::dsp::lines::Lines;
use audio::settings::LimiterSettings;
use audio::test_signal::{Rng, sine};
use audio::{BLOCK, BlockAdapter, Command, MasterChange, MasterSettings, TrackChange, TrackSettings};
use common::{RATE, full, lines, sound};

fn interleave(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect()
}

fn tone(seconds: f32, amplitude: f32) -> Vec<f32> {
    let s = sine(RATE as f32, 1_000.0, amplitude, (RATE as f32 * seconds) as usize);
    interleave(&s, &s)
}

/// Runs `blocks` blocks, returning the interleaved output.
fn run(mixer: &mut audio::Mixer, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut block = [0f32; 2 * BLOCK];
    for _ in 0..blocks {
        mixer.process(&mut block);
        out.extend_from_slice(&block);
    }
    out
}

#[test]
fn the_delay_is_fixed_and_reported() {
    for limiter_on in [false, true] {
        let master = MasterSettings {
            limiter: LimiterSettings {
                on: limiter_on,
                ceiling_db: -1.0,
            },
            ..MasterSettings::default()
        };
        let (mut mixer, mut c, _) = audio::mixer(RATE, 2, master);
        let (mut feed, _) = c
            .add_track(0, TrackSettings::default(), Box::default(), None, 8 * BLOCK)
            .unwrap();
        let mut click = vec![0f32; 2 * 6 * BLOCK];
        click[0] = 0.5;
        click[1] = 0.5;
        feed.write(&click);
        let out = run(&mut mixer, 6);
        let at = out.iter().position(|v| *v != 0.0).unwrap() / 2;
        assert_eq!(at, mixer.latency(), "limiter on: {limiter_on}");
        assert!((out[2 * at] - 0.5).abs() < 1e-6, "and as it went in");
    }
    let (mixer, _, _) = audio::mixer(48_000, 1, MasterSettings::default());
    println!("delay at 48 kHz: {} frames", mixer.latency());
    assert!(mixer.latency() > cleanup::LATENCY);
}

#[test]
fn a_command_is_heard_within_one_block() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 1, MasterSettings::default());
    let (mut feed, _) = c
        .add_track(0, TrackSettings::default(), Box::default(), None, 64 * BLOCK)
        .unwrap();
    feed.write(&tone(0.3, 0.5));
    let master_delay = mixer.latency() - cleanup::LATENCY;
    run(&mut mixer, 20);
    c.change_track(0, TrackChange::Mute(true)).unwrap();
    let out = run(&mut mixer, 4);
    assert!(mixer.track_settings(0).unwrap().mute, "taken at the next block");
    // The block after the command glides down; everything from the next
    // block on is silent (once through the master's fixed delay).
    let silent_from = BLOCK + master_delay;
    assert!(
        out[2 * master_delay..2 * (master_delay + 8)]
            .iter()
            .any(|v| v.abs() > 0.1)
    );
    assert!(out[2 * silent_from..].iter().all(|v| *v == 0.0));
    // The master's volume, the same way.
    c.change_track(0, TrackChange::Mute(false)).unwrap();
    c.change_master(MasterChange::Volume(-20.0)).unwrap();
    run(&mut mixer, 1);
    assert_eq!(mixer.master_settings().volume_db, -20.0);
}

#[test]
fn mute_and_solo_decide_what_is_heard() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 3, MasterSettings::default());
    let mut readouts = Vec::new();
    for slot in 0..3 {
        let (mut feed, readout) = c
            .add_track(slot, TrackSettings::default(), Box::default(), None, 64 * BLOCK)
            .unwrap();
        feed.write(&tone(0.3, 0.25));
        readouts.push(readout);
    }
    c.change_track(1, TrackChange::Solo(true)).unwrap();
    c.change_track(2, TrackChange::Solo(true)).unwrap();
    c.change_track(2, TrackChange::Mute(true)).unwrap();
    run(&mut mixer, 2);
    for r in &readouts {
        r.take_peak();
    }
    run(&mut mixer, 20);
    let peaks: Vec<f32> = readouts.iter().map(|r| r.take_peak()[0]).collect();
    assert_eq!(peaks[0], f32::NEG_INFINITY, "not soloed while another is");
    assert!(peaks[1] > -13.0, "soloed: {}", peaks[1]);
    assert_eq!(peaks[2], f32::NEG_INFINITY, "muted wins over solo");
}

#[test]
fn music_ducks_under_dialogue() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 2, MasterSettings::default());
    let voice = TrackSettings {
        key: true,
        ..TrackSettings::default()
    };
    let music = TrackSettings {
        duck: DuckSettings {
            on: true,
            ..DuckSettings::OFF
        },
        ..TrackSettings::default()
    };
    let (mut v, _) = c.add_track(0, voice, Box::default(), None, 512 * BLOCK).unwrap();
    let (mut m, music_readout) = c.add_track(1, music, Box::default(), None, 512 * BLOCK).unwrap();
    // A second of quiet, then the voice speaks for a second.
    let mut speaking = vec![0f32; RATE as usize * 2];
    speaking.extend(tone(1.0, 0.1));
    v.write(&speaking);
    m.write(&tone(2.0, 0.2));
    run(&mut mixer, 180);
    assert!(music_readout.take().ducked_db > -0.01, "nobody speaking yet");
    run(&mut mixer, 150);
    let ducked = music_readout.take().ducked_db;
    assert!((ducked + 12.0).abs() < 0.2, "under the voice: {ducked}");
    // Its own setting: change the depth while it plays.
    c.change_track(
        1,
        TrackChange::Duck(DuckSettings {
            on: true,
            depth_db: -6.0,
            ..DuckSettings::OFF
        }),
    )
    .unwrap();
    run(&mut mixer, 20);
    assert!((music_readout.take().ducked_db + 6.0).abs() < 0.2);
}

#[test]
fn a_dry_ring_is_silence_owed_to_the_feeder() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 1, MasterSettings::default());
    let (mut feed, _) = c
        .add_track(0, TrackSettings::default(), Box::default(), None, 8 * BLOCK)
        .unwrap();
    feed.write(&tone(0.004, 0.5)); // 192 frames: less than a block
    let out = run(&mut mixer, 3);
    assert!(out.iter().all(|v| v.is_finite()));
    assert_eq!(mixer.dry_frames(), 3 * BLOCK as u64 - 192);
    assert_eq!(
        feed.take_owed(),
        3 * BLOCK as u64 - 192,
        "the feeder skips this much"
    );
    assert_eq!(feed.take_owed(), 0);
}

#[test]
fn a_jump_empties_the_rings_and_says_so() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 1, MasterSettings::default());
    let (mut feed, _) = c
        .add_track(0, TrackSettings::default(), Box::default(), None, 16 * BLOCK)
        .unwrap();
    feed.write(&tone(0.05, 0.5));
    run(&mut mixer, 2);
    assert_eq!(mixer.position(), 2 * BLOCK as u64);
    let number = c.locate(48_000).unwrap();
    assert_ne!(c.located(), number);
    run(&mut mixer, 1);
    assert_eq!(c.located(), number, "done, and said so");
    assert_eq!(mixer.position(), 48_000 + BLOCK as u64);
    assert_eq!(feed.room(), 16 * BLOCK, "emptied");
    // What the block after the jump lacked is owed from the new place on.
    assert_eq!(feed.take_owed(), BLOCK as u64);
}

#[test]
fn what_is_swapped_out_comes_back_to_be_freed() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 2, MasterSettings::default());
    c.add_track(0, TrackSettings::default(), Box::default(), None, BLOCK)
        .unwrap();
    run(&mut mixer, 1);
    assert_eq!(c.collect(), 0, "the slot was empty");
    c.set_lines(0, Box::new(Lines::default())).unwrap();
    c.set_noise(0, None).unwrap();
    c.add_track(0, TrackSettings::default(), Box::default(), None, BLOCK)
        .unwrap();
    c.remove_track(0).unwrap();
    run(&mut mixer, 1);
    assert_eq!(c.collect(), 4, "old lines, old noise, two tracks");
    assert!(mixer.track_settings(0).is_none());
    assert!(
        c.add_track(9, TrackSettings::default(), Box::default(), None, BLOCK)
            .is_err(),
        "no such slot"
    );
}

#[test]
fn a_rebuilt_track_carries_on_without_a_jump() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 1, MasterSettings::default());
    let settings = full(20);
    let (mut feed, _) = c.add_track(0, settings, lines(0, 2), None, 64 * BLOCK).unwrap();
    let (l, r) = sound(20, RATE as usize);
    let all = interleave(&l, &r);
    feed.write(&all[..2 * 32 * BLOCK]);
    let before = run(&mut mixer, 32);
    let (mut feed, _) = c
        .rebuild_track(0, settings, lines(0, 2), None, 64 * BLOCK)
        .unwrap();
    feed.write(&all[2 * 32 * BLOCK..2 * 64 * BLOCK]);
    let after = run(&mut mixer, 32);
    // A cut-off sound would jump to the new strip's silence: carried over,
    // the step across the swap is like any other.
    let left: Vec<f32> = before.iter().step_by(2).copied().collect();
    let across = (after[0] - left[left.len() - 1]).abs();
    let typical = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
    assert!(across <= typical * 1.5 + 1e-3, "{across} against {typical}");
}

/// The mix of four tracks with every effect and lines on, through device
/// buffers of `frames`, interleaved.
fn through_device(frames: usize, seconds: usize) -> Vec<f32> {
    let total = RATE as usize * seconds;
    let (mut mixer, mut c, _) = audio::mixer(RATE, 4, common::master());
    for (slot, t) in [0, 9, 17, 25].into_iter().enumerate() {
        let (mut feed, _) = c
            .add_track(slot, full(t), lines(t, seconds), None, total + BLOCK)
            .unwrap();
        let (l, r) = sound(t, total);
        feed.write(&interleave(&l, &r));
    }
    let mut adapter = BlockAdapter::default();
    let mut out = vec![0f32; 2 * total];
    for buffer in out.chunks_mut(2 * frames) {
        adapter.render(&mut mixer, buffer, 2);
    }
    out
}

#[test]
fn the_same_sound_however_the_device_cuts_it() {
    let whole = through_device(BLOCK, 1);
    assert!(whole.iter().any(|v| v.abs() > 0.01));
    for frames in [128, 441, 480, 1024] {
        assert!(through_device(frames, 1) == whole, "{frames}-frame buffers");
    }
}

#[test]
fn the_adapter_fills_any_channel_count() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 1, MasterSettings::default());
    let (mut feed, _) = c
        .add_track(0, TrackSettings::default(), Box::default(), None, 64 * BLOCK)
        .unwrap();
    let mut rng = Rng::new(1);
    let noise: Vec<f32> = (0..2 * 32 * BLOCK).map(|_| rng.signed() * 0.5).collect();
    feed.write(&noise);
    let mut adapter = BlockAdapter::default();
    let mut six = vec![1f32; 6 * 3 * BLOCK];
    adapter.render(&mut mixer, &mut six, 6);
    assert!(six.chunks(6).all(|f| f[2..].iter().all(|v| *v == 0.0)));
    let mut mono = vec![0f32; 3 * BLOCK];
    adapter.render(&mut mixer, &mut mono, 1);
    assert!(mono.iter().any(|v| *v != 0.0));
}

#[test]
fn the_limiter_holds_its_ceiling_on_a_loud_mix() {
    let (mut mixer, mut c, _) = audio::mixer(RATE, 4, common::master());
    for slot in 0..4 {
        let (mut feed, _) = c
            .add_track(slot, TrackSettings::default(), Box::default(), None, 200 * BLOCK)
            .unwrap();
        let mut rng = Rng::new(slot as u64);
        let loud: Vec<f32> = (0..2 * 200 * BLOCK).map(|_| rng.signed() * 2.0).collect();
        feed.write(&loud);
    }
    run(&mut mixer, 200);
    let levels = c.master().levels();
    assert!(levels.true_peak_max <= -1.0, "{levels:?}");
    assert!(c.master().limiter_db() < -6.0);
}

#[test]
fn the_analyser_listens_to_the_chosen_track() {
    let (mut mixer, mut c, mut analyser) = audio::mixer(RATE, 2, MasterSettings::default());
    let (mut feed, _) = c
        .add_track(1, TrackSettings::default(), Box::default(), None, 64 * BLOCK)
        .unwrap();
    feed.write(&tone(0.3, 0.5));
    run(&mut mixer, 16);
    assert!(!analyser.step(), "nobody chosen");
    c.send(Command::Analyse(Some(1))).unwrap();
    run(&mut mixer, 16);
    assert!(analyser.step());
    let band = audio::dsp::bands::band_of(1_000.0);
    assert!(analyser.spectrum().levels()[band] > -12.0);
}
