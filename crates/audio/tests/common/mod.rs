//! A full mix for the mixer's tests: 32 tracks (8 dialogue, 8 music, 8
//! effects, 8 ambience) with every effect on, fed from made-up sound.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, dead_code)]

use audio::dsp::cleanup::{CleanupSettings, NoiseProfile};
use audio::dsp::duck::DuckSettings;
use audio::dsp::dynamics::Preset;
use audio::dsp::filters::FLAT;
use audio::dsp::level::LevelSettings;
use audio::dsp::lines::{Line, Lines};
use audio::settings::{EqSettings, HighPassSettings, LimiterSettings};
use audio::test_signal::{Rng, speech};
use audio::{BLOCK, Controller, FeedWriter, MasterSettings, Mixer, TrackSettings};

pub const RATE: u32 = 48_000;
pub const TRACKS: usize = 32;

/// Stereo sound for track `t`, `n` frames long.
pub fn sound(t: usize, n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut rng = Rng::new(t as u64 + 1);
    let rate = RATE as f32;
    let hiss = |rng: &mut Rng, level: f32| -> Vec<f32> { (0..n).map(|_| rng.signed() * level).collect() };
    match t / 8 {
        0 => {
            let (voice, _) = speech(rate, n, t as u64 + 7);
            let (hl, hr) = (hiss(&mut rng, 0.003), hiss(&mut rng, 0.003));
            (
                voice.iter().zip(&hl).map(|(v, h)| v + h).collect(),
                voice.iter().zip(&hr).map(|(v, h)| 0.9 * v + h).collect(),
            )
        }
        1 => {
            let root = 110.0 * (1.0 + t as f32 / 16.0);
            let l: Vec<f32> = (0..n)
                .map(|i| {
                    let x = i as f32 / rate;
                    [1.0, 1.26, 1.5, 2.0]
                        .iter()
                        .map(|k| 0.06 * (2.0 * std::f32::consts::PI * root * k * x).sin())
                        .sum()
                })
                .collect();
            (l.clone(), l)
        }
        2 => {
            let mut l = vec![0f32; n];
            let mut r = vec![0f32; n];
            let mut at = 0;
            while at < n {
                at += ((0.2 + 0.8 * rng.unit()) * rate) as usize;
                let loud = if rng.unit() < 0.3 { 0.95 } else { 0.3 };
                for i in at..(at + RATE as usize / 2).min(n) {
                    let e = loud * (-((i - at) as f32 / rate) * 12.0).exp();
                    l[i] = rng.signed() * e;
                    r[i] = rng.signed() * e;
                }
            }
            (l, r)
        }
        _ => (hiss(&mut rng, 0.05), hiss(&mut rng, 0.05)),
    }
}

/// Every effect on: dialogue is the key; everything else ducks under it.
pub fn full(t: usize) -> TrackSettings {
    let dialogue = t < 8;
    let mut bands = FLAT;
    for (i, b) in bands.iter_mut().enumerate() {
        b.gain_db = [-2.0, -3.0, 1.0, 2.0, -1.5, 1.5][i];
    }
    TrackSettings {
        key: dialogue,
        high_pass: HighPassSettings {
            on: true,
            freq: if dialogue { 80.0 } else { 30.0 },
        },
        eq: EqSettings { on: true, bands },
        compressor: Preset::Voice.settings(),
        cleanup: CleanupSettings {
            on: true,
            ..CleanupSettings::OFF
        },
        duck: DuckSettings {
            on: !dialogue,
            ..DuckSettings::OFF
        },
        level: LevelSettings {
            on: true,
            ..LevelSettings::OFF
        },
        volume_db: -6.0,
        pan: (t as f32 / 16.0) - 1.0,
        mute: false,
        solo: false,
    }
}

/// A volume and a pan line with a point every half second.
pub fn lines(t: usize, seconds: usize) -> Box<Lines> {
    let mut rng = Rng::new(t as u64 + 999);
    let step = u64::from(RATE / 2);
    let points = seconds as u64 * 2 + 2;
    Box::new(Lines {
        volume_db: Line::new(
            (0..points)
                .map(|i| (i * step, -6.0 + 4.0 * rng.signed()))
                .collect(),
        ),
        pan: Line::new((0..points).map(|i| (i * step, 0.5 * rng.signed())).collect()),
    })
}

/// The master as it would be for a finished mix: compressor and limiter on.
pub fn master() -> MasterSettings {
    MasterSettings {
        volume_db: 0.0,
        compressor: Preset::Gentle.settings(),
        limiter: LimiterSettings {
            on: true,
            ceiling_db: -1.0,
        },
    }
}

/// A mixer with every track added and the first block's swaps taken.
pub struct Mix {
    pub mixer: Mixer,
    pub controller: Controller,
    pub feeds: Vec<FeedWriter>,
    pub sounds: Vec<(Vec<f32>, Vec<f32>)>,
    pub fed: usize,
}

pub fn full_mix(seconds: usize) -> Mix {
    let n = RATE as usize * seconds;
    let (mixer, mut controller, _) = audio::mixer(RATE, TRACKS, master());
    let noise = NoiseProfile::learn(
        RATE,
        &(0..RATE as usize)
            .map(|i| ((i * 7919 % 2003) as f32 / 2003.0 - 0.5) * 0.006)
            .collect::<Vec<_>>(),
        &(0..RATE as usize)
            .map(|i| ((i * 6007 % 1999) as f32 / 1999.0 - 0.5) * 0.006)
            .collect::<Vec<_>>(),
    );
    let mut feeds = Vec::new();
    for t in 0..TRACKS {
        let (feed, _) = controller
            .add_track(t, full(t), lines(t, seconds), Some(&noise), 4 * BLOCK)
            .expect("room");
        feeds.push(feed);
    }
    let sounds = (0..TRACKS).map(|t| sound(t, n + BLOCK)).collect();
    Mix {
        mixer,
        controller,
        feeds,
        sounds,
        fed: 0,
    }
}

impl Mix {
    /// Writes the next block of every track's sound into its ring (the
    /// feeder's work, outside what is measured).
    pub fn feed(&mut self) {
        let mut block = [0f32; 2 * BLOCK];
        for (feed, (l, r)) in self.feeds.iter_mut().zip(&self.sounds) {
            let at = self.fed % (l.len() - BLOCK);
            for i in 0..BLOCK {
                block[2 * i] = l[at + i];
                block[2 * i + 1] = r[at + i];
            }
            feed.write(&block);
        }
        self.fed += BLOCK;
    }
}
