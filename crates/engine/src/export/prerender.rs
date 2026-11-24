//! Pre-rendering a stretch of the timeline into the render cache
//! (`cache::render`): each frame put together as an export does it, read
//! back and kept by what it shows ([`frame_key`]), so the viewer shows it
//! from there instead of decoding and putting together every layer. Frames
//! already kept, and gaps (black), are passed over.
//!
//! Frames are kept at the size the viewer shows them at most (the
//! timeline's shape, no larger than 1920×1080 or the timeline): enough for
//! the viewer, a fraction of the disk. The export never reads them, and a
//! pre-render never reads what was pre-rendered before.

use super::{Devices, Out, Picturing, Shared, cannot, frames, nv12_to_rgba, wait_for};
use crate::frame::{self, FrameDescription};
use cache::render::{Key, Store};
use project::{Project, SequenceId, Time};
use render::{Area, Reader};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, channel};

/// The largest frame kept: the most a viewer is usually shown at.
pub const LARGEST: (u32, u32) = (1920, 1080);

/// The key a frame described as `description` is kept by: a hash of
/// everything that makes its picture (every layer's place, colour,
/// effects, titles, nested timelines and adjustment layers, the moment of
/// each file, and each file's fingerprint), and of the app's version, so
/// a frame put together by an older version is never shown. Its place on
/// the timeline is not in it. `None` when a file it shows is not in the
/// project.
pub fn frame_key(project: &Project, description: &FrameDescription) -> Option<Key> {
    let mut hash = Fnv::new();
    hash.write(brand::VERSION.as_bytes());
    hash.write(format!("{description:?}").as_bytes());
    for layer in description.layers.iter().filter(|l| l.title.is_none()) {
        let media = project.media.get(&layer.media)?;
        hash.write(&media.fingerprint.size.to_le_bytes());
        hash.write(media.fingerprint.sample_hash.as_bytes());
    }
    Some(hash.finish())
}

/// FNV-1a, 128 bits: the same on every machine and in every version of
/// the compiler, as a key kept on disk must be.
struct Fnv(u128);

impl Fnv {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    fn new() -> Fnv {
        Fnv(Fnv::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u128::from(*byte);
            self.0 = self.0.wrapping_mul(Fnv::PRIME);
        }
        // Where one part ends: "ab" + "c" differs from "a" + "bc".
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(Fnv::PRIME);
    }

    fn finish(&self) -> u128 {
        self.0
    }
}

/// The size frames of a timeline of `timeline` pixels are kept at.
pub fn kept_size(timeline: (u32, u32)) -> (u32, u32) {
    let fitted = Area::fitted(timeline, LARGEST).size();
    let side = |fitted: f32, own: u32| ((fitted.round() as u32).min(own).max(2) + 1) & !1;
    (side(fitted.0, timeline.0), side(fitted.1, timeline.1))
}

/// How a pre-render went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// Frames put together and kept now.
    pub written: u64,
    /// Frames of the stretch that were kept already or are gaps.
    pub passed: u64,
}

/// A pre-render under way.
#[derive(Debug)]
pub struct Prerender {
    shared: Arc<Shared>,
    outcome: Receiver<diag::Result<Option<Done>>>,
    range: (Time, Time),
}

impl Prerender {
    /// Starts pre-rendering `range` of `sequence` as `project` is now, into
    /// `store`. `notify` runs on the worker now and then, and at the end.
    pub fn start(
        project: Project,
        sequence: SequenceId,
        range: (Time, Time),
        devices: Devices,
        store: Arc<Store>,
        notify: impl Fn() + Send + 'static,
    ) -> Prerender {
        let shared = Arc::new(Shared::default());
        let (send, outcome) = channel();
        let theirs = shared.clone();
        std::thread::Builder::new()
            .name("pre-render".into())
            .spawn(move || {
                let result = run(&project, sequence, range, devices, &store, &theirs, &notify);
                let _ = send.send(result);
                notify();
            })
            .map_err(|err| tracing::error!("the pre-render could not start: {err}"))
            .ok();
        Prerender {
            shared,
            outcome,
            range,
        }
    }

    /// The stretch being pre-rendered.
    pub fn range(&self) -> (Time, Time) {
        self.range
    }

    /// Frames done, of how many.
    pub fn progress(&self) -> (u64, u64) {
        (
            self.shared.frames_done.load(Ordering::Relaxed),
            self.shared.frames.load(Ordering::Relaxed),
        )
    }

    /// Stops it after the frame under way; what is done is kept.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// How it ended, once it has: `Ok(None)` when cancelled.
    pub fn finished(&self) -> Option<diag::Result<Option<Done>>> {
        self.outcome.try_recv().ok()
    }
}

impl Drop for Prerender {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The frames of `range` still to put together: each one's moment and key.
pub fn still_to_do(
    project: &Project,
    sequence: SequenceId,
    range: (Time, Time),
    store: &Store,
) -> Vec<(Time, Key)> {
    let Some(seq) = project.sequence(sequence) else {
        return Vec::new();
    };
    let rate = seq.settings.frame_rate;
    let (first, end) = frames(seq.length(), Some(range), rate);
    let mut todo: Vec<(Time, Key)> = Vec::new();
    for index in first..end {
        let at = rate.frame_start(index);
        let description = frame::describe(project, seq, at);
        if description.layers.is_empty() {
            continue;
        }
        let Some(key) = frame_key(project, &description) else {
            continue;
        };
        // A frame the same as one before it (a still, a held frame) once.
        if !store.has(key) && !todo.iter().rev().take(8).any(|(_, k)| *k == key) {
            todo.push((at, key));
        }
    }
    todo
}

/// The stretches of `range` that play from the render cache: runs of
/// frames that are kept, or are gaps, as `(start, end)` on the timeline.
pub fn kept_stretches(
    project: &Project,
    sequence: SequenceId,
    range: (Time, Time),
    store: &Store,
) -> Vec<(Time, Time)> {
    let Some(seq) = project.sequence(sequence) else {
        return Vec::new();
    };
    let rate = seq.settings.frame_rate;
    let (first, end) = frames(seq.length(), Some(range), rate);
    let mut out: Vec<(Time, Time)> = Vec::new();
    for index in first..end {
        let at = rate.frame_start(index);
        let description = frame::describe(project, seq, at);
        let kept = description.layers.is_empty()
            || frame_key(project, &description).is_some_and(|key| store.has(key));
        if !kept {
            continue;
        }
        let until = rate.frame_start(index + 1);
        match out.last_mut() {
            Some(last) if last.1 == at => last.1 = until,
            _ => out.push((at, until)),
        }
    }
    out
}

fn run(
    project: &Project,
    sequence: SequenceId,
    range: (Time, Time),
    devices: Devices,
    store: &Store,
    shared: &Shared,
    notify: &dyn Fn(),
) -> diag::Result<Option<Done>> {
    let seq = project
        .sequence(sequence)
        .ok_or_else(|| cannot("The timeline is not in the project any more."))?;
    let rate = seq.settings.frame_rate;
    let (first, end) = frames(seq.length(), Some(range), rate);
    let todo = still_to_do(project, sequence, range, store);
    let all = (end - first).max(0) as u64;
    shared.frames.store(todo.len() as u64, Ordering::Relaxed);
    let passed = all.saturating_sub(todo.len() as u64);
    if todo.is_empty() {
        return Ok(Some(Done { written: 0, passed }));
    }
    let size = kept_size((seq.settings.width, seq.settings.height));
    let device = devices.composing.device.clone();
    let reader = render::caught(&device, || Reader::new(&device, size)).map_err(|err| {
        cannot(format!(
            "The graphics card can't read back a picture of {}×{} ({}).",
            size.0,
            size.1,
            render::caught::describe(&err)
        ))
        .fix("Update the graphics driver, or try again after closing other programs.")
    })?;
    let mut picturing = Picturing::start(devices, reader, size, || {});
    let mut writer = store.writer()?;
    let (_, spares) = channel::<Vec<u8>>();
    let ask = |p: &Picturing, at: Time| p.preview.show(project, sequence, at, size.0, size.1);
    ask(&picturing, todo[0].0);
    let mut written = 0;
    for (i, (at, key)) in todo.iter().enumerate() {
        if !shared.carry_on() {
            writer.finish()?;
            return Ok(None);
        }
        let Some(shown) = wait_for(&picturing.preview, *at, shared)? else {
            writer.finish()?;
            return Ok(None);
        };
        // The next frame is put together meanwhile.
        if let Some((next, _)) = todo.get(i + 1) {
            ask(&picturing, *next);
        }
        if let Out::Picture(nv12) = picturing.read_back(shown, size, size, &spares)? {
            writer.add(*key, &nv12_to_rgba(&nv12, size), size)?;
            written += 1;
        }
        shared.frames_done.store(i as u64 + 1, Ordering::Relaxed);
        if i % 15 == 0 {
            notify();
        }
    }
    writer.finish()?;
    Ok(Some(Done { written, passed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Look};

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    fn recording() -> (Project, SequenceId) {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        (
            Command::Import { sequence: seq, media }.apply(&project).unwrap(),
            seq,
        )
    }

    fn key_at(project: &Project, seq: SequenceId, at: Time) -> Option<Key> {
        frame_key(
            project,
            &frame::describe(project, project.sequence(seq).unwrap(), at),
        )
    }

    #[test]
    fn a_frames_key_follows_what_it_shows_not_where_it_is() {
        let (p, seq) = recording();
        let at = Time::from_ratio(1, 2).unwrap();
        let key = key_at(&p, seq, at).unwrap();
        assert_eq!(key_at(&p, seq, at), Some(key), "the same every time");
        assert_ne!(key_at(&p, seq, Time::seconds(1)), Some(key), "another moment");

        // Its colour changed: another picture.
        let clip = p.sequence(seq).unwrap().video_tracks[0].clips[0].clone();
        let mut look = clip.look;
        look.exposure = Look::default().exposure + 200_000;
        let graded = Command::SetLook {
            sequence: seq,
            clip: clip.id,
            look,
        }
        .apply(&p)
        .unwrap();
        assert_ne!(key_at(&graded, seq, at), Some(key));

        // Moved later on the timeline: the same picture, a second later.
        let moved = Command::MoveClip {
            sequence: seq,
            clip: clip.id,
            track: None,
            start: Time::seconds(1),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(key_at(&moved, seq, at + Time::seconds(1)), Some(key));
    }

    #[test]
    fn kept_frames_are_at_most_the_viewers_size() {
        assert_eq!(kept_size((2560, 1440)), (1920, 1080));
        assert_eq!(kept_size((1280, 720)), (1280, 720));
        assert_eq!(kept_size((1080, 1920)), (608, 1080));
    }

    #[test]
    fn frames_kept_already_and_gaps_are_passed_over() {
        let (p, seq) = recording();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let rate = p.sequence(seq).unwrap().settings.frame_rate;
        let range = (Time::ZERO, rate.frame_start(10));
        let todo = still_to_do(&p, seq, range, &store);
        assert_eq!(todo.len(), 10);
        let mut writer = store.writer().unwrap();
        writer.add(todo[3].1, &[0; 16], (2, 2)).unwrap();
        writer.finish().unwrap();
        assert_eq!(still_to_do(&p, seq, range, &store).len(), 9);
        // Kept: the third frame (index 3) and the stretch past it is not.
        let kept = kept_stretches(&p, seq, range, &store);
        assert_eq!(kept, vec![(rate.frame_start(3), rate.frame_start(4))]);
        // Past the end of the timeline: nothing there.
        let past = (Time::seconds(100), Time::seconds(101));
        assert!(still_to_do(&p, seq, past, &store).is_empty());
    }
}
