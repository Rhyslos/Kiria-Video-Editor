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
//! Frames decoded on the window's graphics device are kept there after
//! they were decoded ([`crate::frames`]): the one shown and every one
//! passed on the way to it. Going back over them (a step back, a drag,
//! playing a stretch again) shows them at once, without the decoder.
//!
//! When nothing has been asked for a moment, the worker fills in behind the
//! playhead ([`Worker::fill`]): it decodes the stretch before the frames
//! that are kept, from the key frame before them, so that going back
//! across a key frame finds its frames there too. It does so on a decoder
//! of its own (the one at the playhead keeps its place) and stops between
//! frames as soon as anything is asked for.
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

use crate::compose::{Composed, Composer, Composing};
use crate::frame::{self, FrameDescription};
use crate::frames::FrameCache;
use crate::timeline::video_at;
use media::{Decoded, Decoding, GpuFrame, Level, Newer, Reached, VideoDecoder, WindowDevice, Work};
use project::{MediaId, Project, SequenceId, Time};
use render::Area;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How many files keep an open decoder.
const OPEN_DECODERS: usize = 4;

/// How long nothing must have been asked for before the worker fills in
/// behind the playhead: longer than the gap between two requests while
/// playing or dragging, so filling never competes with either.
const IDLE_AFTER: Duration = Duration::from_millis(80);

/// How much is kept behind the playhead (seconds of video), when the frames
/// are there to be kept: less than this behind it, and the stretch before
/// is filled in.
const KEPT_BEHIND: f64 = 2.0;

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
    /// How many bytes of decoded frames may be kept on the device
    /// ([`crate::frames::budget_on_card`]).
    pub frames_kept: u64,
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
    /// The top clip's picture, when the worker doesn't put pictures
    /// together itself. `None` in a gap (black), or when the frame couldn't
    /// be decoded (see `error`).
    pub frame: Option<Decoded>,
    /// The timeline's whole picture, put together on the window's device,
    /// when the worker was started with one ([`Preview::start_composing`]).
    /// `None` in a gap (black) or when nothing could be decoded.
    pub composed: Option<Composed>,
    /// The renderer could not take a picture decoded on the window's device:
    /// whoever started the worker starts one that decodes the usual way.
    pub refused: bool,
    /// Some of a composed picture had its colour converted on the CPU (a
    /// format the planes way doesn't take): work that a smaller size saves.
    pub converted_on_cpu: bool,
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
        Preview::start_composing(decoding, convert, on_device, None, notify)
    }

    /// Like [`Preview::start`]. With `composing` (the device the window is
    /// drawn with) the worker puts every visible layer together into the
    /// timeline's picture there, and pictures come back as
    /// [`Picture::composed`]; without, the top clip's picture comes back
    /// alone as [`Picture::frame`].
    pub fn start_composing(
        decoding: Decoding,
        convert: Convert,
        on_device: Option<OnDevice>,
        composing: Option<Composing>,
        notify: impl Fn() + Send + 'static,
    ) -> Preview {
        let (requests, inbox) = channel::<Request>();
        let (outbox, pictures) = channel::<Picture>();
        let worker = Worker {
            ladder: ladder(decoding, convert, on_device.is_some()),
            convert,
            frames: FrameCache::new(on_device.as_ref().map_or(0, |d| d.frames_kept)),
            on_device,
            decoders: VecDeque::new(),
            off_device: HashSet::new(),
            gave_up_last: false,
            composing,
            composer: None,
            playhead: None,
            filler: None,
            unfillable: HashSet::new(),
            stuck: None,
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

    /// The next finished picture, waiting up to `patience` for it: for
    /// work that asks for one picture after another (export), never for the
    /// interface.
    pub fn wait(&self, patience: Duration) -> Option<Picture> {
        self.pictures.recv_timeout(patience).ok()
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
    loop {
        let request = match waiting.next_within(IDLE_AFTER) {
            Next::Request(request) => request,
            Next::Closed => return,
            // Nothing asked for a moment: fill in behind the playhead, a
            // stretch at a time, until there is nothing to fill or
            // something is asked for.
            Next::Idle if worker.fill(&mut waiting) => continue,
            Next::Idle => match waiting.next() {
                Some(request) => request,
                None => return,
            },
        };
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

/// What came of waiting for a request for a while.
enum Next {
    Request(Request),
    /// Nothing was asked for in that time.
    Idle,
    /// Nobody asks any more.
    Closed,
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

    /// The request to work on next, waiting at most `patience` for one to arrive.
    fn next_within(&mut self, patience: Duration) -> Next {
        self.take_in();
        if self.newest.is_none() && !self.closed {
            match self.inbox.recv_timeout(patience) {
                Ok(request) => self.newest = Some(request),
                Err(RecvTimeoutError::Timeout) => return Next::Idle,
                Err(RecvTimeoutError::Disconnected) => self.closed = true,
            }
            self.take_in();
        }
        match self.newest.take() {
            Some(request) if !self.closed => Next::Request(request),
            _ => Next::Closed,
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
    /// The window's device, when pictures are put together here.
    composing: Option<Composing>,
    /// Made from it on this thread, the first time a picture is needed.
    composer: Option<Composer>,
    /// Frames decoded on the window's device, kept there.
    frames: FrameCache<(MediaId, u32), GpuFrame>,
    /// Where the playhead last was in a file whose frames are kept: what
    /// to fill in behind.
    playhead: Option<Playhead>,
    /// The decoder that fills in, and the file it has open.
    filler: Option<((MediaId, u32), VideoDecoder)>,
    /// Files that could not be filled in: not tried again.
    unfillable: HashSet<(MediaId, u32)>,
    /// A stretch (file, and where its kept frames begin) that a finished
    /// fill did not make any longer: left alone until that changes.
    stuck: Option<((MediaId, u32), f64)>,
}

/// A file the viewer is showing, and where in it.
#[derive(Debug, Clone)]
struct Playhead {
    key: (MediaId, u32),
    path: std::path::PathBuf,
    rotation: u16,
    /// Seconds from the start of the file.
    moment: f64,
    /// The size pictures are asked for at.
    size: (u32, u32),
}

/// The moment to decode up to so that the stretch before the frames kept
/// around `moment` is kept too, when it should be: `kept_from` is where the
/// kept frames begin. `None` when enough is kept behind `moment`, or the
/// kept frames begin at the start of the file.
fn fill_up_to(moment: f64, kept_from: f64, tick: f64) -> Option<f64> {
    // Just before the first kept frame: the frame on screen then is the last one missing.
    let last_missing = kept_from - tick - 0.0002;
    (moment - kept_from < KEPT_BEHIND && last_missing > 0.0).then_some(last_missing)
}

/// What the viewer's corner says for a picture that was kept.
const KEPT: &str = "kept on the graphics card";

impl Worker {
    /// The picture for `r`, or for a request that arrived while it was
    /// decoded and was reached first (the picture's `at` says which).
    /// `None` when the decode was given up for a newer request it can't serve.
    fn picture(&mut self, r: &Request, waiting: &mut Waiting<'_>) -> Option<Picture> {
        let started = Instant::now();
        // Whatever is answered without decoding counts as a finished answer.
        let gave_up_last = std::mem::take(&mut self.gave_up_last);
        self.playhead = None;
        let Some(sequence) = r.project.sequences.iter().find(|s| s.id == r.sequence) else {
            return Some(Picture::black(r.at));
        };
        if self.composing.is_some() {
            return self.composed(r, sequence, waiting, !gave_up_last, started);
        }
        let Some(showing) = video_at(&r.project, sequence, r.at) else {
            return Some(Picture::black(r.at)); // a gap: black
        };
        let target = Target {
            key: (showing.media.id, showing.clip.stream),
            path: &showing.media.path,
            rotation: showing.media.info.video.as_ref().map_or(0, |v| v.rotation),
            moment: showing.source_time.as_secs_f64(),
            size: (r.max_width, r.max_height),
        };
        let mut out = self.decoded(r.at, &target, waiting, Role::ALONE.give_up(!gave_up_last))?;
        out.took = started.elapsed();
        Some(out)
    }

    /// The timeline's whole picture for `r`: every layer that is seen,
    /// decoded and put together on the window's device.
    fn composed(
        &mut self,
        r: &Request,
        sequence: &project::Sequence,
        waiting: &mut Waiting<'_>,
        give_up: bool,
        started: Instant,
    ) -> Option<Picture> {
        let size = preview_size(
            (sequence.settings.width, sequence.settings.height),
            (r.max_width, r.max_height),
        );
        let mut description = frame::describe(&r.project, sequence, r.at).at_size(size);
        let mut out = Picture::black(r.at);
        if description.layers.is_empty() {
            return Some(out); // a gap: black
        }
        // One layer is the usual case and behaves as a picture alone does:
        // given up for a newer request, or carried on to it. With more, each
        // is decoded to the end, so they all show the same moment.
        let alone = description.layers.len() == 1;
        let top = description.layers.len() - 1;
        let mut pictures = Vec::with_capacity(description.layers.len());
        for (index, layer) in description.layers.iter().enumerate() {
            let Some(media) = r.project.media.get(&layer.media) else {
                continue;
            };
            let target = Target {
                key: (layer.media, layer.stream),
                path: &media.path,
                rotation: media.info.video.as_ref().map_or(0, |v| v.rotation),
                moment: layer.source_time.as_secs_f64(),
                size: pixels_needed(layer),
            };
            let role = Role {
                leads: index == top,
                follows: alone,
                give_up: alone && give_up,
            };
            let mut got = self.decoded(r.at, &target, waiting, role)?;
            if let Some(decoded) = got.frame.take() {
                out.converted_on_cpu |= matches!(decoded, Decoded::Rgba(_));
                pictures.push((index, decoded));
            }
            // The corner and Diagnostics describe the top layer; any
            // layer's problem is told.
            if index == top {
                out.at = got.at;
                out.work = got.work;
                out.method = got.method;
            }
            out.error = out.error.or(got.error);
        }
        // Reached a newer request's moment on the way: the layer sits where
        // it does at that moment.
        if out.at != r.at {
            let there = frame::describe(&r.project, sequence, out.at).at_size(size);
            if same_layers(&there, &description) {
                description = there;
            }
        }
        if pictures.is_empty() {
            out.took = started.elapsed();
            return Some(out);
        }
        let layers = pictures
            .into_iter()
            .map(|(index, decoded)| (description.layers[index].clone(), decoded))
            .collect();
        let composing = self.composing.clone()?;
        let composer = self.composer.get_or_insert_with(|| Composer::new(composing));
        match composer.compose(size, layers) {
            Ok(composed) => out.composed = Some(composed),
            Err(_) => out.refused = true,
        }
        out.took = started.elapsed();
        Some(out)
    }

    /// The picture of one file at one moment: kept, or decoded. `None` when
    /// the decode was given up for a newer request.
    fn decoded(
        &mut self,
        on_timeline: Time,
        target: &Target<'_>,
        waiting: &mut Waiting<'_>,
        role: Role,
    ) -> Option<Picture> {
        let mut out = Picture::black(on_timeline);
        let (key, at) = (target.key, target.moment);
        let file = File {
            path: target.path,
            stream: key.1,
            rotation: target.rotation,
        };
        if !role.leads {
            return self.decoded_below(out, target, &file, waiting);
        }
        // The frames around the playhead are the last to make room, and
        // what is filled in behind when nothing else is going on.
        self.frames.look_at(key, at);
        self.playhead = Some(Playhead {
            key,
            path: file.path.to_path_buf(),
            rotation: file.rotation,
            moment: at,
            size: target.size,
        });
        // Decoded before and still kept: nothing to decode.
        if let Some(frame) = self.frames.get(key, at) {
            out.frame = Some(Decoded::Gpu(frame));
            out.method = Some(KEPT.to_owned());
            return Some(out);
        }
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
        // What the decoder is told between frames: what the newest request
        // waiting means for this file. Moments of this file that were asked
        // for meanwhile are remembered with their place on the timeline, to
        // name the picture if the decode stops at one of them.
        let mut asked_meanwhile: Vec<(f64, Time)> = Vec::new();
        let mut now_wanted = Newer::Nothing;
        // A decode that follows one that was given up is finished, whatever
        // arrives: requests that keep coming must not keep pictures from
        // coming.
        let give_up = role.give_up;
        let mut newer = || {
            if waiting.take_in()
                && role.follows
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
            // Frames the decoder passes on the way are kept for going back to.
            let tick = decoder.tick();
            let frames = &mut self.frames;
            let mut keep = |picture: Decoded, until: f64| {
                if let Decoded::Gpu(frame) = picture {
                    let (w, h) = frame.planes.size();
                    let bytes = u64::from(w) * u64::from(h) * 3 / 2;
                    frames.put(key, frame.time, until, tick, bytes, frame);
                }
            };
            let decoded = decode(
                self.convert,
                self.on_device.as_ref(),
                &mut decoder,
                at,
                target.size,
                &mut newer,
                Some(&mut keep),
            );
            match decoded {
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
        Some(out)
    }

    /// The picture of a layer under the top one: kept, or decoded to the
    /// end whatever is asked for meanwhile (only closing stops it). It
    /// leaves the playhead, and so what is filled in behind it, alone.
    fn decoded_below(
        &mut self,
        mut out: Picture,
        target: &Target<'_>,
        file: &File<'_>,
        waiting: &mut Waiting<'_>,
    ) -> Option<Picture> {
        let (key, at) = (target.key, target.moment);
        if let Some(frame) = self.frames.get(key, at) {
            out.frame = Some(Decoded::Gpu(frame));
            out.method = Some(KEPT.to_owned());
            return Some(out);
        }
        let found = self.decoders.iter().position(|(k, _)| *k == key);
        let mut decoder = match found.and_then(|i| self.decoders.remove(i)) {
            Some((_, decoder)) => decoder,
            None => match self.open(key, file, self.ladder_for(key)) {
                Ok(decoder) => decoder,
                Err(err) => {
                    out.error = Some(err);
                    return Some(out);
                }
            },
        };
        let mut newer = || {
            waiting.take_in();
            if waiting.closed {
                Newer::Elsewhere
            } else {
                Newer::Nothing
            }
        };
        let mut closing = false;
        loop {
            let decoded = decode(
                self.convert,
                self.on_device.as_ref(),
                &mut decoder,
                at,
                target.size,
                &mut newer,
                None,
            );
            match decoded {
                Ok(Reached::Shown { picture, .. }) => {
                    out.frame = Some(picture);
                    break;
                }
                Ok(Reached::Dropped) => {
                    closing = true;
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
                    let next = if lower.is_empty() {
                        None
                    } else {
                        tracing::warn!(file = %file.path.display(), "decoding with {} failed, stepping down: {}", decoder.method, err.report());
                        self.open(key, file, lower).ok()
                    };
                    match next {
                        Some(next) => decoder = next,
                        None => {
                            out.error = Some(err);
                            break;
                        }
                    }
                }
            }
        }
        out.method = Some(decoder.method.clone());
        out.work = Some(decoder.last_work());
        self.decoders.push_front((key, decoder));
        self.decoders.truncate(OPEN_DECODERS);
        (!closing).then_some(out)
    }

    /// Fills in behind the playhead: decodes the stretch before the frames
    /// kept around it, from the key frame before them, keeping every frame.
    /// Stops between frames as soon as a request arrives. Returns whether
    /// anything was done (there may be more to do).
    fn fill(&mut self, waiting: &mut Waiting<'_>) -> bool {
        let Some(playhead) = self.playhead.clone() else {
            return false;
        };
        let key = playhead.key;
        if self.unfillable.contains(&key) || self.off_device.contains(&key) {
            return false;
        }
        let Some(kept_from) = self.frames.stretch_start(key, playhead.moment) else {
            return false; // the frame at the playhead isn't kept: nothing to add to
        };
        if self.stuck == Some((key, kept_from)) {
            return false;
        }
        // The filler is a decoder of its own: the one at the playhead keeps its place.
        // A second decoder on the device failing says nothing about the
        // first: the file is just not filled in.
        let Some(on_device) = &self.on_device else {
            return false;
        };
        let opened = match self.filler.take() {
            Some((open, decoder)) if open == key => Ok(decoder),
            _ => {
                on_device.watch.about_to_decode();
                let opened =
                    VideoDecoder::open_on_device(&playhead.path, key.1, playhead.rotation, &on_device.device);
                match &opened {
                    Ok(_) => on_device.watch.answered(),
                    Err(_) => on_device.watch.reported_an_error(),
                }
                opened
            }
        };
        let mut decoder = match opened {
            Ok(decoder) => decoder,
            Err(err) => {
                tracing::info!(file = %playhead.path.display(), "not filling in behind the playhead: {}", err.report());
                self.unfillable.insert(key);
                return false;
            }
        };
        let tick = decoder.tick();
        // Enough is kept behind the playhead already, or what would be
        // added is further from it than everything there is room for.
        let up_to =
            fill_up_to(playhead.moment, kept_from, tick).filter(|up_to| self.frames.would_keep(key, *up_to));
        let Some(up_to) = up_to else {
            self.filler = Some((key, decoder));
            return false;
        };
        let frames = &mut self.frames;
        let mut keep = |picture: Decoded, until: f64| {
            if let Decoded::Gpu(frame) = picture {
                let (w, h) = frame.planes.size();
                let bytes = u64::from(w) * u64::from(h) * 3 / 2;
                frames.put(key, frame.time, until, tick, bytes, frame);
            }
        };
        // Anything asked for comes first.
        let mut newer = || {
            waiting.take_in();
            if waiting.newest.is_some() || waiting.closed {
                Newer::Elsewhere
            } else {
                Newer::Nothing
            }
        };
        on_device.watch.about_to_decode();
        let (w, h) = playhead.size;
        let reached = decoder.picture_at_while(up_to, w, h, &mut newer, Some(&mut keep));
        match reached {
            Ok(reached) => {
                on_device.watch.answered();
                self.filler = Some((key, decoder));
                // Finished, and the kept stretch begins where it did: the
                // frames didn't join up (times that don't follow on). Not again.
                if matches!(reached, Reached::Shown { .. })
                    && self.frames.stretch_start(key, playhead.moment) == Some(kept_from)
                {
                    self.stuck = Some((key, kept_from));
                }
                // Given up for a request: more to do later. Finished: look again.
                true
            }
            Err(err) => {
                on_device.watch.reported_an_error();
                tracing::warn!(file = %playhead.path.display(), "filling in behind the playhead failed: {}", err.report());
                self.unfillable.insert(key);
                false
            }
        }
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
}

/// Decodes towards `at`, asking `newer` between frames whether that is
/// still what is wanted and handing the frames passed to `keep`
/// (`VideoDecoder::picture_at_while`).
fn decode(
    convert: Convert,
    on_device: Option<&OnDevice>,
    decoder: &mut VideoDecoder,
    at: f64,
    (w, h): (u32, u32),
    newer: &mut dyn FnMut() -> Newer,
    keep: Option<&mut dyn FnMut(Decoded, f64)>,
) -> diag::Result<Reached<Decoded>> {
    let rgba = |reached: Reached<media::Frame>| match reached {
        Reached::Shown { picture, at } => Reached::Shown {
            picture: Decoded::Rgba(picture),
            at,
        },
        Reached::Dropped => Reached::Dropped,
    };
    let Convert::WhileDrawing { largest_side } = convert else {
        return decoder.frame_at_while(at, w, h, newer).map(rgba);
    };
    let watch = match (on_device, decoder.level()) {
        (Some(on_device), Level::Device) => Some(&on_device.watch),
        _ => None,
    };
    if let Some(watch) = watch {
        watch.about_to_decode();
    }
    let reached = decoder.picture_at_while(at, w, h, newer, keep);
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

/// One file's picture at one moment, as a layer needs it.
struct Target<'a> {
    key: (MediaId, u32),
    path: &'a Path,
    rotation: u16,
    /// Seconds from the start of the file.
    moment: f64,
    /// The size the picture is asked for at.
    size: (u32, u32),
}

/// What a decode is to the request it is made for.
#[derive(Debug, Clone, Copy)]
struct Role {
    /// The top layer: the playhead is here, and what is filled in behind it.
    leads: bool,
    /// Reaching a moment a newer request asks for makes it that request's picture.
    follows: bool,
    /// A newer request that can't use the decode ends it.
    give_up: bool,
}

impl Role {
    /// The only picture of its request.
    const ALONE: Role = Role {
        leads: true,
        follows: true,
        give_up: true,
    };

    fn give_up(self, give_up: bool) -> Role {
        Role { give_up, ..self }
    }
}

impl Picture {
    /// Nothing to see at `at`, and nothing wrong.
    fn black(at: Time) -> Picture {
        Picture {
            at,
            frame: None,
            composed: None,
            refused: false,
            converted_on_cpu: false,
            error: None,
            took: Duration::ZERO,
            work: None,
            method: None,
        }
    }
}

/// The size the timeline's picture is put together at for a viewer of
/// `room` pixels: the timeline's shape, as large as fits the room, never
/// larger than the timeline's own size.
fn preview_size(timeline: (u32, u32), room: (u32, u32)) -> (u32, u32) {
    let fitted = Area::fitted(timeline, (room.0.max(1), room.1.max(1)));
    let (w, h) = fitted.size();
    let (w, h) = (
        (w.round() as u32).min(timeline.0),
        (h.round() as u32).min(timeline.1),
    );
    (w.max(2), h.max(2))
}

/// The size a layer's whole picture is needed at for the part shown to
/// have a pixel for every pixel of its area.
fn pixels_needed(layer: &frame::Layer) -> (u32, u32) {
    let (w, h) = layer.area.size();
    let whole = |shown: f32, part: f32| (shown / part.clamp(0.01, 1.0)).ceil().clamp(2.0, 16_384.0) as u32;
    (whole(w, layer.part.width), whole(h, layer.part.height))
}

/// Whether two descriptions show the same clips' files, layer for layer.
fn same_layers(a: &FrameDescription, b: &FrameDescription) -> bool {
    a.layers.len() == b.layers.len()
        && a.layers
            .iter()
            .zip(&b.layers)
            .all(|(x, y)| (x.media, x.stream) == (y.media, y.stream))
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

    /// The device a window would be drawn with, here without a window (a
    /// software one will do). `None` when this machine has none at all.
    fn composing() -> Option<Composing> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Composing {
            device,
            queue,
            wrap: None,
        })
    }

    /// Red, green, blue and alpha of one pixel of a composed frame.
    fn pixel(composing: &Composing, composed: &Composed, (x, y): (u32, u32)) -> [f32; 4] {
        let buffer = composing.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = composing
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: composed.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        composing.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        composing
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let bytes = buffer.get_mapped_range(..).unwrap();
        // Four 16-bit floats.
        let half = |i: usize| {
            let bits = u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
            let (exponent, fraction) = (i32::from((bits >> 10) & 0x1f), f32::from(bits & 0x3ff));
            let value = if exponent == 0 {
                fraction / 1024.0 * 2f32.powi(-14)
            } else {
                (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15)
            };
            if bits & 0x8000 != 0 { -value } else { value }
        };
        [half(0), half(1), half(2), half(3)]
    }

    #[test]
    fn the_timelines_whole_picture_is_put_together_on_the_device() {
        let Some(composing) = composing() else {
            eprintln!("no graphics device here: putting pictures together isn't checked");
            return;
        };
        // A wide recording on a tall timeline: bars above and below it.
        let project = Project::new("T", project::SequenceSettings::VERTICAL_SHORT);
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        for convert in [PLANES, Convert::OnCpu] {
            let preview =
                Preview::start_composing(Decoding::Software, convert, None, Some(composing.clone()), || {});
            preview.show(&project, seq, Time::seconds(1), 400, 480);
            let p = wait(&preview);
            assert!(
                p.frame.is_none() && p.error.is_none() && !p.refused,
                "{convert:?}"
            );
            let composed = p.composed.expect("a picture");
            // The timeline's shape, as large as fits the room.
            assert_eq!(composed.size(), (270, 480), "{convert:?}");
            assert!(!composed.on_device);
            assert_eq!(
                pixel(&composing, &composed, (135, 20)),
                [0.0, 0.0, 0.0, 1.0],
                "above the picture"
            );
            assert_eq!(
                pixel(&composing, &composed, (135, 460)),
                [0.0, 0.0, 0.0, 1.0],
                "below it"
            );
            let middle = pixel(&composing, &composed, (135, 240));
            assert!(
                middle[..3].iter().any(|&light| light > 0.01),
                "{convert:?}: {middle:?}"
            );
            assert!(p.method.is_some() && p.work.is_some());

            preview.show(&project, seq, Time::seconds(30), 400, 480);
            let gap = wait(&preview);
            assert!(
                gap.composed.is_none() && gap.error.is_none(),
                "after the last clip is black"
            );
        }
    }

    #[test]
    fn a_picture_that_shows_through_bars_has_the_one_below_under_it() {
        let Some(composing) = composing() else {
            return;
        };
        // On a tall timeline the clip on top leaves bars; what is below it
        // is decoded too, at the same moment, and both are in the picture.
        let project = Project::new("T", project::SequenceSettings::VERTICAL_SHORT);
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let mut project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        project = Command::AddTrack {
            sequence: seq,
            kind: project::TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&project)
        .unwrap();
        let sequence = project.active().unwrap();
        let (track, mut clip) = (
            sequence.video_tracks[1].id,
            sequence.video_tracks[0].clips[0].clone(),
        );
        clip.id = project.allocate_id();
        clip.link = None;
        let project = Command::PlaceClip {
            sequence: seq,
            track,
            clip,
        }
        .apply(&project)
        .unwrap();
        let described = frame::describe(&project, project.active().unwrap(), Time::seconds(1));
        assert_eq!(
            described.layers.len(),
            2,
            "the top clip has bars, so the lower one is seen"
        );

        let preview =
            Preview::start_composing(Decoding::Software, PLANES, None, Some(composing.clone()), || {});
        preview.show(&project, seq, Time::seconds(1), 270, 480);
        let p = wait(&preview);
        assert!(p.error.is_none());
        let composed = p.composed.expect("a picture");
        let middle = pixel(&composing, &composed, (135, 240));
        assert!(middle[..3].iter().any(|&light| light > 0.01), "{middle:?}");
        assert_eq!(pixel(&composing, &composed, (135, 20))[3], 1.0);
    }

    #[test]
    fn the_preview_has_the_timelines_shape_and_never_more_than_its_size() {
        assert_eq!(preview_size((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(preview_size((1080, 1920), (400, 480)), (270, 480));
        assert_eq!(
            preview_size((1920, 1080), (4000, 4000)),
            (1920, 1080),
            "never enlarged"
        );
        assert_eq!(
            preview_size((1920, 1080), (0, 0)),
            (2, 2),
            "a viewer with no room yet"
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
            frames_kept: 64 << 20,
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
            composing: None,
            composer: None,
            frames: FrameCache::new(0),
            playhead: None,
            filler: None,
            unfillable: HashSet::new(),
            stuck: None,
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
            composing: None,
            composer: None,
            frames: FrameCache::new(0),
            playhead: None,
            filler: None,
            unfillable: HashSet::new(),
            stuck: None,
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
    fn waiting_for_a_request_ends_when_nothing_comes() {
        let (_, p, seq) = worker_and_project();
        let (requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        let patience = Duration::from_millis(20);
        assert!(matches!(waiting.next_within(patience), Next::Idle));
        requests.send(request(&p, seq, 100)).unwrap();
        requests.send(request(&p, seq, 200)).unwrap();
        let Next::Request(newest) = waiting.next_within(patience) else {
            panic!("a request was waiting");
        };
        assert_eq!(Some(newest.at), Time::from_ratio(200, 1000));
        assert!(matches!(waiting.next_within(patience), Next::Idle));
        drop(requests);
        assert!(matches!(waiting.next_within(patience), Next::Closed));
    }

    #[test]
    fn the_stretch_behind_the_playhead_is_filled_in_when_little_is_kept() {
        let tick = 0.0005;
        // The playhead is 0.3 s into the frames kept, which begin at 12 s:
        // decode up to the frame just before them.
        let up_to = fill_up_to(12.3, 12.0, tick).unwrap();
        assert!(up_to < 12.0 - tick && up_to > 12.0 - 0.004, "{up_to}");
        // Two seconds or more are kept behind it: enough.
        assert_eq!(fill_up_to(14.0, 12.0, tick), None);
        // The frames kept begin at the start of the file: nothing before them.
        assert_eq!(fill_up_to(0.5, 0.0, tick), None);
    }

    #[test]
    fn with_nothing_kept_there_is_nothing_to_fill_in() {
        let (mut worker, p, seq) = worker_and_project();
        let (_requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        assert!(!worker.fill(&mut waiting), "no playhead yet");
        // A picture decoded on the CPU is not kept, so nothing is filled in behind it.
        worker.picture(&request(&p, seq, 1000), &mut waiting).unwrap();
        assert!(worker.playhead.is_some());
        assert!(!worker.fill(&mut waiting));
        // In a gap there is no file to fill in.
        worker.picture(&request(&p, seq, 30_000), &mut waiting).unwrap();
        assert!(worker.playhead.is_none());
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

    /// A frame kept on the graphics card is shown without a decoder being
    /// opened. Runs where there is a Vulkan driver (a software one will do).
    #[test]
    fn a_frame_kept_on_the_card_is_shown_without_decoding() {
        let Some(device) = video_device::testing::device() else {
            return;
        };
        let (mut worker, p, seq) = worker_and_project();
        let (_requests, inbox) = channel::<Request>();
        let mut waiting = Waiting::new(&inbox);
        worker.frames = FrameCache::new(64 << 20);
        let (key, moment) = source_of(&request(&p, seq, 500)).unwrap();
        let planes = Arc::new(video_device::PlaneImages::new(&device, (64, 36)).unwrap());
        let frame = GpuFrame {
            planes,
            colour: media::Colour {
                matrix: media::Matrix::Bt709,
                range: media::Range::Limited,
            },
            rotation: 0,
            time: moment,
        };
        worker
            .frames
            .put(key, moment, moment + 1.0 / 60.0, 0.0005, 64 * 36 * 3 / 2, frame);
        let picture = worker.picture(&request(&p, seq, 500), &mut waiting).unwrap();
        assert!(matches!(picture.frame, Some(Decoded::Gpu(_))));
        assert_eq!(picture.method.as_deref(), Some(KEPT));
        assert!(
            picture.work.is_none() && worker.decoders.is_empty(),
            "no decoder was needed"
        );
        // A moment that isn't kept is decoded as always.
        let picture = worker.picture(&request(&p, seq, 900), &mut waiting).unwrap();
        assert!(matches!(picture.frame, Some(Decoded::Rgba(_))));
        assert_eq!(worker.decoders.len(), 1);
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
