use super::*;
use audio::dsp::biquad::{Biquad, Coefs};
use audio::test_signal::{Rng, sine, speech};
use project::track_sound::TrackRole;
use project::{Command, TrackKind};
use std::collections::HashMap;
use std::path::Path;

const RATE: u32 = 48_000;
const SECONDS: usize = 8;

/// Made-up sound per track, the same on both sides.
#[derive(Debug, Clone, Default)]
struct Made(HashMap<TrackId, Vec<f32>>);

impl DrySound for Made {
    fn fill(&mut self, _: &Project, track: &project::Track, at: u64, out: &mut [f32]) {
        let sound = self.0.get(&track.id);
        for (i, frame) in out.chunks_exact_mut(2).enumerate() {
            let v = sound.and_then(|s| s.get(at as usize + i)).copied().unwrap_or(0.0);
            frame[0] = v;
            frame[1] = v;
        }
    }
}

/// A project with an audio track per role, named "A1", "A2", ...
fn tracks(roles: &[TrackRole]) -> (Project, SequenceId, Vec<TrackId>) {
    let mut p = Project::new_automatic("T");
    let seq = p.active_sequence;
    while p.sequence(seq).unwrap().audio_tracks.len() < roles.len() {
        let name = format!("A{}", p.sequence(seq).unwrap().audio_tracks.len() + 1);
        p = Command::AddTrack {
            sequence: seq,
            kind: TrackKind::Audio,
            name,
        }
        .apply(&p)
        .unwrap();
    }
    let ids: Vec<TrackId> = p
        .sequence(seq)
        .unwrap()
        .audio_tracks
        .iter()
        .map(|t| t.id)
        .collect();
    for (id, role) in ids.iter().zip(roles) {
        p = Command::SetTrackSound {
            sequence: seq,
            track: *id,
            sound: Box::new(project::TrackSound {
                role: *role,
                ..Default::default()
            }),
        }
        .apply(&p)
        .unwrap();
    }
    assert_eq!(p.sequence(seq).unwrap().settings.sample_rate, RATE);
    (p, seq, ids)
}

fn n() -> usize {
    SECONDS * RATE as usize
}

fn voice() -> Vec<f32> {
    speech(RATE as f32, n(), 7).0
}

fn pink(seed: u64, gain: f32) -> Vec<f32> {
    // White noise split into octaves (the difference of one-pole low
    // passes an octave apart), each turned down 3 dB more than the one
    // below: about equal power per octave from 40 Hz to 20 kHz.
    let mut rng = Rng::new(seed);
    let poles: Vec<f32> = (0..10)
        .map(|i| (-std::f32::consts::TAU * 40.0 * 2f32.powi(i) / RATE as f32).exp())
        .collect();
    let mut state = vec![0f32; poles.len()];
    (0..n())
        .map(|_| {
            let w = rng.signed();
            for (s, a) in state.iter_mut().zip(&poles) {
                *s = a * *s + (1.0 - a) * w;
            }
            let octaves: f32 = state
                .windows(2)
                .enumerate()
                .map(|(i, s)| (s[1] - s[0]) * 0.5f32.powf(i as f32 / 2.0))
                .sum();
            octaves * gain * 8.0
        })
        .collect()
}

fn tones(freqs: &[f32], amplitude: f32) -> Vec<f32> {
    let mut out = vec![0f32; n()];
    for f in freqs {
        for (o, s) in out.iter_mut().zip(sine(RATE as f32, *f, amplitude, n())) {
            *o += s;
        }
    }
    out
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

fn scaled(a: &[f32], gain: f32) -> Vec<f32> {
    a.iter().map(|x| x * gain).collect()
}

/// Sound only between `from` and `to` seconds.
fn between(a: &[f32], from: f32, to: f32) -> Vec<f32> {
    let r = RATE as f32;
    a.iter()
        .enumerate()
        .map(|(i, x)| {
            if (from * r..to * r).contains(&(i as f32)) {
                *x
            } else {
                0.0
            }
        })
        .collect()
}

/// Something like music: a wide chord and a bed of noise.
fn music() -> Vec<f32> {
    let chord = tones(
        &[
            196.0, 247.0, 294.0, 392.0, 494.0, 587.0, 784.0, 988.0, 1_175.0, 1_568.0, 1_976.0, 2_349.0,
            3_136.0,
        ],
        0.008,
    );
    add(&chord, &pink(3, 0.07))
}

fn band_passed(a: &[f32], freq: f32, q: f32) -> Vec<f32> {
    let mut f = Biquad::new(Coefs::band_pass(RATE as f32, freq, q));
    a.iter().map(|x| f.tick(0, *x)).collect()
}

fn check(p: &Project, seq: SequenceId, sound: &Made, request: Request) -> Report {
    analyse(p, seq, request, &mut sound.clone(), &|| true, &|_| {})
        .unwrap()
        .unwrap()
}

fn whole() -> Request {
    Request::Region(Time::ZERO, Time::seconds(SECONDS as i64))
}

fn secs(t: Time) -> f64 {
    t.as_secs_f64()
}

/// Voice on A1, music on A2, the music between 2 and 6 s at `gain`.
fn music_over_voice(gain: f32) -> (Project, SequenceId, Vec<TrackId>, Made) {
    let (p, seq, ids) = tracks(&[TrackRole::Dialogue, TrackRole::Music]);
    let mut made = Made::default();
    made.0.insert(ids[0], voice());
    made.0.insert(ids[1], scaled(&between(&music(), 2.0, 6.0), gain));
    (p, seq, ids, made)
}

#[test]
fn music_over_dialogue_is_found_in_the_right_band_and_time() {
    let (p, seq, ids, made) = music_over_voice(1.0);
    let report = check(&p, seq, &made, whole());
    for f in &report.findings {
        println!(
            "{:?} {:.2}-{:.2} s {:?} {:.2} {}",
            f.problem,
            secs(f.range.0),
            secs(f.range.1),
            f.band_hz,
            f.severity,
            f.words
        );
    }
    let covers: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.problem == Problem::CoversVoice)
        .collect();
    assert!(!covers.is_empty(), "{:#?}", report.findings);
    for f in &covers {
        assert_eq!(f.tracks, vec![ids[1]]);
        assert!(secs(f.range.0) >= 1.6 && secs(f.range.1) <= 6.4, "{:?}", f.range);
        assert!(f.band_hz.0 >= 315 && f.band_hz.1 <= 4_000, "{:?}", f.band_hz);
        assert!(
            f.words.starts_with("The music on A2 covers the voice "),
            "{}",
            f.words
        );
        assert!(
            f.fixes
                .iter()
                .any(|x| matches!(x.remedy, Remedy::LowerUnderDialogue { .. }))
        );
    }
    let covered: f64 = covers.iter().map(|f| secs(f.range.1) - secs(f.range.0)).sum();
    assert!(covered >= 2.5, "most of the 4 s with music: {covered:.2} s");
    assert!(
        covers
            .iter()
            .any(|f| f.fixes.iter().any(|x| x.remedy == Remedy::MakeRoomForVoice)),
        "the music covers 2 to 3 kHz too"
    );
    let levels = &report.tracks;
    assert_eq!(levels.len(), 2);
    assert_eq!(levels[0].track, ids[0], "dialogue first");
}

#[test]
fn a_clean_mix_gives_nothing() {
    let (p, seq, _, made) = music_over_voice(0.05);
    let report = check(&p, seq, &made, whole());
    assert!(report.findings.is_empty(), "{:#?}", report.findings);
    // Loud music on a muted track isn't heard, so it covers nothing.
    let (p, seq, ids, made) = music_over_voice(1.0);
    let muted = Command::SetTrackMix {
        sequence: seq,
        track: ids[1],
        muted: true,
        solo: false,
        gain_db: 0.0,
    }
    .apply(&p)
    .unwrap();
    let report = check(&muted, seq, &made, whole());
    assert!(report.findings.is_empty(), "{:#?}", report.findings);
    assert_eq!(report.tracks.len(), 1);
}

/// Three tracks heavy between 200 and 500 Hz, with a little above.
fn pile_up() -> (Project, SequenceId, Vec<TrackId>, Made) {
    let (p, seq, ids) = tracks(&[TrackRole::Music, TrackRole::Ambience, TrackRole::Effects]);
    let mut made = Made::default();
    let air = |seed| pink(seed, 0.15);
    made.0
        .insert(ids[0], add(&tones(&[220.0, 277.0, 330.0, 415.0], 0.08), &air(11)));
    made.0.insert(
        ids[1],
        add(&scaled(&band_passed(&pink(12, 1.0), 300.0, 1.0), 0.4), &air(13)),
    );
    made.0
        .insert(ids[2], add(&tones(&[250.0, 375.0, 440.0], 0.08), &air(14)));
    (p, seq, ids, made)
}

#[test]
fn a_pile_up_between_200_and_500_hz_is_found() {
    let (p, seq, ids, made) = pile_up();
    let report = check(&p, seq, &made, whole());
    for f in &report.findings {
        println!(
            "{:?} {:.2}-{:.2} s {:.2} {}",
            f.problem,
            secs(f.range.0),
            secs(f.range.1),
            f.severity,
            f.words
        );
    }
    let mud: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.problem == Problem::Muddy)
        .collect();
    assert_eq!(mud.len(), 1, "{:#?}", report.findings);
    let f = mud[0];
    assert_eq!(f.band_hz, (200, 500));
    assert!(secs(f.range.1) - secs(f.range.0) > 6.0, "{:?}", f.range);
    let mut involved = f.tracks.clone();
    involved.sort();
    assert_eq!(involved, ids);
    assert!(f.words.contains("pile up between 200 and 500 Hz"), "{}", f.words);
    assert!(
        f.severity > 0.2 && f.severity < 0.95,
        "room to get better: {}",
        f.severity
    );
    assert!(f.fixes.iter().all(|x| x.remedy == Remedy::CutLowMiddle) && !f.fixes.is_empty());
    // Two tracks alone are no pile-up.
    let (p2, seq2, ids2) = tracks(&[TrackRole::Music, TrackRole::Ambience]);
    let mut two = Made::default();
    two.0.insert(ids2[0], made.0[&ids[0]].clone());
    two.0.insert(ids2[1], made.0[&ids[1]].clone());
    assert!(check(&p2, seq2, &two, whole()).findings.is_empty());
}

/// Applies `fix` the way the page will: its command on the project.
fn fixed(p: &Project, fix: &Fix) -> Project {
    let command = fix.apply(p).unwrap();
    assert!(matches!(command, Command::SetTrackSound { .. }));
    command.apply(p).unwrap()
}

#[test]
fn every_fix_for_covered_dialogue_makes_it_better() {
    let (p, seq, ids, made) = music_over_voice(1.0);
    let before = check(&p, seq, &made, whole());
    let was = before.amount(Problem::CoversVoice, ids[1]);
    let fixes = &before.findings[0].fixes;
    assert!(fixes.len() >= 2, "{fixes:#?}");
    for fix in fixes {
        let after = check(&fixed(&p, fix), seq, &made, whole());
        let now = after.amount(Problem::CoversVoice, ids[1]);
        println!("{}: {was:.2} -> {now:.2}", fix.label);
        // The dips are gentle on purpose: a few percent is a real change.
        assert!(now < was * 0.97, "{}: {was} -> {now}", fix.label);
    }
}

#[test]
fn cutting_around_300_hz_makes_a_pile_up_better() {
    let (p, seq, ids, made) = pile_up();
    let before = check(&p, seq, &made, whole());
    let mud = before
        .findings
        .iter()
        .find(|f| f.problem == Problem::Muddy)
        .unwrap();
    let was = before.amount(Problem::Muddy, mud.tracks[0]);
    let after = check(&fixed(&p, &mud.fixes[0]), seq, &made, whole());
    let now = after.amount(Problem::Muddy, mud.tracks[0]);
    println!("{}: {was:.2} -> {now:.2}", mud.fixes[0].label);
    assert!(now < was * 0.95, "{was} -> {now}");
    // Every fix on offer, one after another, helps more.
    let mut all = p.clone();
    for fix in &mud.fixes {
        all = fixed(&all, fix);
    }
    let both = check(&all, seq, &made, whole()).amount(Problem::Muddy, mud.tracks[0]);
    println!("all {} fixes: {was:.2} -> {both:.2}", mud.fixes.len());
    assert!(both < now, "{now} -> {both}");
    let _ = ids;
}

#[test]
fn at_the_playhead_only_a_short_stretch_is_checked() {
    let (p, seq, ids, made) = music_over_voice(1.0);
    let quiet = check(&p, seq, &made, Request::Playhead(Time::seconds(7)));
    assert_eq!(quiet.stretch, (Time::seconds(6), Time::seconds(8)));
    assert_eq!(quiet.amount(Problem::CoversVoice, ids[1]), 0.0);
    let busy = check(&p, seq, &made, Request::Playhead(Time::seconds(4)));
    assert!(
        busy.amount(Problem::CoversVoice, ids[1]) > 0.0,
        "{:#?}",
        busy.findings
    );
    let start = check(&p, seq, &made, Request::Playhead(Time::ZERO));
    assert_eq!(start.stretch, (Time::ZERO, Time::seconds(1)));
}

#[test]
fn progress_rises_to_the_end_and_cancel_stops_it() {
    let (p, seq, _, made) = music_over_voice(1.0);
    let seen = std::sync::Mutex::new(Vec::new());
    let report = analyse(&p, seq, whole(), &mut made.clone(), &|| true, &|d| {
        seen.lock().unwrap().push(d)
    })
    .unwrap();
    assert!(report.is_some());
    let seen = seen.into_inner().unwrap();
    assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0));
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let stopped = analyse(
        &p,
        seq,
        whole(),
        &mut made.clone(),
        &|| asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5,
        &|_| {},
    )
    .unwrap();
    assert!(stopped.is_none());
    assert_eq!(asked.into_inner(), 6, "stopped at the first frame after");
}

fn jobs() -> JobSystem {
    JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()))
}

#[test]
fn on_the_analysis_pool_it_finishes_and_cancels() {
    let jobs = jobs();
    let (p, seq, ids, made) = music_over_voice(1.0);
    let check = Clarity::start_with(&jobs, p.clone(), seq, whole(), Box::new(made.clone()));
    match check.wait() {
        Outcome::Done(report) => assert!(report.amount(Problem::CoversVoice, ids[1]) > 0.0),
        other => panic!("{other:?}"),
    }
    let check = Clarity::start_with(&jobs, p, seq, whole(), Box::new(made));
    check.cancel();
    assert!(matches!(check.wait(), Outcome::Cancelled));
}

#[test]
fn a_recordings_tracks_are_decoded_and_measured() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
    let p = Project::new_automatic("T");
    let seq = p.active_sequence;
    let media = media::describe(&fixture).unwrap();
    let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
    let jobs = jobs();
    let check = Clarity::start(
        &jobs,
        p.clone(),
        seq,
        Request::Region(Time::ZERO, Time::seconds(1)),
    );
    let report = match check.wait() {
        Outcome::Done(report) => report,
        other => panic!("{other:?}"),
    };
    assert_eq!(report.tracks.len(), p.sequence(seq).unwrap().audio_tracks.len());
    for t in &report.tracks {
        let loudest = t.levels_db.iter().copied().fold(f32::MIN, f32::max);
        assert!(loudest > -90.0, "{:?}", t.levels_db);
    }
}

#[test]
fn a_clips_own_volume_line_is_heard_by_the_check() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
    let p = Project::new_automatic("T");
    let seq = p.active_sequence;
    let media = media::describe(&fixture).unwrap();
    let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
    let first = p.sequence(seq).unwrap().audio_tracks[0].clips[0].clone();
    let point = |value| project::track_sound::LinePoint {
        at: first.source_in,
        value,
    };
    let quiet = Command::SetClipLines {
        sequence: seq,
        clip: first.id,
        lines: Box::new(project::ClipLines {
            volume: [point(-30_000)].into_iter().collect(),
            pan: Default::default(),
        }),
    }
    .apply(&p)
    .unwrap();
    let request = Request::Region(Time::ZERO, Time::seconds(1));
    let level = |p: &Project| {
        let mut clips = Clips::new(RATE);
        let report = analyse(p, seq, request, &mut clips, &|| true, &|_| {})
            .unwrap()
            .unwrap();
        let t = &report.tracks[0];
        let power: f32 = t.levels_db.iter().map(|db| 10f32.powf(db / 10.0)).sum();
        bands::db(power)
    };
    let (before, after) = (level(&p), level(&quiet));
    println!("first track: {before:.1} dB, with its clip line at -30 dB: {after:.1} dB");
    assert!((before - after - 30.0).abs() < 1.5, "{before} -> {after}");
}
