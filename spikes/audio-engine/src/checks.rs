//! Checks that the effects do what they say and the designs agree.

use crate::bench::{self, build, full_gate, profile, run};
use crate::dsp::dynamics::{CompSettings, Compressor};
use crate::dsp::limiter::Limiter;
use crate::dsp::meter::TruePeak;
use crate::graph::{Parts, chain_a, chain_b, chain_c};
use crate::params::{self, Way};
use crate::settings::{Clean, PROBE, TrackSettings};
use crate::signal::{self, RATE, Rng};
use std::sync::Arc;

fn signals() -> Arc<Vec<signal::Signal>> {
    Arc::new(signal::all((RATE * 2.0) as usize))
}

#[test]
fn the_three_designs_sound_exactly_the_same_and_never_allocate() {
    let s = signals();
    let p = profile();
    let a = run(&mut build(&chain_a, &full_gate, &s, &p, None, true), 256);
    let b = run(&mut build(&chain_b, &full_gate, &s, &p, None, true), 256);
    let c = run(&mut build(&chain_c, &full_gate, &s, &p, None, true), 256);
    assert!(a.sum > 0.0);
    assert_eq!(a.sum, b.sum);
    assert_eq!(a.sum, c.sum);
    assert_eq!(a.allocs + b.allocs + c.allocs, 0);
}

#[test]
fn the_master_never_goes_over_the_true_peak_ceiling() {
    let s = signals();
    let r = run(&mut build(&chain_c, &full_gate, &s, &profile(), None, true), 512);
    assert!(r.tp_max_db <= -0.9, "true peak {}", r.tp_max_db);
}

#[test]
fn the_limiter_holds_a_loud_noise_under_minus_one() {
    let mut lim = Limiter::new(RATE, -1.0, 50.0);
    let mut rng = Rng::new(3);
    let mut l: Vec<f32> = (0..48_000).map(|_| rng.s() * 2.0).collect();
    let mut r: Vec<f32> = (0..48_000).map(|_| rng.s() * 2.0).collect();
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        lim.process(a, b);
    }
    let mut tp = TruePeak::new();
    let peak = l
        .iter()
        .zip(&r)
        .map(|(&a, &b)| tp.push(a, b))
        .fold(0f32, f32::max);
    assert!(20.0 * peak.log10() <= -0.9, "{}", 20.0 * peak.log10());
}

#[test]
fn the_compressor_turns_a_steady_tone_down_by_the_ratio() {
    let s = CompSettings {
        threshold_db: -20.0,
        ratio: 4.0,
        attack_ms: 1.0,
        release_ms: 50.0,
        knee_db: 0.0,
        makeup_db: 0.0,
    };
    let mut c = Compressor::new(RATE, s);
    let amp = 10f32.powf(-6.0 / 20.0);
    let mut l: Vec<f32> = (0..24_000)
        .map(|i| if i / 50 % 2 == 0 { amp } else { -amp })
        .collect();
    let mut r = l.clone();
    c.process(&mut l, &mut r);
    // 14 dB over, at 4:1 comes out 3.5 dB over: 10.5 dB down.
    println!("compressor: {:.2} dB", c.gr_meter);
    assert!((c.gr_meter + 10.5).abs() < 0.3, "{}", c.gr_meter);
}

#[test]
fn the_gate_takes_the_hiss_out_of_pauses_and_keeps_the_words() {
    let (pause_drop, word_drop) = bench::cleanup_quality(0.003, true);
    println!("gate: pauses down {pause_drop:.1} dB, words down {word_drop:.2} dB");
    assert!(pause_drop > 12.0, "pauses down {pause_drop:.1} dB");
    assert!(word_drop < 1.0, "words down {word_drop:.1} dB");
}

#[test]
fn settings_survive_the_flat_list() {
    for t in [0, 8] {
        for s in [bench::full_gate(t), bench::full_expand(t), TrackSettings::none()] {
            assert_eq!(TrackSettings::from_array(&s.to_array()), s);
        }
    }
    let mut s = TrackSettings::none();
    s.clean = Clean::Off;
    assert_eq!(TrackSettings::from_array(&s.to_array()), s);
}

#[test]
fn every_way_delivers_a_change_without_allocating_on_the_real_time_side() {
    let sig = signals();
    let p = profile();
    for way in Way::ALL {
        let settings: Vec<TrackSettings> = (0..bench::TRACKS).map(full_gate).collect();
        let parts: Vec<Parts> = (0..bench::TRACKS)
            .map(|t| Parts {
                profile: p.clone(),
                fader: crate::graph::automation(t, 2.0),
                tap: None,
            })
            .collect();
        let (mut tx, mut rx) = params::make(way, &settings, &parts);
        let mut g = build(&|s, p| Box::new(chain_c(s, p)), &full_gate, &sig, &p, None, true);
        tx.change(3, PROBE, 1234.0);
        crate::alloc::reset();
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        crate::alloc::on_rt(|| {
            rx.poll(&mut g);
            g.process(&mut l, &mut r);
        });
        assert_eq!(crate::alloc::count(), 0, "{way:?}");
        assert_eq!(g.tracks[3].chain.settings.eq[2].freq, 1234.0, "{way:?}");
        tx.collect();
    }
}
