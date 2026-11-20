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
//! A request whose picture can't be decoded quickly (the file's decoder
//! would have to jump, and the frame isn't kept) gets a first picture at
//! once, before the exact one ([`Worker::stand_in`]): the frame at that
//! moment from the file's proxy when it has one (`cache::proxy`), else the
//! key frame before it (up to one key frame distance early), unless that
//! was switched off ([`Preview::set_key_frame_first`]): then a file
//! without a proxy shows nothing first and the exact frame when it comes.
//! The exact
//! frame follows and replaces it; a newer request may give that decode
//! up, since it gets a first picture of its own at once. Not while
//! playing: playing decodes on frame by frame. With proxies On, a file
//! with a proxy is shown from it alone and the original is not decoded.
//! Export never reads proxies: its workers are never handed the shelf.
//!
//! Each file is decoded the fastest way that works for it, and steps down
//! when a way fails, at opening or at any picture ([`ladder`]): on the
//! window's own graphics device when the editor hands one over
//! ([`OnDevice`]), then on the GPU's video decoder with a copy back, then on
//! the CPU. Stepping down is logged with the reason and shows no error; only
//! the last way's failure does.

mod ahead;
mod composed;
mod decode;
mod fill;
mod kept;
mod ladder;
mod proxies;
mod stand_in;
mod waiting;

pub use ladder::{below, ladder};
#[cfg(test)]
pub(crate) use stand_in::PROXY_ONLY;

use crate::compose::{Composed, Composer, Composing};
use crate::timeline::video_at;
use cache::proxy::Shelf;
use fill::{IDLE_AFTER, Playhead};
use media::{Decoded, Decoding, Level, VideoDecoder, WindowDevice, Work};
use project::{Fingerprint, MediaId, Project, SequenceId, Time};
use proxies::Proxies;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use waiting::{Next, Waiting};

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
    /// How many bytes of decoded frames may be kept on the device
    /// ([`crate::frames::budget_on_card`], or the setting), followed while
    /// the worker runs.
    pub frames_kept: crate::frames::Budget,
    /// How many bytes of frames that made room on the card may be kept in
    /// the computer's memory (the memory for frames; 0: none).
    pub frames_in_memory: crate::frames::Budget,
}

impl std::fmt::Debug for OnDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OnDevice({:?})", self.device)
    }
}

/// A picture for one moment of the timeline.
#[derive(Debug)]
pub struct Picture {
    /// The timeline time asked for.
    pub at: Time,
    /// The top clip's picture, when the worker doesn't put pictures
    /// together itself. `None` in a gap (black), or when the frame couldn't
    /// be decoded (see `error`). An RGBA picture comes opaque (every
    /// alpha 255), so the window can show its bytes as they are.
    pub frame: Option<Decoded>,
    /// The timeline's whole picture, put together on the window's device,
    /// when the worker was started with one ([`Preview::start_composing`]).
    /// `None` in a gap (black) or when nothing could be decoded.
    pub composed: Option<Composed>,
    /// A small copy of `composed` as display values, for the scopes, when
    /// they were asked for ([`Preview::set_scope`]).
    pub sampled: Option<Arc<crate::scope::Sampled>>,
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
    /// A first picture, shown until the exact one for the same request
    /// (or a newer one) comes: a proxy frame or a key frame.
    pub stand_in: bool,
    /// How far past it the frames kept on the card go on without a gap
    /// (seconds of the file): what was read ahead. `None` when the frames
    /// of its file aren't kept (decoded on the processor, proxies).
    pub kept_ahead: Option<f64>,
}

struct Request {
    project: Project,
    sequence: SequenceId,
    at: Time,
    max_width: u32,
    max_height: u32,
    /// Where proxies are kept, when they may be used.
    proxies: Option<Arc<Shelf>>,
    /// The timeline is playing: frames are decoded on, never stood in for.
    playing: bool,
    /// A file without a proxy may show the key frame before the moment first.
    key_frame_first: bool,
    /// A small copy of the picture is read back for the scopes.
    scope: bool,
}

pub struct Preview {
    /// `None` once closed: nothing more is asked for.
    requests: Option<Sender<Request>>,
    /// Handed along with every request: proxies may be shown.
    proxies: Option<Arc<Shelf>>,
    playing: bool,
    /// Handed along with every request: see [`Preview::set_key_frame_first`].
    key_frame_first: bool,
    /// Handed along with every request: see [`Preview::set_scope`].
    scope: bool,
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
            frames: kept::Kept::new(
                on_device.as_ref().map_or(0, |d| d.frames_kept.bytes()),
                on_device.as_ref().map_or(0, |d| d.frames_in_memory.bytes()),
                on_device.as_ref().map(|d| Arc::clone(&d.device)),
            ),
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
            playing: false,
            rested: false,
            key_frame_first: true,
            proxies: Proxies::default(),
            glancer: None,
            last_glance: None,
            titles: VecDeque::new(),
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
            proxies: None,
            playing: false,
            key_frame_first: true,
            scope: false,
            pictures,
            ended,
        }
    }

    /// Lets the worker show frames from the proxies on `shelf` (or not,
    /// with `None`), as the shelf's mode says. The viewer's workers are
    /// handed it; an export's never are.
    pub fn set_proxies(&mut self, shelf: Option<Arc<Shelf>>) {
        self.proxies = shelf;
    }

    /// Whether a jump in a file without a proxy shows the key frame before
    /// the moment at once, then the exact frame (on, the usual), or waits
    /// for the exact frame (off). Proxy frames shown first are not affected.
    pub fn set_key_frame_first(&mut self, on: bool) {
        self.key_frame_first = on;
    }

    /// Whether pictures come with a small copy read back for the scopes
    /// (`Picture::sampled`): only while a scope is shown.
    pub fn set_scope(&mut self, on: bool) {
        self.scope = on;
    }

    /// The timeline is playing (or not): while it plays, pictures are
    /// decoded on frame by frame and nothing stands in for them.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
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
            proxies: self.proxies.clone(),
            playing: self.playing,
            key_frame_first: self.key_frame_first,
            scope: self.scope,
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
        // Playing, or rested: read ahead of the playhead until something is asked for.
        if (worker.playing || worker.rested) && worker.read_ahead(&mut waiting) {
            continue;
        }
        let request = match waiting.next_within(IDLE_AFTER) {
            Next::Request(request) => *request,
            Next::Closed => return,
            // Nothing asked for a moment: fill in behind the playhead, a
            // stretch at a time, until there is nothing to fill or
            // something is asked for. (And let go of proxies that went.)
            Next::Idle if worker.idle(&mut waiting) => continue,
            Next::Idle => match worker.rest(&mut waiting) {
                Some(request) => request,
                None => return,
            },
        };
        let mut show = |mut picture: Picture| {
            picture.make_opaque();
            if outbox.send(picture).is_err() {
                return false;
            }
            notify();
            true
        };
        // A decode the next request can't use is given up: nothing to send.
        let Some(picture) = worker.picture(&request, &mut waiting, &mut show) else {
            continue;
        };
        if !show(picture) {
            return;
        }
    }
}

/// The waiting request has its answer already when the picture made for
/// `r` is the one it asks for: the same moment (a decode can stop at a
/// moment asked for meanwhile, which `out` then names) of the same project
/// at the same size. A request for the same moment of a changed project
/// (a grade or a look being dragged) waits for a picture of its own: taken
/// for answered, it was never shown, and the viewer, having asked, didn't
/// ask again.
fn drop_answered(r: &Request, out: &Picture, waiting: &mut Waiting<'_>) {
    let answered = waiting.newest.as_ref().is_some_and(|next| {
        next.at == out.at
            && next.sequence == r.sequence
            && (next.max_width, next.max_height) == (r.max_width, r.max_height)
            && next.scope == r.scope
            && next.project == r.project
    });
    if answered {
        waiting.newest = None;
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
    /// Frames decoded on the window's device, kept there and in memory.
    frames: kept::Kept,
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
    /// The newest request was made while playing.
    playing: bool,
    /// Nothing was asked for a while since the newest request: reading
    /// ahead goes on until something is.
    rested: bool,
    /// The newest request lets a key frame be shown first.
    key_frame_first: bool,
    /// Proxies: where they are and the decoders reading them.
    proxies: Proxies,
    /// Titles drawn last, newest first, with the size they were drawn at:
    /// a title that stays the same isn't drawn again every frame.
    titles: VecDeque<(crate::title::TitleLayer, (u32, u32), media::Frame)>,
    /// A decoder of its own for a key frame shown first, and its file: the
    /// one at the playhead keeps its place.
    glancer: Option<((MediaId, u32), VideoDecoder)>,
    /// The key frame last shown first (file, and the key frame's time):
    /// not shown again while it is what the screen shows anyway.
    last_glance: Option<((MediaId, u32), f64)>,
}

/// What the viewer's corner says for a picture that was kept.
const KEPT: &str = "kept on the graphics card";

impl Worker {
    /// The picture for `r`, or for a request that arrived while it was
    /// decoded and was reached first (the picture's `at` says which).
    /// `None` when the decode was given up for a newer request it can't serve.
    ///
    /// A first picture is handed to `show` before the exact one is decoded,
    /// when that would take a while ([`Worker::stand_in`]).
    fn picture(
        &mut self,
        r: &Request,
        waiting: &mut Waiting<'_>,
        show: &mut dyn FnMut(Picture) -> bool,
    ) -> Option<Picture> {
        let started = Instant::now();
        // Whatever is answered without decoding counts as a finished answer.
        let gave_up_last = std::mem::take(&mut self.gave_up_last);
        self.playhead = None;
        self.playing = r.playing;
        self.rested = false;
        self.follow_budget();
        self.key_frame_first = r.key_frame_first;
        self.proxies.take(r.proxies.as_ref());
        let Some(sequence) = r.project.sequences.iter().find(|s| s.id == r.sequence) else {
            return Some(Picture::black(r.at));
        };
        // A decode that follows one that was given up is finished, unless
        // a proxy frame answered this request first: then the next request
        // gets one at once as well, and the exact decode may be given up.
        let mut give_up = !gave_up_last;
        if let Some((first, from_proxy)) = self.stand_in(r, sequence, started) {
            if !show(first) {
                return None;
            }
            give_up |= from_proxy;
        }
        if self.composing.is_some() {
            let out = self.composed(r, sequence, waiting, give_up, started)?;
            drop_answered(r, &out, waiting);
            return Some(self.with_kept_ahead(out));
        }
        let Some(showing) = video_at(&r.project, sequence, r.at) else {
            return Some(Picture::black(r.at)); // a gap: black
        };
        let target = Target {
            key: (showing.media.id, showing.clip.stream),
            path: &showing.media.path,
            fingerprint: &showing.media.fingerprint,
            rotation: showing.media.info.video.as_ref().map_or(0, |v| v.rotation),
            moment: showing.source_time.as_secs_f64(),
            size: (r.max_width, r.max_height),
        };
        let mut out = self.decoded(r.at, &target, waiting, Role::ALONE.give_up(give_up))?;
        out.took = started.elapsed();
        drop_answered(r, &out, waiting);
        Some(self.with_kept_ahead(out))
    }

    /// The picture, saying how far the frames kept go on past the playhead.
    fn with_kept_ahead(&self, mut out: Picture) -> Picture {
        out.kept_ahead = self.playhead.as_ref().and_then(|playhead| {
            let end = self.frames.stretch_end(playhead.key, playhead.moment)?;
            Some((end - playhead.moment).clamp(0.0, ahead::MOST_AHEAD))
        });
        out
    }

    /// Takes the budget for frames kept as it is set now.
    fn follow_budget(&mut self) {
        if let Some(on_device) = &self.on_device {
            self.frames
                .set_budgets(on_device.frames_kept.bytes(), on_device.frames_in_memory.bytes());
        }
    }
}

/// One file's picture at one moment, as a layer needs it.
#[derive(Clone, Copy)]
struct Target<'a> {
    key: (MediaId, u32),
    path: &'a Path,
    /// The file's fingerprint, which its proxy is found by.
    fingerprint: &'a Fingerprint,
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
    /// Sets every alpha of an RGBA picture to 255, here on the worker, so
    /// the window has no pass over its pixels to make (the viewer shows a
    /// picture without its transparency).
    pub(super) fn make_opaque(&mut self) {
        if let Some(Decoded::Rgba(frame)) = &mut self.frame {
            for alpha in frame.rgba.iter_mut().skip(3).step_by(4) {
                *alpha = 255;
            }
        }
    }

    /// Nothing to see at `at`, and nothing wrong.
    fn black(at: Time) -> Picture {
        Picture {
            at,
            frame: None,
            composed: None,
            sampled: None,
            refused: false,
            converted_on_cpu: false,
            error: None,
            took: Duration::ZERO,
            work: None,
            method: None,
            stand_in: false,
            kept_ahead: None,
        }
    }
}

/// The file a request shows.
struct File<'a> {
    path: &'a Path,
    stream: u32,
    rotation: u16,
}

#[cfg(test)]
mod tests;
