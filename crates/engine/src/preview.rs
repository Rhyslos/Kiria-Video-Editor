//! The preview worker: decodes the picture at the playhead on its own thread.
//!
//! The interface calls [`Preview::show`] whenever the playhead moves; only the
//! newest request counts, so scrubbing never builds a queue. Finished pictures
//! come back through [`Preview::latest`], and a notifier (the window's repaint
//! request) runs when one is ready.
//!
//! Getting to a moment in a file can mean decoding a hundred frames from the
//! key frame before it. A request that arrives meanwhile is looked at between
//! frames ([`Waiting`]): a decode the newer request can't use is given up,
//! so a click does not wait for the jump before it; a decode that is on
//! the way to it carries on, and the pictures it passes are shown. The
//! decode started after one was given up is always finished, so however
//! fast requests come (dragging the playhead), pictures keep coming too.
//! Not every request gets an answer, but the newest always does.
//!
//! Decoders are kept open for the files used most recently, so moving around
//! one recording only seeks.
//!
//! A picture comes back as the decoder's YUV planes when the viewer can
//! convert colour while drawing ([`Convert::WhileDrawing`]) and the file's
//! format allows; otherwise as RGBA, converted here on the worker.
//!
//! Each file is decoded the fastest way that works for it, and steps down
//! when a way fails, at opening or at any picture ([`ladder`]): on the
//! window's own graphics device when the editor hands one over
//! ([`OnDevice`]), then on the GPU's video decoder with a copy back, then on
//! the CPU. Stepping down is logged with the reason and shows no error; only
//! the last way's failure does.

use crate::timeline::video_at;
use media::{Decoded, Decoding, Level, Newer, Reached, VideoDecoder, WindowDevice, Work};
use project::{MediaId, Project, SequenceId, Time};
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How many files keep an open decoder.
const OPEN_DECODERS: usize = 4;

/// Where a picture's colour is converted for the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convert {
    /// The viewer's shader converts the decoder's planes while drawing.
    /// Formats that can't go that way still come as RGBA, and so do planes
    /// with a side longer than `largest_side` (the most a texture can be).
    WhileDrawing { largest_side: u32 },
    /// Every picture is converted to RGBA on the worker.
    OnCpu,
}

/// Told when the worker decodes on the window's device, so that a driver
/// which crashes or freezes there can be noticed at the next start
/// (`graphics` keeps a marker file for it).
pub trait DeviceWatch: Send + Sync {
    /// Called before anything is asked of the device: opening a file on it,
    /// or decoding a picture. One of the other two follows when it comes back.
    fn about_to_decode(&self);
    /// The device did what it was asked.
    fn answered(&self);
    /// The device reported an error instead. That is handled here (the file
    /// steps down), so it is not a crash.
    fn reported_an_error(&self);
}

/// The window's graphics device for decoding on, and who watches it.
#[derive(Clone)]
pub struct OnDevice {
    pub device: Arc<WindowDevice>,
    pub watch: Arc<dyn DeviceWatch>,
}

impl std::fmt::Debug for OnDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OnDevice({:?})", self.device)
    }
}

/// The ways a file may be decoded, in the order they are tried. Decoding on
/// the window's device needs the device, the viewer's shader to draw from
/// it, and `Decoding::Best`.
pub fn ladder(decoding: Decoding, convert: Convert, on_device: bool) -> &'static [Level] {
    match (decoding, convert) {
        (Decoding::Best, Convert::WhileDrawing { .. }) if on_device => {
            &[Level::Device, Level::Hardware, Level::Software]
        }
        (Decoding::Best, _) => &[Level::Hardware, Level::Software],
        (Decoding::Software, _) => &[Level::Software],
    }
}

/// The ways left after `failed`: what a file steps down through.
pub fn below(ladder: &'static [Level], failed: Level) -> &'static [Level] {
    let first = ladder.iter().position(|l| *l > failed).unwrap_or(ladder.len());
    &ladder[first..]
}

/// A picture for one moment of the timeline.
#[derive(Debug)]
pub struct Picture {
    /// The timeline time asked for.
    pub at: Time,
    /// `None` in a gap (black), or when the frame couldn't be decoded (see `error`).
    pub frame: Option<Decoded>,
    pub error: Option<diag::AppError>,
    /// How long decoding took, with copying the planes out or converting to RGBA.
    pub took: Duration,
    /// Where the decoder's part of that went, when a picture was decoded.
    pub work: Option<Work>,
    /// Which decoder made it, for the viewer's corner.
    pub method: Option<String>,
}

struct Request {
    project: Project,
    sequence: SequenceId,
    at: Time,
    max_width: u32,
    max_height: u32,
}

pub struct Preview {
    /// `None` once closed: nothing more is asked for.
    requests: Option<Sender<Request>>,
    pictures: Receiver<Picture>,
    /// Disconnects when the worker thread has ended.
    ended: Receiver<()>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Preview")
    }
}

impl Preview {
    /// Starts the worker. `notify` runs on the worker each time a picture is ready.
    /// `on_device` is the window's graphics device, when video may be decoded on it.
    pub fn start(
        decoding: Decoding,
        convert: Convert,
        on_device: Option<OnDevice>,
        notify: impl Fn() + Send + 'static,
    ) -> Preview {
        let (requests, inbox) = channel::<Request>();
        let (outbox, pictures) = channel::<Picture>();
        let worker = Worker {
            ladder: ladder(decoding, convert, on_device.is_some()),
            convert,
            on_device,
            decoders: VecDeque::new(),
            off_device: HashSet::new(),
            gave_up_last: false,
        };
        let (running, ended) = channel::<()>();
        let spawned = std::thread::Builder::new().name("preview".into()).spawn(move || {
            run(&inbox, &outbox, worker, &notify);
            // The worker and its decoders are gone by now.
            drop(running);
        });
        if let Err(err) = spawned {
            tracing::error!("the preview worker could not start: {err}");
        }
        Preview {
            requests: Some(requests),
            pictures,
            ended,
        }
    }

    /// Stops the worker and waits until it has ended, at most `timeout`:
    /// a decode under way finishes (one not started yet is dropped), the
    /// files are closed, and everything it held on the window's graphics
    /// device is given back. Returns whether it ended in time.
    ///
    /// For closing the app, before the window's graphics go away: a worker
    /// still decoding on the graphics device when the program ends can take
    /// the program down with it. Dropping a `Preview` stops the worker too,
    /// without waiting.
    pub fn close(&mut self, timeout: Duration) -> bool {
        self.requests = None;
        matches!(
            self.ended.recv_timeout(timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        )
    }

    /// Asks for the picture at `at` on `sequence`, fitted in `max_width × max_height`
    /// (planes come at the nearest halving that still fills it).
    /// Replaces any request not started yet.
    pub fn show(&self, project: &Project, sequence: SequenceId, at: Time, max_width: u32, max_height: u32) {
        let Some(requests) = &self.requests else {
            return;
        };
        let _ = requests.send(Request {
            project: project.clone(),
            sequence,
            at,
            max_width,
            max_height,
        });
    }

    /// The newest finished picture, if any arrived since the last call.
    pub fn latest(&self) -> Option<Picture> {
        let mut newest = None;
        while let Ok(p) = self.pictures.try_recv() {
            newest = Some(p);
        }
        newest
    }
}

fn run(inbox: &Receiver<Request>, outbox: &Sender<Picture>, mut worker: Worker, notify: &dyn Fn()) {
    let mut waiting = Waiting::new(inbox);
    while let Some(request) = waiting.next() {
        // A decode the next request can't use is given up: nothing to send.
        let Some(picture) = worker.picture(&request, &mut waiting) else {
            continue;
        };
        if outbox.send(picture).is_err() {
            return;
        }
        notify();
    }
}

/// The requests that have arrived and not been worked on: only the newest
/// counts. It is taken in while a picture is being decoded too, so the
/// decode can be asked whether it is still wanted.
struct Waiting<'a> {
    inbox: &'a Receiver<Request>,
    newest: Option<Request>,
    /// Nobody asks any more: the preview was closed or dropped.
    closed: bool,
}

impl<'a> Waiting<'a> {
    fn new(inbox: &'a Receiver<Request>) -> Waiting<'a> {
        Waiting {
            inbox,
            newest: None,
            closed: false,
        }
    }

    /// Takes in what has arrived. Returns whether a newer request did.
    fn take_in(&mut self) -> bool {
        let mut newer = false;
        loop {
            match self.inbox.try_recv() {
                Ok(request) => {
                    self.newest = Some(request);
                    newer = true;
                }
                Err(TryRecvError::Empty) => return newer,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    return newer;
                }
            }
        }
    }

    /// The request to work on next: the newest waiting, or the next to
    /// arrive. `None` when nobody asks any more.
    fn next(&mut self) -> Option<Request> {
        self.take_in();
        if self.newest.is_none() && !self.closed {
            self.newest = self.inbox.recv().ok();
            self.take_in();
        }
        if self.closed { None } else { self.newest.take() }
    }
}

/// The file, stream and moment in the file a request shows; `None` in a gap.
fn source_of(r: &Request) -> Option<((MediaId, u32), f64)> {
    let sequence = r.project.sequences.iter().find(|s| s.id == r.sequence)?;
    let showing = video_at(&r.project, sequence, r.at)?;
    Some((
        (showing.media.id, showing.clip.stream),
        showing.source_time.as_secs_f64(),
    ))
}

/// What the worker thread keeps between requests.
struct Worker {
    /// The ways to decode a file, in the order they are tried.
    ladder: &'static [Level],
    convert: Convert,
    on_device: Option<OnDevice>,
    /// Open decoders, most recently used first.
    decoders: VecDeque<((MediaId, u32), VideoDecoder)>,
    /// Files the window's device failed for: not tried there again in this
    /// session, also once their decoder has been closed to make room.
    off_device: HashSet<(MediaId, u32)>,
    /// The last decode was given up for a newer request: the next one is finished.
    gave_up_last: bool,
}

impl Worker {
    /// The picture for `r`, or for a request that arrived while it was
    /// decoded and was reached first (the picture's `at` says which).
    /// `None` when the decode was given up for a newer request it can't serve.
    fn picture(&mut self, r: &Request, waiting: &mut Waiting<'_>) -> Option<Picture> {
        let started = Instant::now();
        let mut out = Picture {
            at: r.at,
            frame: None,
            error: None,
            took: Duration::ZERO,
            work: None,
            method: None,
        };
        // Whatever is answered without decoding counts as a finished answer.
        let gave_up_last = std::mem::take(&mut self.gave_up_last);
        let Some(sequence) = r.project.sequences.iter().find(|s| s.id == r.sequence) else {
            return Some(out);
        };
        let Some(showing) = video_at(&r.project, sequence, r.at) else {
            return Some(out); // a gap: black
        };
        let key = (showing.media.id, showing.clip.stream);
        let file = File {
            path: &showing.media.path,
            stream: showing.clip.stream,
            rotation: showing.media.info.video.as_ref().map_or(0, |v| v.rotation),
        };
        let found = self.decoders.iter().position(|(k, _)| *k == key);
        let mut decoder = match found.and_then(|i| self.decoders.remove(i)) {
            Some((_, decoder)) => decoder,
            None => match self.open(key, &file, self.ladder_for(key)) {
                Ok(decoder) => decoder,
                Err(err) => {
                    out.error = Some(err);
                    return Some(out);
                }
            },
        };
        let at = showing.source_time.as_secs_f64();
        // What the decoder is told between frames: what the newest request
        // waiting means for this file. Moments of this file that were asked
        // for meanwhile are remembered with their place on the timeline, to
        // name the picture if the decode stops at one of them.
        let mut asked_meanwhile: Vec<(f64, Time)> = Vec::new();
        let mut now_wanted = Newer::Nothing;
        // A decode that follows one that was given up is finished, whatever
        // arrives: requests that keep coming must not keep pictures from
        // coming.
        let give_up = !gave_up_last;
        let mut newer = || {
            if waiting.take_in()
                && let Some(next) = &waiting.newest
            {
                now_wanted = match source_of(next) {
                    Some((k, moment)) if k == key => {
                        asked_meanwhile.push((moment, next.at));
                        Newer::At { moment, give_up }
                    }
                    _ if give_up => Newer::Elsewhere,
                    _ => Newer::Nothing,
                };
            }
            if waiting.closed {
                Newer::Elsewhere
            } else {
                now_wanted
            }
        };
        // Decode; when a way fails for this file (an unusual profile, a
        // driver problem), open it one way down and ask again.
        let mut dropped = false;
        loop {
            match self.decode(&mut decoder, at, r, &mut newer) {
                Ok(Reached::Shown { picture, at: reached }) => {
                    out.frame = Some(picture);
                    // Stopped at a moment asked for meanwhile: it is that request's picture.
                    if (reached - at).abs() > 1e-9
                        && let Some((_, on_timeline)) = asked_meanwhile
                            .iter()
                            .rev()
                            .find(|(moment, _)| (moment - reached).abs() <= 1e-9)
                    {
                        out.at = *on_timeline;
                    }
                    break;
                }
                Ok(Reached::Dropped) => {
                    dropped = true;
                    break;
                }
                Err(err) => {
                    let failed = decoder.level();
                    if failed == Level::Device {
                        self.off_device.insert(key);
                        if let Some(on_device) = &self.on_device {
                            on_device.watch.reported_an_error();
                        }
                    }
                    let lower = below(self.ladder, failed);
                    if lower.is_empty() {
                        out.error = Some(err);
                        break;
                    }
                    tracing::warn!(file = %file.path.display(), "decoding with {} failed, stepping down: {}", decoder.method, err.report());
                    match self.open(key, &file, lower) {
                        Ok(next) => decoder = next,
                        Err(_) => {
                            out.error = Some(err);
                            break;
                        }
                    }
                }
            }
        }
        out.method = Some(decoder.method.clone());
        out.work = Some(decoder.last_work());
        // Most recently used first.
        self.decoders.push_front((key, decoder));
        self.decoders.truncate(OPEN_DECODERS);
        self.gave_up_last = dropped;
        if dropped {
            return None;
        }
        // The waiting request has its answer already when the decode stopped at it.
        if waiting.newest.as_ref().is_some_and(|next| next.at == out.at) {
            waiting.newest = None;
        }
        out.took = started.elapsed();
        Some(out)
    }

    /// The ways to try for a file: all of them, or those below the window's
    /// device once it has failed for this file.
    fn ladder_for(&self, key: (MediaId, u32)) -> &'static [Level] {
        if self.off_device.contains(&key) {
            below(self.ladder, Level::Device)
        } else {
            self.ladder
        }
    }

    /// Opens the file the first way of `ways` that works for it. On the
    /// window's device that may fail for many reasons (a kind of video it
    /// doesn't decode, a driver problem): the next way is tried then. The
    /// other ways only fail when the file itself can't be opened.
    fn open(&mut self, key: (MediaId, u32), file: &File<'_>, ways: &[Level]) -> diag::Result<VideoDecoder> {
        let mut last = None;
        for way in ways {
            let opened = match (way, &self.on_device) {
                (Level::Device, Some(on_device)) => {
                    on_device.watch.about_to_decode();
                    let opened = VideoDecoder::open_on_device(
                        file.path,
                        file.stream,
                        file.rotation,
                        &on_device.device,
                    );
                    if opened.is_ok() {
                        on_device.watch.answered();
                    }
                    if let Err(err) = &opened {
                        self.off_device.insert(key);
                        on_device.watch.reported_an_error();
                        tracing::info!(file = %file.path.display(), "not decoded on the window's device: {}", err.report());
                    }
                    opened
                }
                (Level::Device, None) => continue,
                (Level::Hardware, _) => {
                    VideoDecoder::open(file.path, file.stream, file.rotation, Decoding::Best)
                }
                (Level::Software, _) => {
                    VideoDecoder::open(file.path, file.stream, file.rotation, Decoding::Software)
                }
            };
            match opened {
                Ok(decoder) => {
                    tracing::info!(file = %file.path.display(), method = %decoder.method, "opened video");
                    return Ok(decoder);
                }
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| {
            diag::AppError::new("The video can't be shown").why("No way of decoding it is left to try.")
        }))
    }

    /// The picture at `at` from this decoder, the way the viewer takes it.
    /// Decodes towards `at`, asking `newer` between frames whether that is
    /// still what is wanted (`VideoDecoder::picture_at_while`).
    fn decode(
        &self,
        decoder: &mut VideoDecoder,
        at: f64,
        r: &Request,
        newer: &mut dyn FnMut() -> Newer,
    ) -> diag::Result<Reached<Decoded>> {
        let (w, h) = (r.max_width, r.max_height);
        let rgba = |reached: Reached<media::Frame>| match reached {
            Reached::Shown { picture, at } => Reached::Shown {
                picture: Decoded::Rgba(picture),
                at,
            },
            Reached::Dropped => Reached::Dropped,
        };
        let Convert::WhileDrawing { largest_side } = self.convert else {
            return decoder.frame_at_while(at, w, h, newer).map(rgba);
        };
        let watch = match (&self.on_device, decoder.level()) {
            (Some(on_device), Level::Device) => Some(&on_device.watch),
            _ => None,
        };
        if let Some(watch) = watch {
            watch.about_to_decode();
        }
        let reached = decoder.picture_at_while(at, w, h, newer);
        if let (Some(watch), Ok(_)) = (watch, &reached) {
            watch.answered(); // an error is reported by the caller
        }
        let (picture, at) = match reached? {
            Reached::Shown { picture, at } => (picture, at),
            Reached::Dropped => return Ok(Reached::Dropped),
        };
        let picture = match picture {
            // The frame is held already: this only converts it.
            Decoded::Yuv(planes) if planes.width.max(planes.height) > largest_side => {
                Decoded::Rgba(decoder.frame_at(at, w, h)?)
            }
            // Pictures on the graphics card come at the file's size: one too
            // large for a texture goes down a way instead.
            Decoded::Gpu(frame) if frame.planes.size().0.max(frame.planes.size().1) > largest_side => {
                return Err(diag::AppError::new("The video can't be shown")
                    .why("The picture is larger than a texture on this graphics card can be."));
            }
            picture => picture,
        };
        Ok(Reached::Shown { picture, at })
    }
}

/// The file a request shows.
struct File<'a> {
    path: &'a Path,
    stream: u32,
    rotation: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Command;
    use std::path::Path;

    const PLANES: Convert = Convert::WhileDrawing { largest_side: 8192 };

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    fn wait(preview: &Preview) -> Picture {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(p) = preview.latest() {
                return p;
            }
            assert!(Instant::now() < deadline, "no picture in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pictures_for_clips_and_black_for_gaps() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, Convert::OnCpu, None, || {});

        preview.show(&project, seq, Time::seconds(1), 160, 160);
        let p = wait(&preview);
        let frame = p.frame.expect("a picture");
        assert!(matches!(frame, Decoded::Rgba(_)));
        assert_eq!(frame.size(), (160, 90));
        assert!((frame.time() - 1.0).abs() < 0.02);
        assert!(p.error.is_none());

        preview.show(&project, seq, Time::seconds(30), 160, 160);
        let gap = wait(&preview);
        assert!(
            gap.frame.is_none() && gap.error.is_none(),
            "after the last clip is black"
        );
    }

    #[test]
    fn planes_for_the_viewer_to_convert() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, PLANES, None, || {});

        preview.show(&project, seq, Time::seconds(1), 320, 320);
        let p = wait(&preview);
        let Some(Decoded::Yuv(frame)) = p.frame else {
            panic!("planes, got {:?}", p.frame);
        };
        assert_eq!((frame.width, frame.height), (320, 180));
        assert!((frame.time - 1.0).abs() < 0.02);
        // A smaller viewer gets the planes halved.
        preview.show(&project, seq, Time::seconds(1), 160, 160);
        assert_eq!(wait(&preview).frame.expect("a picture").size(), (160, 90));
    }

    #[test]
    fn planes_too_big_for_a_texture_come_as_rgba() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let small = Convert::WhileDrawing { largest_side: 256 };
        let preview = Preview::start(Decoding::Software, small, None, || {});
        preview.show(&project, seq, Time::seconds(1), 320, 320);
        let frame = wait(&preview).frame.expect("a picture");
        assert!(matches!(frame, Decoded::Rgba(_)), "{frame:?}");
        assert_eq!(frame.size(), (320, 180));
    }

    #[test]
    fn each_file_goes_down_the_ladder_one_way_at_a_time() {
        use Level::{Device, Hardware, Software};
        let full = ladder(Decoding::Best, PLANES, true);
        assert_eq!(full, [Device, Hardware, Software]);
        assert_eq!(below(full, Device), [Hardware, Software]);
        assert_eq!(below(full, Hardware), [Software]);
        assert!(below(full, Software).is_empty(), "nothing under the CPU");
        // Without the device, or with a viewer that can't draw from it, or
        // when only the CPU is wanted, the device is never tried.
        assert_eq!(ladder(Decoding::Best, PLANES, false), [Hardware, Software]);
        assert_eq!(ladder(Decoding::Best, Convert::OnCpu, true), [Hardware, Software]);
        assert_eq!(ladder(Decoding::Software, PLANES, true), [Software]);
        // "The best way" may itself have ended on the CPU: nothing is below that.
        assert!(below(ladder(Decoding::Best, PLANES, false), Software).is_empty());
    }

    /// Counts what the worker tells the watcher.
    #[derive(Default)]
    struct Counting {
        about_to: std::sync::atomic::AtomicU32,
        answered: std::sync::atomic::AtomicU32,
        errors: std::sync::atomic::AtomicU32,
    }

    impl DeviceWatch for Counting {
        fn about_to_decode(&self) {
            self.about_to.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn answered(&self) {
            self.answered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn reported_an_error(&self) {
            self.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[test]
    fn a_file_the_device_turns_down_is_decoded_the_next_way_without_an_error() {
        // A device without a window, on whatever Vulkan this machine has. It
        // decodes no video, so every file is turned down by it.
        let Some(device) = video_device::testing::device() else {
            return;
        };
        let device = WindowDevice::new(device).unwrap();
        let watch = Arc::new(Counting::default());
        let on_device = OnDevice {
            device,
            watch: Arc::clone(&watch) as Arc<dyn DeviceWatch>,
        };
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Best, PLANES, Some(on_device), || {});
        preview.show(&project, seq, Time::seconds(1), 320, 320);
        let p = wait(&preview);
        assert!(p.error.is_none(), "{:?}", p.error.map(|e| e.report()));
        assert!(matches!(p.frame, Some(Decoded::Yuv(_))), "{:?}", p.frame);
        assert!(!p.method.unwrap().contains("stays on the GPU"));
        let count = |n: &std::sync::atomic::AtomicU32| n.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!((count(&watch.about_to), count(&watch.errors)), (1, 1));
        assert_eq!(
            count(&watch.answered),
            0,
            "turned down, which is an answer of its own"
        );
        // The file stays on the way that worked: the device isn't asked again.
        preview.show(&project, seq, Time::from_ratio(3, 2).unwrap(), 320, 320);
        assert!(wait(&preview).frame.is_some());
        assert_eq!((count(&watch.about_to), count(&watch.errors)), (1, 1));
    }

    #[test]
    fn a_file_the_device_failed_for_is_not_tried_there_again() {
        use Level::{Device, Hardware, Software};
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let (failed, other) = ((media.id, 0), (media.id, 1));
        let mut worker = Worker {
            ladder: ladder(Decoding::Best, PLANES, true),
            convert: PLANES,
            on_device: None,
            decoders: VecDeque::new(),
            off_device: HashSet::new(),
            gave_up_last: false,
        };
        assert_eq!(worker.ladder_for(failed), [Device, Hardware, Software]);
        worker.off_device.insert(failed);
        // Also after its decoder was closed to make room for other files.
        assert_eq!(worker.ladder_for(failed), [Hardware, Software]);
        assert_eq!(worker.ladder_for(other), [Device, Hardware, Software]);
    }

    #[test]
    fn closing_waits_for_the_worker_and_asks_for_nothing_more() {
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
        let mut preview = Preview::start(Decoding::Software, Convert::OnCpu, None, || {});
        preview.show(&p, seq, Time::seconds(1), 640, 360);
        assert!(wait(&preview).frame.is_some());
        // One more is asked for and may be under way when it closes.
        preview.show(&p, seq, Time::from_ratio(3, 2).unwrap(), 640, 360);
        assert!(preview.close(Duration::from_secs(20)), "the worker ended");
        let _ = preview.latest();
        preview.show(&p, seq, Time::seconds(2), 640, 360);
        std::thread::sleep(Duration::from_millis(100));
        assert!(preview.latest().is_none(), "nothing is decoded after closing");
        assert!(preview.close(Duration::from_millis(10)), "closing twice is fine");
    }

    /// A project with the two-second fixture on the timeline, and a worker
    /// that decodes on the CPU.
    fn worker_and_project() -> (Worker, Project, SequenceId) {
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
        let worker = Worker {
            ladder: ladder(Decoding::Software, Convert::OnCpu, false),
            convert: Convert::OnCpu,
            on_device: None,
            decoders: VecDeque::new(),
            off_device: HashSet::new(),
            gave_up_last: false,
        };
        (worker, p, seq)
    }

    fn request(p: &Project, seq: SequenceId, millis: i64) -> Request {
        Request {
            project: p.clone(),
            sequence: seq,
            at: Time::from_ratio(millis, 1000).unwrap(),
            max_width: 160,
            max_height: 160,
        }
    }

    #[test]
    fn only_the_newest_request_waiting_counts() {
        let (_, p, seq) = worker_and_project();
        let (requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        for millis in [100, 200, 300] {
            requests.send(request(&p, seq, millis)).unwrap();
        }
        assert_eq!(waiting.next().map(|r| r.at), Time::from_ratio(300, 1000));
        requests.send(request(&p, seq, 400)).unwrap();
        assert!(waiting.take_in(), "a newer one arrived");
        assert!(!waiting.take_in(), "nothing since");
        assert_eq!(waiting.next().map(|r| r.at), Time::from_ratio(400, 1000));
        // Nobody asks any more: what was still waiting is not worked on.
        requests.send(request(&p, seq, 500)).unwrap();
        drop(requests);
        assert!(waiting.next().is_none());
    }

    #[test]
    fn a_jump_overtaken_by_a_request_elsewhere_is_given_up() {
        let (mut worker, p, seq) = worker_and_project();
        let (requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        // While 1.8 s is decoded, a moment after the clip's end (a gap) is asked for.
        requests.send(request(&p, seq, 30_000)).unwrap();
        assert!(worker.picture(&request(&p, seq, 1800), &mut waiting).is_none());
        // The newer request is still waiting, and is answered next.
        let next = waiting.next().unwrap();
        assert_eq!(Some(next.at), Time::from_ratio(30_000, 1000));
        let gap = worker.picture(&next, &mut waiting).unwrap();
        assert!(gap.frame.is_none() && gap.error.is_none());
    }

    #[test]
    fn the_decode_after_one_given_up_is_finished_whatever_arrives() {
        let (mut worker, p, seq) = worker_and_project();
        let (requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        let elsewhere = 30_000; // after the clip's end
        worker.picture(&request(&p, seq, 1900), &mut waiting).unwrap();
        // A jump back, overtaken by a request this decode can't serve: given up.
        requests.send(request(&p, seq, elsewhere)).unwrap();
        assert!(worker.picture(&request(&p, seq, 500), &mut waiting).is_none());
        // The next decode is finished, though it is overtaken the same way.
        requests.send(request(&p, seq, elsewhere + 1)).unwrap();
        let picture = worker.picture(&request(&p, seq, 1000), &mut waiting).unwrap();
        assert_eq!(Some(picture.at), Time::from_ratio(1000, 1000));
        assert!(picture.frame.is_some());
        assert!(
            picture.work.is_some_and(|work| work.frames > 30),
            "{:?}",
            picture.work
        );
        // After a finished one, giving up is allowed again.
        requests.send(request(&p, seq, elsewhere + 2)).unwrap();
        assert!(worker.picture(&request(&p, seq, 300), &mut waiting).is_none());
    }

    #[test]
    fn a_request_reached_on_the_way_gets_the_picture() {
        let (mut worker, p, seq) = worker_and_project();
        let (requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        // While 1.8 s is decoded from the start, 0.5 s is asked for: the
        // decode stops there, and the picture is that request's.
        requests.send(request(&p, seq, 500)).unwrap();
        let picture = worker.picture(&request(&p, seq, 1800), &mut waiting).unwrap();
        assert_eq!(Some(picture.at), Time::from_ratio(500, 1000));
        let frame = picture.frame.unwrap();
        assert!((frame.time() - 0.5).abs() < 0.02, "{}", frame.time());
        assert!(waiting.newest.is_none(), "answered: not decoded a second time");
        // A later moment on the way does not stop the one before it.
        requests.send(request(&p, seq, 1900)).unwrap();
        let picture = worker.picture(&request(&p, seq, 1200), &mut waiting).unwrap();
        assert_eq!(Some(picture.at), Time::from_ratio(1200, 1000));
        assert_eq!(
            waiting.newest.as_ref().map(|r| r.at),
            Time::from_ratio(1900, 1000)
        );
    }

    #[test]
    fn a_missing_file_is_explained() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let mut media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        media.path = fixture("moved-away.mkv");
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, PLANES, None, || {});
        preview.show(&project, seq, Time::seconds(1), 160, 160);
        let p = wait(&preview);
        assert!(p.frame.is_none());
        assert!(p.error.unwrap().what().contains("can't be shown"));
    }
}
