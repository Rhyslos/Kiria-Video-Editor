//! A nested timeline's sound in playback and export: exactly that timeline
//! as it sounds opened (its tracks' effects and its master), then the clip
//! and the track it is on.

use super::*;

/// The recording nested whole: the outer timeline (the active one) plays
/// one nested clip, the inner one the recording's clips. Returns the
/// project and the inner timeline.
fn nested(p: Project) -> (Project, SequenceId) {
    let seq = p.active_sequence;
    let picture = p.active().unwrap().video_tracks[0].clips[0].id;
    let p = Command::NestClips {
        sequence: seq,
        clips: vec![picture],
        name: "Inner".into(),
    }
    .apply(&p)
    .unwrap();
    let inner = played_by(&p, seq);
    (p, inner)
}

/// The timeline the sound clip of `seq` plays.
fn played_by(p: &Project, seq: SequenceId) -> SequenceId {
    p.sequence(seq)
        .unwrap()
        .audio_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find_map(project::Clip::nested)
        .unwrap()
}

/// `frames` of `seq` exported at `rate` from `from`, through its own master.
fn export(p: &Project, seq: SequenceId, rate: u32, from: Time, frames: usize) -> Vec<f32> {
    let master = MasterSettings::from_project(p.sequence(seq).unwrap());
    let mut mix = OfflineMix::new(p, seq, rate, from, master).unwrap();
    let mut out = Vec::new();
    mix.take(frames, &mut out);
    out
}

fn assert_same(a: &[f32], b: &[f32], what: &str) {
    assert!(loudness(a) > 0.01, "{what}: it sounds");
    assert!(a == b, "{what}: off by up to {}", most_apart(a, b));
}

/// Effects on every track and the master of the nested timeline, the
/// outer clip and track left plain: the outer timeline exported is that
/// timeline exported on its own, sample for sample, and its effects are
/// heard (it differs from the same timeline with them off).
#[test]
fn a_nested_timeline_sounds_as_it_does_opened_effects_and_master_included() {
    let (plain, inner) = nested(recording());
    let p = effects_on(plain.clone(), inner);
    let from = Time::from_ratio(1, 4).unwrap();
    let outer = export(&p, p.active_sequence, RATE, from, RATE as usize);
    let alone = export(&p, inner, RATE, from, RATE as usize);
    assert_same(&outer, &alone, "nested and opened");
    let without = export(&plain, plain.active_sequence, RATE, from, RATE as usize);
    assert!(most_apart(&outer, &without) > 0.01, "the effects are heard");
}

/// With every effect off, a nested timeline sounds exactly as it did when
/// it was mixed the old way (the timeline's clips, its tracks' volume, mute
/// and its master volume summed by `mix::Mixer::mix`, a piece at a time),
/// to the bit.
#[test]
fn with_every_effect_off_a_nested_timeline_is_exactly_as_before() {
    let (mut p, inner) = nested(recording());
    {
        let seq = p.sequences.iter_mut().find(|s| s.id == inner).unwrap();
        seq.audio_tracks[0].gain_db = -7.5;
        seq.audio_tracks[1].gain_db = 3.0;
        seq.audio_tracks[2].muted = true;
        seq.master_gain_db = -2.0;
    }
    let start = Time::from_ratio(1, 3).unwrap();
    let frames = RATE as usize * 3 / 2;
    let mut before = vec![0f32; frames * 2];
    let mut old = crate::mix::Mixer::new(RATE);
    let seq = p.sequence(inner).unwrap();
    for (n, chunk) in before.chunks_mut(feeder::PIECE * 2).enumerate() {
        let at = start.as_secs_f64() + (n * feeder::PIECE) as f64 / f64::from(RATE);
        old.mix(&p, seq, at, chunk);
    }
    let now = export(&p, p.active_sequence, RATE, start, frames);
    assert_same(&now, &before, "now and before");
}

/// At half speed the nested timeline is mixed at twice the rate and played
/// at the outer's (tape-style, as a file is): the outer timeline exported
/// is the nested one exported at 96 kHz, effects included, from half as far
/// in.
#[test]
fn at_half_speed_a_nested_timeline_is_mixed_at_twice_the_rate() {
    let (p, inner) = nested(recording());
    let p = effects_on(p, inner);
    let seq = p.active_sequence;
    let clip = p.active().unwrap().video_tracks[0].clips[0].id;
    let p = Command::SetSpeed {
        sequence: seq,
        clip,
        speed: project::Speed::new(1, 2).unwrap(),
    }
    .apply(&p)
    .unwrap();
    let sound = &p.active().unwrap().audio_tracks[0].clips[0];
    assert_eq!(
        sound.speed,
        project::Speed::new(1, 2).unwrap(),
        "the sound follows"
    );
    let outer = export(&p, seq, RATE, Time::from_ratio(1, 2).unwrap(), RATE as usize);
    let alone = export(
        &p,
        inner,
        2 * RATE,
        Time::from_ratio(1, 4).unwrap(),
        RATE as usize,
    );
    assert_same(&outer, &alone, "half speed");
}

/// Two levels: the outer timeline plays a middle one (effects on its
/// track and master) that plays the inner one (effects on all of it). The
/// outer is the middle exported on its own, and the middle hears the inner
/// with its effects.
#[test]
fn two_levels_of_nesting_each_sound_as_opened() {
    let (p, inner) = nested(recording());
    let p = effects_on(p, inner);
    let outer = p.active_sequence;
    let clip = p.active().unwrap().video_tracks[0].clips[0].id;
    let p = Command::NestClips {
        sequence: outer,
        clips: vec![clip],
        name: "Middle".into(),
    }
    .apply(&p)
    .unwrap();
    let middle = played_by(&p, outer);
    assert_eq!(played_by(&p, middle), inner);
    let mut sound = TrackSound::default();
    sound.eq.on = true;
    sound.eq.bands[0].gain_mdb = 9_000;
    sound.compressor.on = true;
    let track = p.sequence(middle).unwrap().audio_tracks[0].id;
    let p = Command::SetTrackSound {
        sequence: middle,
        track,
        sound: Box::new(sound),
    }
    .apply(&p)
    .unwrap();
    let mut master = MasterSound::default();
    master.limiter.on = true;
    master.limiter.ceiling_mdb = -12_000;
    let p = Command::SetMasterSound {
        sequence: middle,
        sound: master,
    }
    .apply(&p)
    .unwrap();
    let from = Time::from_ratio(1, 4).unwrap();
    let frames = RATE as usize;
    let heard = export(&p, outer, RATE, from, frames);
    let opened = export(&p, middle, RATE, from, frames);
    assert_same(&heard, &opened, "outer and middle");
    let innermost = export(&p, inner, RATE, from, frames);
    assert!(
        most_apart(&opened, &innermost) > 0.01,
        "the middle's effects are heard"
    );
}

/// Played from the start with a jump into the middle: once the outer
/// mixer's fixed delay has passed, the sound is the export from where it
/// jumped to, sample for sample: the nested timeline's mixer went there
/// afresh, its effects and delay too.
#[test]
fn a_jump_into_the_middle_of_a_nested_timeline() {
    let (p, inner) = nested(recording());
    let p = effects_on(p, inner);
    let mut fake = fake(&p, Time::ZERO);
    let latency = fake.output.latency();
    fake.play(RATE as usize / 2, 480);
    // A jump, as the audio thread makes it, without waiting.
    let to = Time::seconds(1);
    let sample = sample_at(to, RATE);
    fake.clock.output.primed.store(false, Ordering::Release);
    fake.session.controller.locate(sample).unwrap();
    fake.output.render(&mut [0.0; 480 * 2], 2);
    fake.session.feeder.jump(sample, to.as_secs_f64());
    fake.session.feeder.prime(RATE as usize / 10, &|| true);
    fake.clock.output.primed.store(true, Ordering::Release);
    let played = fake.play(latency + RATE as usize / 2, 480);
    let exported: Vec<f32> = export(&p, p.active_sequence, RATE, to, RATE as usize / 2)
        .iter()
        .map(|s| s.clamp(-1.0, 1.0))
        .collect();
    assert_same(&played[latency * 2..], &exported, "after the jump");
}

/// Playback of a nested timeline with effects is its export, sample for
/// sample, and the sound card's callback never allocates or frees while
/// it plays (the nested mixing is the feeder's, on the audio thread).
#[test]
fn playing_a_nested_timeline_is_its_export_and_the_callback_never_allocates() {
    let (p, inner) = nested(recording());
    let p = effects_on(p, inner);
    let from = Time::from_ratio(1, 4).unwrap();
    let mut fake = fake(&p, from);
    let latency = fake.output.latency();
    let mut played = Vec::new();
    let mut buf = vec![0f32; 441 * 2];
    let mut counted = 0;
    while played.len() < (RATE as usize + latency) * 2 {
        fake.session.step(&fake.inbox, &fake.clock);
        let ((), allocations) = os::alloc_count::counted(|| fake.output.render(&mut buf, 2));
        counted += allocations;
        played.extend_from_slice(&buf);
    }
    assert_eq!(counted, 0, "allocations or frees in the callback");
    let exported: Vec<f32> = export(&p, p.active_sequence, RATE, from, RATE as usize)
        .iter()
        .map(|s| s.clamp(-1.0, 1.0))
        .collect();
    let heard = &played[latency * 2..(latency + RATE as usize) * 2];
    assert_same(heard, &exported, "playback and export");
    assert_eq!(fake.output.dry_frames(), 0);
}

/// An edit to the nested timeline's tracks while the outer one plays is
/// heard from what is mixed next: its tracks muted, it falls silent once
/// what was already in the rings has played.
#[test]
fn an_edit_to_the_nested_timeline_while_playing_is_heard() {
    let (p, inner) = nested(recording());
    let mut fake = fake(&p, Time::ZERO);
    let latency = fake.output.latency();
    assert!(loudness(&fake.play(RATE as usize / 4, 480)) > 0.01);
    let mut muted = p.clone();
    let seq = muted.sequences.iter_mut().find(|s| s.id == inner).unwrap();
    for t in seq.audio_tracks.iter_mut() {
        t.muted = true;
    }
    fake.update(&muted);
    // The outer ring's half second and the delay play out first.
    fake.play(RATE as usize / 2 + latency + feeder::PIECE, 480);
    let after = fake.play(RATE as usize / 4, 480);
    assert!(after.iter().all(|s| *s == 0.0), "silent");
}

/// What a nested track costs the feeder, against exporting the timeline
/// it plays: the first piece (its mixer made, its files opened, the delay
/// mixed ahead) and each piece after. Run by hand with `--ignored
/// --nocapture`; the numbers are in the engine's README.
#[test]
#[ignore = "a measurement, by hand"]
fn cost_of_a_nested_track() {
    let (p, inner) = nested(recording());
    let p = effects_on(p, inner);
    // Four seconds: the clips' two and silence after them.
    let pieces = 200;
    let track = &p.active().unwrap().audio_tracks[0];
    let mut mixer = crate::mix::Mixer::new(RATE);
    let mut piece = vec![0f32; feeder::PIECE * 2];
    let started = Instant::now();
    mixer.mix_track(&p, track, 0.0, &mut piece);
    let first = started.elapsed();
    let started = Instant::now();
    for n in 1..=pieces {
        mixer.mix_track(
            &p,
            track,
            (n * feeder::PIECE) as f64 / f64::from(RATE),
            &mut piece,
        );
    }
    let each = started.elapsed() / pieces as u32;
    let started = Instant::now();
    let master = MasterSettings::from_project(p.sequence(inner).unwrap());
    let mut alone = OfflineMix::new(&p, inner, RATE, Time::ZERO, master).unwrap();
    let mut out = Vec::new();
    alone.take(feeder::PIECE, &mut out);
    let first_alone = started.elapsed();
    let started = Instant::now();
    for _ in 0..pieces {
        alone.take(feeder::PIECE, &mut out);
    }
    let each_alone = started.elapsed() / pieces as u32;
    println!(
        "a piece of {} frames: nested {each:?} (first {first:?}), the timeline exported {each_alone:?} (first {first_alone:?})",
        feeder::PIECE
    );
}
