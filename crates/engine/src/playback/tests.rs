use super::*;
use crate::offline::OfflineMix;
use audio::BLOCK;
use project::track_sound::LinePoint;
use project::{ClipId, Command, MasterSound, MediaId, TrackRole, TrackSound};
use std::path::Path;

mod level;
mod nested;

const RATE: u32 = 48_000;

fn tenth() -> Time {
    Time::from_ratio(1, 10).unwrap_or(Time::ZERO)
}

fn imported(name: &str) -> Project {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name);
    let p = Project::new_automatic("T");
    let seq = p.active_sequence;
    let media = media::describe(&fixture).unwrap();
    Command::Import { sequence: seq, media }.apply(&p).unwrap()
}

/// The recording with three sound streams (one track each).
fn recording() -> Project {
    imported("game-3-audio.mkv")
}

/// The recording with an effect on every track and on the master: a
/// dialogue track with high pass, clean-up (a learned profile), EQ and
/// compressor; a ducked one with the Loudness tool; one panned with a
/// volume line; the master's compressor and limiter.
fn with_effects() -> Project {
    let p = recording();
    let seq = p.active_sequence;
    effects_on(p, seq)
}

/// [`with_effects`]' effects on the first three sound tracks and the master
/// of timeline `seq`.
fn effects_on(mut p: Project, seq: SequenceId) -> Project {
    let tracks: Vec<_> = p
        .sequence(seq)
        .unwrap()
        .audio_tracks
        .iter()
        .map(|t| t.id)
        .collect();
    let mut sounds = vec![TrackSound::default(); 3];
    let s = &mut sounds[0];
    s.role = TrackRole::Dialogue;
    s.high_pass.on = true;
    s.cleanup.on = true;
    s.cleanup.profile = Some(audio::settings::noise_to_project(
        &audio::dsp::cleanup::NoiseProfile([-70.0; 64]),
    ));
    s.eq.on = true;
    s.eq.bands[2].gain_mdb = 6_000;
    s.compressor.on = true;
    let s = &mut sounds[1];
    s.ducking.on = true;
    s.ducking.threshold_mdb = -60_000;
    s.loudness.on = true;
    let s = &mut sounds[2];
    s.pan = -400_000;
    s.volume_line.push_back(LinePoint {
        at: Time::ZERO,
        value: 0,
    });
    s.volume_line.push_back(LinePoint {
        at: Time::seconds(1),
        value: -12_000,
    });
    for (track, sound) in tracks.into_iter().zip(sounds) {
        p = Command::SetTrackSound {
            sequence: seq,
            track,
            sound: Box::new(sound),
        }
        .apply(&p)
        .unwrap();
    }
    let mut master = MasterSound::default();
    master.compressor.on = true;
    master.limiter.on = true;
    master.limiter.ceiling_mdb = -6_000;
    Command::SetMasterSound {
        sequence: seq,
        sound: master,
    }
    .apply(&p)
    .unwrap()
}

/// Playback's two ends without a sound card: the audio thread's and the
/// callback's, and the clock they share.
struct Fake {
    session: Session,
    output: Output,
    clock: Arc<Clock>,
    send: Sender<Message>,
    inbox: Receiver<Message>,
}

fn fake(project: &Project, from: Time) -> Fake {
    let clock = Arc::new(Clock::new());
    let (session, output) =
        prepare(project.clone(), project.active_sequence, from, (RATE, 2), &clock).unwrap();
    clock.rate.store(u64::from(RATE), Ordering::Release);
    clock.follow_sound();
    let (send, inbox) = channel();
    Fake {
        session,
        output,
        clock,
        send,
        inbox,
    }
}

impl Fake {
    /// `frames` of sound as a device asking for `chunk` frames at a time
    /// gets them, the audio thread taking its turn before each.
    fn play(&mut self, frames: usize, chunk: usize) -> Vec<f32> {
        let mut out = vec![0f32; frames * 2];
        for part in out.chunks_mut(chunk * 2) {
            self.session.step(&self.inbox, &self.clock);
            self.output.render(part, 2);
        }
        out
    }

    fn update(&mut self, project: &Project) {
        self.send
            .send(Message::Project(Box::new(project.clone())))
            .unwrap();
        self.session.step(&self.inbox, &self.clock);
    }
}

fn loudness(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

fn most_apart(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max)
}

#[test]
fn the_clock_moves_from_where_playback_started() {
    let p = Project::new_automatic("T");
    let playback = Playback::start(p.clone(), p.active_sequence, Time::seconds(3));
    assert_eq!(playback.started_at(), Time::seconds(3));
    poll(|| {
        let at = playback.position();
        assert!(at >= Time::seconds(3), "{at:?}");
        // Without a sound card (as on test machines) time itself runs.
        (playback.has_sound() || at > Time::seconds(3) + tenth()).then_some(())
    });
}

/// Asks `ready` every 10 ms until it gives something, for up to 30
/// seconds (long, for a busy test machine); fails if it never does.
fn poll<T>(mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(done) = ready() {
            return done;
        }
        assert!(Instant::now() < deadline, "not within 30 s");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn quiet_playback(from: Time, audio: impl FnOnce(&Arc<Clock>) + Send + 'static) -> Playback {
    let (send, _) = channel();
    Playback::with_audio(SequenceId(0), from, send, audio)
}

#[test]
fn the_clock_stands_still_until_the_sound_is_ready() {
    // Sound that takes a while to get ready, as far into a long file on a
    // slow drive: here, until the test says it is.
    let (ready, wait) = channel::<()>();
    let playback = quiet_playback(Time::seconds(5), move |clock| {
        let _ = wait.recv_timeout(Duration::from_secs(30));
        clock.follow_time();
    });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(playback.position(), Time::seconds(5), "waiting for the sound");
    assert!(!playback.has_sound());
    let readied = playback.clock.now_us();
    ready.send(()).unwrap();
    poll(|| playback.unheard().then_some(()));
    // The clock runs from when the sound was ready, not from the start.
    let from = playback.clock.wall_from.load(Ordering::Acquire);
    assert!(from >= readied, "from {from} us, ready at {readied} us");
    let at = poll(|| Some(playback.position()).filter(|at| *at > Time::seconds(5) + tenth()));
    let since = Time::from_ratio((playback.clock.now_us() - readied) as i64, 1_000_000).unwrap();
    assert!(
        at <= Time::seconds(5) + since,
        "from where it started: {at:?}, {since:?} after the sound was ready"
    );
}

#[test]
fn the_position_is_the_sample_the_sound_card_is_handed_less_the_delay() {
    let mut playback = quiet_playback(Time::seconds(2), |clock| {
        clock.rate.store(48_000, Ordering::Release);
        clock.output.heard.store(120_000, Ordering::Release);
        clock.follow_sound();
    });
    poll(|| playback.has_sound().then_some(()));
    assert_eq!(
        playback.position(),
        Time::seconds(2) + Time::from_ratio(1, 2).unwrap()
    );
    // A jump: the position stays at the place jumped to until its sound is
    // ready, also when the sample handed out is still the old one.
    playback.jump(Time::seconds(1));
    assert_eq!(playback.position(), Time::seconds(1));
    assert_eq!(playback.started_at(), Time::seconds(1));
    playback.clock.jumped.store(1, Ordering::Release);
    playback
        .clock
        .output
        .heard
        .store(48_000 + 4_800, Ordering::Release);
    assert_eq!(playback.position(), Time::seconds(1) + tenth());
}

#[test]
fn without_a_sound_card_a_jump_restarts_the_wall_clock() {
    let mut playback = quiet_playback(Time::seconds(2), |clock| clock.follow_time());
    poll(|| playback.unheard().then_some(()));
    poll(|| (playback.position() > Time::seconds(2)).then_some(()));
    let before = playback.clock.now_us();
    playback.jump(Time::seconds(10));
    let at = playback.position();
    let since = Time::from_ratio((playback.clock.now_us() - before) as i64, 1_000_000).unwrap();
    // No more than the time the jump and the reading took.
    assert!(
        at >= Time::seconds(10) && at <= Time::seconds(10) + since,
        "{at:?}, {since:?} after the jump"
    );
}

/// The sound card's mix couldn't be taken back (its driver stuck letting
/// go of it): time takes over from where the sound had got to, not from
/// where playback started, and a jump afterwards runs from its own place.
#[test]
fn when_time_takes_over_from_the_sound_the_playhead_goes_on() {
    let mut playback = quiet_playback(Time::seconds(2), |clock| {
        clock.rate.store(48_000, Ordering::Release);
        // Two seconds past the start.
        clock.output.heard.store(4 * 48_000, Ordering::Release);
        clock.follow_sound();
        clock.follow_time_from(Time::seconds(2));
    });
    poll(|| playback.unheard().then_some(()));
    let at = playback.position();
    assert!(at >= Time::seconds(4), "back to the start: {at:?}");
    playback.jump(Time::seconds(10));
    let at = playback.position();
    let wall_from = playback.clock.wall_from.load(Ordering::Acquire);
    let since = Time::from_ratio((playback.clock.now_us() - wall_from) as i64, 1_000_000).unwrap();
    assert!(
        at >= Time::seconds(10) && at <= Time::seconds(10) + since,
        "from the jump: {at:?}, {since:?} after it"
    );
}

/// A jump the sound hadn't taken when time took over runs from the place
/// jumped to, without the time the sound had played before it.
#[test]
fn when_time_takes_over_a_jump_not_taken_runs_from_its_own_place() {
    let clock = Clock::new();
    clock.rate.store(48_000, Ordering::Release);
    clock.output.heard.store(4 * 48_000, Ordering::Release);
    clock.follow_paced();
    // Jump 1 (to 10 s) was sent; the mix last took the start, at 2 s.
    clock.follow_time_from(Time::seconds(2));
    let at = clock.position(Time::seconds(10), 1);
    let wall_from = clock.wall_from.load(Ordering::Acquire);
    let since = Time::from_ratio((clock.now_us() - wall_from) as i64, 1_000_000).unwrap();
    assert!(
        at >= Time::seconds(10) && at <= Time::seconds(10) + since,
        "{at:?}, {since:?} after time took over"
    );
    // The place the sound had got to still holds for the start.
    assert!(clock.position(Time::seconds(2), 0) >= Time::seconds(4));
}

#[test]
fn closing_waits_for_stopped_audio_threads_to_end() {
    let (let_go, letting_go) = channel::<()>();
    let playback = quiet_playback(Time::ZERO, move |clock| {
        while !clock.stopped() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Letting go of the sound card and the files takes a moment: here,
        // until the test says so.
        let _ = letting_go.recv_timeout(Duration::from_secs(30));
    });
    let clock = Arc::clone(&playback.clock);
    assert!(!wait_until_quiet(Duration::from_millis(50)), "still playing");
    assert!(!wait_until_none(&clock.threads, Duration::from_millis(50)));
    drop(playback);
    assert!(
        !wait_until_quiet(Duration::from_millis(50)),
        "stopped, not ended yet"
    );
    assert!(!wait_until_none(&clock.threads, Duration::from_millis(50)));
    let_go.send(()).unwrap();
    // Other tests' audio threads count in `wait_until_quiet` too, so this
    // one's own count is waited for.
    assert!(wait_until_none(&clock.threads, Duration::from_secs(30)), "ended");
}

/// The thread mixing without a sound card counts as an audio thread until
/// it ends, also once the audio thread that started it has returned.
#[test]
fn closing_waits_for_the_paced_thread_of_an_unheard_playback() {
    let output = fake(&recording(), Time::ZERO).output;
    let (returned, audio_returned) = channel::<()>();
    let playback = quiet_playback(Time::ZERO, move |clock| {
        play_unheard(output, (RATE, 2), clock, Time::ZERO);
        let _ = returned.send(());
    });
    audio_returned.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(playback.unheard(), "mixed unheard");
    let clock = Arc::clone(&playback.clock);
    // The audio thread has returned; the paced thread mixes on until
    // playback stops, and is counted.
    assert!(
        !wait_until_none(&clock.threads, Duration::from_millis(200)),
        "the paced thread is counted while it mixes"
    );
    drop(playback);
    assert!(wait_until_none(&clock.threads, Duration::from_secs(30)), "ended");
}

#[test]
fn stopping_does_not_wait_for_the_audio_thread() {
    let (done, finished) = std::sync::mpsc::channel();
    let (carry_on, busy) = channel::<()>();
    let playback = quiet_playback(Time::ZERO, move |clock| {
        // Busy with something that can't be interrupted (until the test
        // says, which it does only once stopping returned), then the usual
        // check. Stopping that waited would leave this to give up.
        let interrupted = busy.recv_timeout(Duration::from_secs(30)).is_ok();
        let _ = done.send((interrupted, clock.stopped()));
    });
    drop(playback);
    let _ = carry_on.send(());
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(60)),
        Ok((true, true)),
        "stopping returned while the thread was busy; it saw the stop and ended"
    );
}

/// Played from `from` through a device asking for 480 frames at a time,
/// and exported from there: the export's first sample is the sound heard
/// once the mixer's fixed delay has passed, and every sample after it is
/// the same, with every track effect and the master on.
#[test]
fn playback_and_export_make_the_same_samples_with_effects_on() {
    for project in [with_effects(), recording()] {
        let from = Time::from_ratio(1, 4).unwrap();
        let mut fake = fake(&project, from);
        let latency = fake.output.latency();
        let played = fake.play(RATE as usize + latency, 480);
        let seq = project.active_sequence;
        let mut export = OfflineMix::new(
            &project,
            seq,
            RATE,
            from,
            MasterSettings::from_project(project.active().unwrap()),
        )
        .unwrap();
        let mut exported = Vec::new();
        export.take(RATE as usize, &mut exported);
        let exported: Vec<f32> = exported.iter().map(|s| s.clamp(-1.0, 1.0)).collect();
        let heard = &played[latency * 2..];
        assert!(loudness(heard) > 0.01, "it sounds");
        assert!(
            heard == &exported[..],
            "playback and export differ by up to {}",
            most_apart(heard, &exported)
        );
        assert_eq!(fake.output.dry_frames(), 0);
    }
}

/// With every effect off, the sound is exactly what playback mixed before
/// the effects came (clips, track volume, mute and solo, the master volume,
/// all summed by `mix::Mixer::mix` a block of 1 024 at a time), to the bit.
/// (Export mixed a frame's samples at a time; a file read in other pieces
/// can differ in the last bit: `mix`'s and export's own tests allow 1e-6.)
#[test]
fn with_every_effect_off_the_sound_is_exactly_as_before() {
    let mut p = recording();
    {
        let seq = &mut p.sequences[0];
        seq.audio_tracks[0].gain_db = -7.5;
        seq.audio_tracks[1].gain_db = 3.0;
        seq.audio_tracks[2].muted = true;
        seq.master_gain_db = -2.0;
    }
    let mut soloed = p.clone();
    soloed.sequences[0].audio_tracks[1].solo = true;
    for project in [p, soloed] {
        let seq = project.active().unwrap();
        let start = Time::from_ratio(1, 3).unwrap();
        let frames = RATE as usize * 3 / 2;
        // As playback mixed: a thousand and twenty-four samples at a time.
        let mut before = vec![0f32; frames * 2];
        let mut old = crate::mix::Mixer::new(RATE);
        for (n, chunk) in before.chunks_mut(1_024 * 2).enumerate() {
            old.mix(
                &project,
                seq,
                start.as_secs_f64() + (n * 1_024) as f64 / f64::from(RATE),
                chunk,
            );
        }
        let mut now =
            OfflineMix::new(&project, seq.id, RATE, start, MasterSettings::from_project(seq)).unwrap();
        let mut mixed = Vec::new();
        now.take(frames, &mut mixed);
        assert!(loudness(&before) > 0.01);
        assert!(mixed == before, "off by up to {}", most_apart(&mixed, &before));
    }
}

/// Muting while playing is heard within one block: the block after the
/// change glides down, and the next is silent once the limiter's short
/// delay has passed. Unmuting brings it back as quickly.
#[test]
fn an_edit_while_playing_is_heard_within_a_block() {
    let project = recording();
    let mut fake = fake(&project, Time::from_ratio(1, 2).unwrap());
    let limiter = audio::dsp::limiter::Limiter::new(RATE, -1.0).latency();
    // Whole blocks, so what the device is handed lines up with the mixer's.
    let before = fake.play(BLOCK * 94, BLOCK);
    assert!(loudness(&before[before.len() - 2 * BLOCK..]) > 0.01);
    let mut muted = project.clone();
    for t in muted.sequences[0].audio_tracks.iter_mut() {
        t.muted = true;
    }
    fake.update(&muted);
    let gliding = fake.play(BLOCK, BLOCK);
    assert!(loudness(&gliding) > 0.0, "the glide");
    let after = fake.play(BLOCK, BLOCK);
    assert!(
        after[limiter * 2..].iter().all(|s| *s == 0.0),
        "silent within a block"
    );
    fake.update(&project);
    fake.play(BLOCK, BLOCK);
    let back = fake.play(BLOCK, BLOCK);
    assert!(loudness(&back[limiter * 2..]) > 0.01, "heard again");
    // Undo is just another project: the master's volume too.
    let mut quiet = project.clone();
    quiet.sequences[0].master_gain_db = -40.0;
    let level = loudness(&fake.play(BLOCK, BLOCK));
    fake.update(&quiet);
    fake.play(BLOCK, BLOCK);
    let down = loudness(&fake.play(BLOCK, BLOCK)[limiter * 2..]);
    assert!(down < level / 20.0, "{down} against {level}");
}

/// A track taken out while playing goes at once; put back (an undo), it
/// comes in in step with the rest, to the sample, as if it had been there.
#[test]
fn tracks_taken_out_and_put_back_while_playing() {
    let project = recording();
    // As an edit that takes out a track with its clips, then undone.
    let mut without = project.clone();
    without.sequences[0].audio_tracks.remove(2);
    let from = Time::from_ratio(1, 4).unwrap();
    let mut always = fake(&project, from);
    let whole = always.play(RATE as usize * 3 / 2, 480);

    let mut changing = fake(&project, from);
    let mut heard = changing.play(RATE as usize / 4, 480);
    changing.update(&without);
    heard.extend(changing.play(RATE as usize / 4, 480));
    changing.update(&project);
    heard.extend(changing.play(RATE as usize, 480));
    // A quarter of a second after it was taken out, it is missing.
    let gone = RATE as usize / 2 - 4_800;
    assert!(most_apart(&heard[gone * 2..RATE as usize], &whole[gone * 2..RATE as usize]) > 0.01);
    // Within the rings' half second of coming back, it is all there again.
    let back = RATE as usize + RATE as usize / 4;
    assert!(
        heard[back * 2..] == whole[back * 2..],
        "off by up to {}",
        most_apart(&heard[back * 2..], &whole[back * 2..])
    );
}

#[test]
fn track_meters_follow_the_tracks_and_the_analyser_hears_the_one_chosen() {
    let project = recording();
    let ids: Vec<_> = project.sequences[0].audio_tracks.iter().map(|t| t.id).collect();
    let mut fake = fake(&project, Time::from_ratio(1, 4).unwrap());
    let meters = |fake: &Fake| {
        fake.clock
            .meters
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    };
    assert_eq!(meters(&fake), ids, "every track, in order");
    let spectrum = fake.clock.spectrum.get().cloned().unwrap();
    fake.play(RATE as usize / 2, 480);
    assert!(
        spectrum.levels().iter().all(|l| *l <= -119.0),
        "nobody chosen: nothing shown"
    );
    // The meters measured something on a track that sounds.
    let heard = fake
        .clock
        .meters
        .lock()
        .unwrap()
        .iter()
        .any(|(_, m)| m.take().momentary.is_finite());
    assert!(heard);
    fake.send
        .send(Message::Analyse(Some(Listen::Track(ids[0]))))
        .unwrap();
    fake.play(RATE as usize / 2, 480);
    assert!(
        spectrum.levels().iter().any(|l| *l > -100.0),
        "the chosen track is shown"
    );
    fake.send.send(Message::Analyse(Some(Listen::Mix))).unwrap();
    let windows = spectrum.stereo().windows;
    fake.play(RATE as usize / 2, 480);
    assert!(spectrum.stereo().windows > windows, "the mix is listened to");
    assert!(spectrum.levels().iter().any(|l| *l > -100.0), "and shown");
    let mut without = project.clone();
    without.sequences[0].audio_tracks.remove(2);
    fake.update(&without);
    assert_eq!(meters(&fake), [ids[0], ids[1]]);
}

/// The callback never allocates or frees: mixing, the commands an edit
/// sends, lines swapped in and the old sent back, a jump taken while
/// silent. Counted on this thread only.
#[test]
fn the_sound_cards_callback_never_allocates() {
    let project = with_effects();
    let mut fake = fake(&project, Time::ZERO);
    let mut buf = vec![0f32; 441 * 2];
    let mut changed = project.clone();
    let mut counted = 0;
    for n in 0..300 {
        if n % 20 == 10 {
            let track = &mut changed.sequences[0].audio_tracks[n % 3];
            track.muted = !track.muted;
            track.gain_db = -(n as f32) / 30.0;
            track.sound.volume_line.push_back(LinePoint {
                at: Time::from_ratio(n as i64, 100).unwrap(),
                value: -3_000,
            });
            fake.update(&changed);
        }
        if n == 150 {
            // A jump, as the audio thread makes it, without waiting.
            fake.clock.output.primed.store(false, Ordering::Release);
            fake.session.controller.locate(RATE as u64).unwrap();
        }
        if n == 152 {
            fake.session.feeder.jump(RATE as u64, 1.0);
            fake.session.feeder.fill(&|| true);
            fake.clock.output.primed.store(true, Ordering::Release);
        }
        fake.session.step(&fake.inbox, &fake.clock);
        let ((), allocations) = os::alloc_count::counted(|| fake.output.render(&mut buf, 2));
        counted += allocations;
    }
    assert_eq!(counted, 0, "allocations or frees in the callback");
}

/// 32 tracks, every effect on each and the master's: the callback mixes
/// them all at the device's pace (a device thread asking for 256 frames
/// every 5.3 ms, as a sound card at 48 kHz) while the audio thread decodes
/// and feeds 32 files. Prints the time per block and the frames that came
/// late. A measurement of the machine as much as of the code, so by hand,
/// in a release build on a quiet machine:
///
/// ```sh
/// cargo test -p engine --release thirty_two_tracks -- --ignored --nocapture
/// ```
#[test]
#[ignore = "timing run, by hand in a release build"]
fn thirty_two_tracks_play_through_the_real_time_path() {
    let mut p = with_effects();
    let media = p.media.values().next().cloned().unwrap();
    {
        let seq = &mut p.sequences[0];
        let first: Vec<_> = seq.audio_tracks.iter().cloned().collect();
        let mut n = 0;
        while seq.audio_tracks.len() < 32 {
            let mut t = first[n % first.len()].clone();
            t.id = TrackId(50_000 + n as u64);
            // A file of its own (the same recording under another name), as
            // 32 tracks of different recordings would have.
            let file = MediaId(70_000 + n as u64);
            for (i, c) in t.clips.iter_mut().enumerate() {
                c.id = ClipId(60_000 + (n * 10 + i) as u64);
                c.media = file;
            }
            t.sound.role = TrackRole::Other;
            t.sound.ducking.on = true;
            t.sound.compressor.on = true;
            t.sound.eq.on = true;
            t.sound.high_pass.on = true;
            t.sound.cleanup.on = true;
            seq.audio_tracks.push_back(t);
            n += 1;
        }
    }
    for n in 0..32 {
        let id = MediaId(70_000 + n);
        p.media.insert(id, project::MediaRef { id, ..media.clone() });
    }
    assert_eq!(p.active().unwrap().audio_tracks.len(), 32);
    let fake = fake(&p, Time::ZERO);
    let Fake {
        mut session,
        mut output,
        clock,
        inbox,
        ..
    } = fake;
    let blocks = RATE as usize / BLOCK; // a second
    let device_clock = Arc::clone(&clock);
    let device = std::thread::spawn(move || {
        let mut buf = vec![0f32; BLOCK * 2];
        let mut times = Vec::with_capacity(blocks);
        let mut loud = 0f32;
        let begun = Instant::now();
        for b in 0..blocks {
            let started = Instant::now();
            output.render(&mut buf, 2);
            times.push(started.elapsed());
            loud = loud.max(loudness(&buf));
            let due = begun + Duration::from_secs_f64((b + 1) as f64 * BLOCK as f64 / f64::from(RATE));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        device_clock.output.stop.store(true, Ordering::Release);
        (times, loud, output.dry_frames())
    });
    while !clock.stopped() {
        if session.step(&inbox, &clock) == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let (mut times, loud, dry) = device.join().unwrap();
    times.sort();
    let mean = times.iter().sum::<Duration>() / times.len() as u32;
    println!(
        "32 tracks, every effect: mean {mean:?}, median {:?}, worst {:?} a block of {BLOCK}; {dry} frames dry",
        times[times.len() / 2],
        times[times.len() - 1]
    );
    assert!(loud > 0.001, "it sounds");
    // The feeder keeps up: next to nothing late (none here, on two busy cores).
    assert!(dry < u64::from(RATE) * 32 / 20, "{dry} frames late");
    // A block lasts 5.3 ms; the mean must leave room even on a busy machine.
    assert!(mean < Duration::from_micros(5_333), "{mean:?}");
}

#[test]
fn following_the_project_sends_only_when_the_mix_changed() {
    let project = recording();
    let (send, inbox) = channel();
    let mut playback = Playback::with_audio(project.active_sequence, Time::ZERO, send, |c| c.follow_time());
    playback.mix = MixShape::of(&project, project.active_sequence);
    playback.follow(&project);
    assert!(inbox.try_recv().is_err(), "nothing changed");
    let mut louder = project.clone();
    louder.sequences[0].audio_tracks[0].gain_db = 2.0;
    playback.follow(&louder);
    assert!(matches!(inbox.try_recv(), Ok(Message::Project(_))));
    playback.follow(&louder);
    assert!(inbox.try_recv().is_err(), "sent once");
    let mut sounded = louder.clone();
    sounded.sequences[0].master_sound.limiter.on = true;
    playback.follow(&sounded);
    assert!(matches!(inbox.try_recv(), Ok(Message::Project(_))));
}

/// Without a sound card the mix is still made at real-time pace and
/// measured: the master's and the tracks' meters move, and the clock runs
/// at the pace of the samples mixed.
#[test]
fn without_a_sound_card_the_meters_still_move() {
    let project = recording();
    let from = Time::from_ratio(1, 4).unwrap();
    let begun = Instant::now();
    let playback = Playback::start_unheard(project.clone(), project.active_sequence, from);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut master = Levels::SILENT;
    let mut tracks = false;
    while Instant::now() < deadline && !(master.momentary.is_finite() && tracks) {
        std::thread::sleep(Duration::from_millis(50));
        master = playback.levels();
        tracks |= playback
            .track_meters()
            .iter()
            .any(|(_, m)| m.take().momentary.is_finite());
    }
    assert!(playback.unheard(), "nothing heard");
    assert!(!playback.has_sound());
    assert!(
        master.momentary.is_finite(),
        "the master's meter moved: {master:?}"
    );
    assert!(tracks, "a track's meter moved");
    // The clock follows the samples mixed, at real-time pace: it moves on,
    // and never ahead of the time passed by more than the buffer mixed
    // ahead (512 frames).
    let a = playback.position();
    let b = poll(|| Some(playback.position()).filter(|b| *b - a > tenth()));
    let passed = begun.elapsed().as_secs_f64();
    let played = (b - from).as_secs_f64();
    assert!(
        played <= passed + 512.0 / f64::from(RATE),
        "{played} s mixed in {passed} s"
    );
}

/// The thread standing in for the sound card never allocates, and waits
/// a buffer's time between buffers.
#[test]
fn the_paced_thread_never_allocates() {
    let project = with_effects();
    let fake = fake(&project, Time::ZERO);
    let mut paced = crate::output::Paced::new(fake.output, (RATE, 2));
    let start = Instant::now();
    let mut counted = 0;
    let mut waited = Duration::ZERO;
    for _ in 0..50 {
        let (wait, allocations) = os::alloc_count::counted(|| paced.tick(start));
        counted += allocations;
        waited = wait;
    }
    assert_eq!(counted, 0, "allocations or frees on the paced thread");
    // 50 buffers of 512 frames at 48 kHz are 533 ms ahead of `start`.
    let ahead = waited.as_secs_f64();
    assert!((0.5..0.56).contains(&ahead), "{ahead}");
}

/// How a fake sound card goes away while playing.
#[derive(Debug, Clone, Copy)]
enum Gone {
    /// The system stops calling it and says nothing.
    Quietly,
    /// It says the device is no longer there, then stops calling.
    Saying,
}

/// A sound card: a thread calling the mix at real-time pace through the
/// same [`Held`] the real callback uses, until it goes away after `calls`
/// calls (it stays alive, not calling, until let go, as a stuck driver does).
struct FakeCard {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeCard {
    /// Frames handed to the mix per call: 10 ms at [`RATE`].
    const FRAMES: usize = 480;

    fn start(slot: Slot, shared: Arc<Shared>, gone: Gone, calls: usize) -> FakeCard {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut held = Held::new(slot);
            let mut buffer = vec![0f32; FakeCard::FRAMES * 2];
            let mut called = 0;
            while !stopped.load(Ordering::Acquire) {
                if called < calls {
                    if let Some(output) = held.output() {
                        output.render(&mut buffer, 2);
                    }
                    called += 1;
                } else if matches!(gone, Gone::Saying) {
                    shared.lost.store(true, Ordering::Release);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        FakeCard {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for FakeCard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// Longer than [`poll`] waits: a jump that waits this long for a sound card
/// fails the test however fast the machine, so a jump taken at all wasn't
/// held up by the card.
const NEVER: Duration = Duration::from_secs(60);

/// Plays `project` from `from` through a [`FakeCard`] that goes away
/// (`gone`) after `calls` calls, the audio thread waiting on it as `limits`
/// say.
fn with_fake_card(project: Project, from: Time, gone: Gone, calls: usize, limits: Limits) -> Playback {
    let seq = project.active_sequence;
    let (send, inbox) = channel();
    Playback::with_audio(seq, from, send, move |clock| {
        let (mut session, output) = prepare(project, seq, from, (RATE, 2), clock).unwrap();
        session.limits = limits;
        let slot: Slot = Arc::new(Mutex::new(Some(output)));
        let card = FakeCard::start(Arc::clone(&slot), Arc::clone(&clock.output), gone, calls);
        clock.follow_sound();
        let heard = Heard {
            stream: Box::new(card),
            slot,
        };
        play(session, Some(heard), (RATE, 2), clock, &inbox);
    })
}

/// When the sound card goes away mid-play, playback carries on unheard from
/// where it was, the clock and meters moving, and jumps don't wait for the
/// card any more.
fn plays_on_when_the_sound_card_goes_away(gone: Gone) {
    let from = Time::from_ratio(1, 4).unwrap();
    let begun = Instant::now();
    // Half a second of sound, then gone. A jump never gives up on the card
    // by itself.
    let limits = Limits {
        jump_patience: NEVER,
        ..Limits::PLAYING
    };
    let mut playback = with_fake_card(recording(), from, gone, 50, limits);
    poll(|| playback.unheard().then_some(()));
    assert!(!playback.has_sound());
    // The clock goes on from where the card left it.
    let a = playback.position();
    assert!(a > from + tenth(), "{gone:?}: it played first: {a:?}");
    poll(|| playback.levels().momentary.is_finite().then_some(()));
    // At real-time pace: it moves on, and is never ahead of the time passed
    // by more than what the card and the paced thread were handed ahead
    // (a call's frames and a buffer of 512).
    let b = poll(|| Some(playback.position()).filter(|b| *b - a > tenth()));
    let passed = begun.elapsed().as_secs_f64();
    let played = (b - from).as_secs_f64();
    let ahead = (FakeCard::FRAMES + 512) as f64 / f64::from(RATE);
    assert!(
        played <= passed + ahead,
        "{gone:?}: {played} s played in {passed} s"
    );
    // A jump is taken by the paced thread, not left waiting for the card.
    playback.jump(Time::seconds(1));
    poll(|| (playback.position() > Time::seconds(1)).then_some(()));
    assert!(playback.unheard(), "{gone:?}");
}

#[test]
fn playback_goes_on_unheard_when_the_sound_card_stops_calling() {
    plays_on_when_the_sound_card_goes_away(Gone::Quietly);
}

#[test]
fn playback_goes_on_unheard_when_the_sound_card_says_it_is_gone() {
    plays_on_when_the_sound_card_goes_away(Gone::Saying);
}

/// A jump made while the sound card goes away is made again once playback
/// goes on unheard, and is not held up for the jump's whole patience.
#[test]
fn a_jump_while_the_sound_card_goes_away_is_not_held_up() {
    // Gone before it ever calls; found gone long before a jump would give
    // up on it.
    let limits = Limits {
        jump_patience: NEVER,
        quiet: Duration::from_millis(50),
    };
    let mut playback = with_fake_card(recording(), Time::ZERO, Gone::Quietly, 0, limits);
    playback.jump(Time::seconds(1));
    poll(|| (playback.position() > Time::seconds(1)).then_some(()));
    assert!(playback.unheard());
}

/// A jump that comes while the sound card is being given up replaces the
/// one the card went away during: the older one is never made afterwards,
/// so the clock stays on the newest jump and the playhead moves on.
#[test]
fn a_newer_jump_replaces_one_the_sound_card_went_away_during() {
    let Fake {
        mut session,
        output,
        clock,
        send,
        inbox,
    } = fake(&recording(), Time::ZERO);
    // The card goes away during the first jump: it waits.
    session.card = true;
    clock.output.lost.store(true, Ordering::Release);
    send.send(Message::Jump {
        at: Time::seconds(1),
        number: 1,
    })
    .unwrap();
    session.step(&inbox, &clock);
    assert_eq!(session.pending, Some((Time::seconds(1), 1)));
    // The mix is taken back and plays on unheard; a second jump came in
    // meanwhile.
    session.card = false;
    play_unheard(output, (RATE, 2), &clock, session.taken);
    send.send(Message::Jump {
        at: Time::seconds(2),
        number: 2,
    })
    .unwrap();
    session.step(&inbox, &clock);
    assert_eq!(clock.jumped.load(Ordering::Acquire), 2);
    assert_eq!(session.pending, None, "the older jump is dropped");
    // Steps with nothing new don't go back to the older jump.
    session.step(&inbox, &clock);
    session.step(&inbox, &clock);
    assert_eq!(clock.jumped.load(Ordering::Acquire), 2);
    // The playhead moves on from the second jump.
    poll(|| (clock.position(Time::seconds(2), 2) > Time::seconds(2)).then_some(()));
    clock.output.stop.store(true, Ordering::Release);
    assert!(wait_until_none(&clock.threads, Duration::from_secs(30)), "ended");
}

#[test]
fn only_errors_that_end_the_stream_give_up_on_the_sound_card() {
    use cpal::ErrorKind as K;
    assert!(ends_the_stream(K::DeviceNotAvailable));
    assert!(ends_the_stream(K::StreamInvalidated));
    assert!(!ends_the_stream(K::Xrun));
    assert!(!ends_the_stream(K::DeviceChanged));
    assert!(!ends_the_stream(K::RealtimeDenied));
}
