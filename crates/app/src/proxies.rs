//! Proxies for the open project's video files (`cache::proxy`): looked at,
//! judged, built and deleted on workers, newest file first, a few at once
//! on a processor with many cores ([`BuildPace`]: each long file also built in
//! stretches side by side, `media::proxy::write`); and what each file's
//! proxy is up to, for the media drawer.
//!
//! "Build proxies at full speed" ([`ProxyBuilds::set_full_speed`]) builds
//! more at once, in more stretches, and keeps building while the timeline
//! plays, until nothing is left to build.
//!
//! - Proxies Auto: a file gets a proxy when a jump in it can't make 50 ms
//!   (`cache::proxy::judge`, timed once per file per session).
//! - On: every video file gets one.
//! - Off: none is built (and none is shown); one asked for by hand still is.
//!
//! Builds are background work on the analysis pool: they wait while the
//! timeline plays, also in the middle of a file. "Build proxy now" puts a
//! file first in line (an automatic build under way gives way to it);
//! "Delete proxy" removes it, and like a proxy cleared in Settings › Cache
//! it is never built again by itself (the shelf remembers it), only by hand. After every build the proxies used longest ago, other than the
//! open project's, are dropped to stay inside the disk budget.

use crate::reading::{self, Pace};
use cache::proxy::{Judgement, ProxyMode, Shelf};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use media::proxy::Steps;
use project::{Fingerprint, MediaId, MediaKind, MediaRef, Project};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui_kit::timeline::LoadState;

/// How often the drawer is redrawn while a proxy builds.
const REDRAW_EVERY: Duration = Duration::from_millis(250);
const GB: u64 = 1024 * 1024 * 1024;

/// What a file's proxy is up to, as the media drawer shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Not a video file, or not looked at yet.
    Nothing,
    /// Jumps in it are quick enough (Auto): no proxy needed.
    NotNeeded,
    /// One will be built (or would be, with proxies Off).
    Needed,
    /// Being built: how far, and the time left once it can be told.
    Building(LoadState),
    Ready,
    /// Being deleted.
    Deleting,
    /// Cleared from the cache on purpose: not built again by itself.
    Cleared,
    /// It could not be built: why.
    Failed(String),
}

/// What is known of one file.
#[derive(Debug)]
enum State {
    /// Looking whether its proxy exists and, under Auto, whether it needs one.
    Looking(JobHandle<Look>),
    /// No proxy; not judged yet.
    Unjudged,
    Judged(Judgement),
    /// Being built (the build under way, or about to be); with its
    /// judgement, which stands if the build gives way to another.
    Building(Option<Judgement>),
    Ready,
    Deleting(JobHandle<bool>),
    /// Cleared from the cache or deleted on purpose (Settings › Cache,
    /// "Delete proxy"): not built
    /// again by itself, in this session or any later one, until asked for
    /// by hand (`cache::proxy::Shelf::is_cleared`).
    Cleared,
    Failed(String),
}

/// What looking at a file found.
#[derive(Debug, Clone, Copy)]
struct Look {
    ready: bool,
    /// Its proxy was cleared on purpose.
    cleared: bool,
    judgement: Option<Judgement>,
}

#[derive(Debug)]
struct File {
    path: PathBuf,
    fingerprint: Fingerprint,
    stream: u32,
    length: f64,
    state: State,
}

/// How many proxies are built at once, and in how many stretches each
/// (`media::proxy::write`'s `workers`), for a processor with `cores`
/// threads. Graphics cards' encoders allow only a few sessions at once,
/// so the product stays at eight or under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildPace {
    pub at_once: usize,
    pub workers: usize,
}

impl BuildPace {
    /// In the background: leaves most of the machine to the editor.
    pub fn background(cores: usize) -> BuildPace {
        BuildPace {
            at_once: if cores >= 12 { 2 } else { 1 },
            workers: (cores / 8).clamp(1, 3),
        }
    }

    /// At full speed: as much of the machine as building can use.
    pub fn full(cores: usize) -> BuildPace {
        BuildPace {
            at_once: (cores / 8).clamp(1, 2),
            workers: (cores / 4).clamp(1, 4),
        }
    }
}

/// A build under way.
struct Build {
    media: MediaId,
    handle: JobHandle<media::proxy::Written>,
    /// Asked for by hand: other builds don't take its place.
    asked: bool,
    pace: Pace,
    state: LoadState,
}

pub struct ProxyBuilds {
    shelf: Arc<Shelf>,
    files: HashMap<MediaId, File>,
    builds: Vec<Build>,
    /// Building at full speed until nothing is left to build.
    full_speed: bool,
    cores: usize,
    /// Files asked for by hand, first in line.
    asked: Vec<MediaId>,
    /// The disk budget for proxies, in bytes.
    budget: u64,
    trim: Option<JobHandle<usize>>,
    /// Trim once after start, before anything is built.
    trimmed_once: bool,
    /// `--hold-proxy`: builds stop at this share, for screenshots.
    hold_at: Option<f32>,
    /// Counts proxies built and deleted: Settings › Cache measures again
    /// when it changes while it is open.
    built: u64,
}

impl ProxyBuilds {
    /// Proxies in `folder`, `mode` as set, within `disk_gb` of disk.
    pub fn new(folder: PathBuf, mode: ProxyMode, disk_gb: u64, hold_at: Option<f32>) -> ProxyBuilds {
        ProxyBuilds {
            shelf: Shelf::new(folder, mode),
            files: HashMap::new(),
            builds: Vec::new(),
            full_speed: false,
            cores: std::thread::available_parallelism().map_or(2, |n| n.get()),
            asked: Vec::new(),
            budget: disk_gb.max(1).saturating_mul(GB),
            trim: None,
            trimmed_once: false,
            hold_at,
            built: 0,
        }
    }

    /// How many proxies were built or deleted this session: what the cache
    /// holds changed when it changes.
    pub fn changes(&self) -> u64 {
        self.built
    }

    /// The shelf, for the viewer's workers.
    pub fn shelf(&self) -> Arc<Shelf> {
        Arc::clone(&self.shelf)
    }

    pub fn set_mode(&mut self, mode: ProxyMode) {
        self.shelf.set_mode(mode);
    }

    /// What `media`'s proxy is up to.
    pub fn status(&self, media: MediaId) -> Status {
        let Some(file) = self.files.get(&media) else {
            return Status::Nothing;
        };
        match &file.state {
            State::Looking(_) => Status::Nothing,
            State::Unjudged => match self.shelf.mode() {
                ProxyMode::On => Status::Needed,
                _ => Status::Nothing,
            },
            State::Judged(judged) if judged.needs_proxy || self.shelf.mode() == ProxyMode::On => {
                Status::Needed
            }
            State::Judged(_) => Status::NotNeeded,
            State::Building(_) => match self.builds.iter().find(|b| b.media == media) {
                Some(build) => Status::Building(build.state),
                None => Status::Needed,
            },
            State::Ready => Status::Ready,
            State::Deleting(_) => Status::Deleting,
            State::Cleared => Status::Cleared,
            State::Failed(why) => Status::Failed(why.clone()),
        }
    }

    /// Builds `media`'s proxy next, whatever the mode and the judgement.
    pub fn build_now(&mut self, media: MediaId) {
        if let Some(file) = self.files.get_mut(&media)
            && matches!(file.state, State::Cleared | State::Failed(_))
        {
            file.state = State::Unjudged;
        }
        self.asked.retain(|m| *m != media);
        self.asked.insert(0, media);
        // An automatic build gives way when no room is left for it.
        if self.builds.len() >= self.pace().at_once
            && !self.builds.iter().any(|b| b.media == media)
            && let Some(build) = self.builds.iter().rev().find(|b| !b.asked)
        {
            build.handle.cancel();
        }
    }

    /// Builds a proxy for every video file of the project that has none
    /// (also those cleared on purpose, and those that failed), by hand, one
    /// at a time, whatever the mode. Returns how many were asked for.
    pub fn build_all(&mut self) -> usize {
        let mut wanted: Vec<MediaId> = self
            .files
            .iter()
            .filter(|(_, f)| wants_proxy(&f.state))
            .map(|(m, _)| *m)
            .collect();
        wanted.sort();
        for media in &wanted {
            if let Some(file) = self.files.get_mut(media)
                && matches!(file.state, State::Cleared | State::Failed(_))
            {
                file.state = State::Unjudged;
            }
            if !self.asked.contains(media) {
                self.asked.push(*media);
            }
        }
        wanted.len()
    }

    /// Proxies were switched to On: every video file is to have one, so
    /// those cleared on purpose are built again (Auto never rebuilds them:
    /// a cleared proxy is built again only when asked for, and switching
    /// to On is asking). Returns how many come back.
    pub fn switched_on(&mut self) -> usize {
        let mut back = 0;
        for file in self.files.values_mut() {
            if matches!(file.state, State::Cleared) {
                file.state = State::Unjudged;
                back += 1;
            }
        }
        back
    }

    /// Deletes `media`'s proxy on a worker.
    pub fn delete(&mut self, jobs: &JobSystem, media: MediaId) {
        let Some(file) = self.files.get_mut(&media) else {
            return;
        };
        for build in self.builds.iter().filter(|b| b.media == media) {
            build.handle.cancel();
        }
        self.asked.retain(|m| *m != media);
        let (shelf, fingerprint, stream) = (Arc::clone(&self.shelf), file.fingerprint.clone(), file.stream);
        let handle = jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "delete proxy",
            move |_| shelf.delete(&fingerprint, stream),
        );
        file.state = State::Deleting(handle);
    }

    /// For screenshots: with `--hold-proxy`, whether a build has reached
    /// the share it is held at (always true without it).
    pub fn settled(&self) -> bool {
        let Some(hold_at) = self.hold_at else {
            return true;
        };
        self.builds.iter().any(|build| match build.state {
            LoadState::Reading { done, .. } => done >= hold_at,
            _ => false,
        })
    }

    /// Stops the builds under way (the app is closing).
    pub fn stop(&self) {
        for build in &self.builds {
            build.handle.cancel();
        }
    }

    /// Builds at full speed (more at once, in more stretches, also while
    /// the timeline plays) until nothing is left to build, or back at the
    /// usual pace. Builds already under way finish at the pace they began.
    pub fn set_full_speed(&mut self, on: bool) {
        self.full_speed = on;
    }

    /// How many at once, in how many stretches, now.
    fn pace(&self) -> BuildPace {
        if self.full_speed {
            BuildPace::full(self.cores)
        } else {
            BuildPace::background(self.cores)
        }
    }

    /// Keeps up with the project's files, collects finished work and starts
    /// what is due. Returns an error to show (a proxy that could not be
    /// deleted); a build that fails says so in the drawer instead.
    pub fn poll(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        project: Option<&Project>,
        now: Instant,
    ) -> Option<diag::AppError> {
        let project = project?;
        let mode = self.shelf.mode();
        self.notice_files(jobs, ctx, project, mode);
        let problem = self.collect();
        self.finish_build(jobs, project, now);
        if !self.trimmed_once && self.trim.is_none() {
            self.trimmed_once = true;
            self.start_trim(jobs, project);
        }
        if let Some(handle) = &self.trim
            && handle.try_result().is_some()
        {
            self.trim = None;
        }
        self.start_build(jobs, ctx, mode);
        if self.builds.is_empty() && self.asked.is_empty() {
            // Nothing left: full speed was for this round of building.
            self.full_speed = false;
        } else {
            ctx.request_repaint_after(REDRAW_EVERY);
        }
        problem
    }

    /// Starts looking at files the project has that are new here, and
    /// forgets files it no longer has.
    fn notice_files(&mut self, jobs: &JobSystem, ctx: &egui::Context, project: &Project, mode: ProxyMode) {
        self.files.retain(|id, _| project.media.contains_key(id));
        self.asked.retain(|id| project.media.contains_key(id));
        for media in project.media.values() {
            if self.files.get(&media.id).is_some_and(|f| f.path == media.path) {
                continue;
            }
            let Some(stream) = video_stream(media) else {
                continue;
            };
            let file = File {
                path: media.path.clone(),
                fingerprint: media.fingerprint.clone(),
                stream,
                length: media.duration.as_secs_f64(),
                state: State::Unjudged,
            };
            let handle = self.look(jobs, ctx, &file, mode == ProxyMode::Auto);
            self.files.insert(
                media.id,
                File {
                    state: State::Looking(handle),
                    ..file
                },
            );
        }
        // Auto asks for a judgement of files not judged yet.
        if mode == ProxyMode::Auto {
            let unjudged: Vec<MediaId> = self
                .files
                .iter()
                .filter(|(_, f)| matches!(f.state, State::Unjudged))
                .map(|(id, _)| *id)
                .collect();
            for id in unjudged {
                if let Some(file) = self.files.get(&id) {
                    let handle = self.look(jobs, ctx, file, true);
                    if let Some(file) = self.files.get_mut(&id) {
                        file.state = State::Looking(handle);
                    }
                }
            }
        }
    }

    /// Looks on a worker whether `file`'s proxy exists, and with `judge`
    /// whether it needs one.
    fn look(&self, jobs: &JobSystem, ctx: &egui::Context, file: &File, judge: bool) -> JobHandle<Look> {
        let (shelf, ctx) = (Arc::clone(&self.shelf), ctx.clone());
        let (path, fingerprint, stream, length) = (
            file.path.clone(),
            file.fingerprint.clone(),
            file.stream,
            file.length,
        );
        jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "proxy needed?",
            move |_| {
                let ready = shelf.ready(&fingerprint, stream).is_some();
                let cleared = !ready && shelf.is_cleared(&fingerprint, stream);
                let judgement = if judge && !ready && !cleared {
                    Some(cache::proxy::judge(&path, stream, length, media::Decoding::Best)?)
                } else {
                    None
                };
                if let Some(judged) = &judgement {
                    tracing::info!(
                        file = %path.display(),
                        "a jump takes about {} ms (key frames up to {:.1} s apart): {}",
                        judged.typical_jump.as_millis(),
                        judged.longest_gap,
                        if judged.needs_proxy { "a proxy is built" } else { "no proxy needed" }
                    );
                }
                ctx.request_repaint();
                Ok(Look {
                    ready,
                    cleared,
                    judgement,
                })
            },
        )
    }

    /// Takes in finished looks and deletions.
    fn collect(&mut self) -> Option<diag::AppError> {
        let mut problem = None;
        for file in self.files.values_mut() {
            match &file.state {
                State::Looking(handle) => match handle.try_result() {
                    Some(Ok(look)) => {
                        file.state = if look.ready {
                            State::Ready
                        } else if look.cleared {
                            State::Cleared
                        } else {
                            look.judgement.map_or(State::Unjudged, State::Judged)
                        };
                    }
                    Some(Err(err)) => {
                        tracing::info!(file = %file.path.display(), "no proxy: {}", err.report());
                        file.state = State::Failed(err.what().to_owned());
                    }
                    None => {}
                },
                State::Deleting(handle) => match handle.try_result() {
                    Some(Ok(_)) => {
                        file.state = State::Cleared;
                        self.built += 1;
                    }
                    Some(Err(err)) => {
                        file.state = State::Cleared;
                        problem = Some(err);
                    }
                    None => {}
                },
                _ => {}
            }
        }
        problem
    }

    /// Takes in the builds that have finished, and reads how far the
    /// others are.
    fn finish_build(&mut self, jobs: &JobSystem, project: &Project, now: Instant) {
        let mut finished = Vec::new();
        self.builds.retain_mut(|build| match build.handle.try_result() {
            Some(result) => {
                finished.push((build.media, result));
                false
            }
            None => {
                build.state = build.pace.of_job(build.handle.progress(), now);
                true
            }
        });
        for (media, result) in finished {
            self.finished(jobs, project, media, result);
        }
    }

    /// Takes in one finished build.
    fn finished(
        &mut self,
        jobs: &JobSystem,
        project: &Project,
        media: MediaId,
        result: diag::Result<media::proxy::Written>,
    ) {
        let Some(file) = self.files.get_mut(&media) else {
            return;
        };
        match result {
            Ok(written) => {
                tracing::info!(file = %file.path.display(), encoder = %written.encoder, frames = written.frames, "proxy built");
                file.state = State::Ready;
                self.built += 1;
                self.asked.retain(|m| *m != media);
                self.start_trim(jobs, project);
            }
            // Stopped: given way to another, deleted, or the file went.
            Err(_) if build_was_stopped(&file.state) => {}
            Err(err) if err.what() == media::proxy::stopped().what() => {
                file.state = match file.state {
                    State::Building(Some(judged)) => State::Judged(judged),
                    _ => State::Unjudged,
                };
            }
            Err(err) => {
                tracing::warn!(file = %file.path.display(), "the proxy could not be built: {}", err.report());
                self.asked.retain(|m| *m != media);
                file.state = State::Failed(err.what().to_owned());
            }
        }
    }

    /// Where the proxies of `project`'s video files are (or would be).
    pub fn project_proxies(&self, project: &Project) -> Vec<PathBuf> {
        crate::cache_cleanup::proxies_of(&self.shelf, project)
    }

    /// The cache is about to be cleared (Settings › Cache): the build
    /// under way stops, and so do builds asked for by hand.
    pub fn clearing(&mut self) {
        for build in &self.builds {
            build.handle.cancel();
        }
        self.asked.clear();
    }

    /// The proxies at `paths` were cleared: their files say so, and are
    /// not built again by themselves (the shelf remembers it for later
    /// sessions) until "Build proxy now".
    pub fn cleared(&mut self, paths: &[PathBuf]) {
        for file in self.files.values_mut() {
            let proxy = self.shelf.file_for(&file.fingerprint, file.stream);
            if paths.contains(&proxy) && !matches!(file.state, State::Looking(_) | State::Deleting(_)) {
                file.state = State::Cleared;
            }
        }
    }

    /// Drops the proxies used longest ago, other than the project's, to
    /// stay inside the budget.
    fn start_trim(&mut self, jobs: &JobSystem, project: &Project) {
        if self.trim.is_some() {
            return;
        }
        let keep = self.project_proxies(project);
        let (shelf, budget) = (Arc::clone(&self.shelf), self.budget);
        self.trim = Some(jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "trim proxies",
            move |_| Ok(shelf.trim(budget, &keep)),
        ));
    }

    /// Starts builds while there is room for them ([`BuildPace::at_once`]): a
    /// file asked for by hand first, then (unless proxies are Off) the
    /// newest file that needs one.
    fn start_build(&mut self, jobs: &JobSystem, ctx: &egui::Context, mode: ProxyMode) {
        // Proxies turned Off: automatic builds stop.
        if mode == ProxyMode::Off {
            for build in self.builds.iter().filter(|b| !b.asked) {
                build.handle.cancel();
            }
        }
        let pace = self.pace();
        while self.builds.len() < pace.at_once {
            let building: Vec<MediaId> = self.builds.iter().map(|b| b.media).collect();
            let asked = self.asked.iter().copied().find(|id| {
                !building.contains(id)
                    && self
                        .files
                        .get(id)
                        .is_some_and(|f| buildable(&f.state, true, mode))
            });
            let next = asked.or_else(|| {
                self.files
                    .iter()
                    .filter(|(id, f)| !building.contains(id) && buildable(&f.state, false, mode))
                    .map(|(id, _)| *id)
                    // Newest files first: ids grow as files are imported.
                    .max_by_key(|id| id.0)
            });
            let Some(media) = next else {
                return;
            };
            self.start_one(jobs, ctx, media, asked.is_some(), pace);
        }
    }

    /// Starts building `media`'s proxy.
    fn start_one(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: MediaId,
        asked: bool,
        pace: BuildPace,
    ) {
        let Some(file) = self.files.get_mut(&media) else {
            return;
        };
        let (shelf, ctx, hold_at) = (Arc::clone(&self.shelf), ctx.clone(), self.hold_at);
        let (path, fingerprint, stream, length) = (
            file.path.clone(),
            file.fingerprint.clone(),
            file.stream,
            file.length,
        );
        // At full speed it is not background work: it goes on while the
        // timeline plays.
        let priority = if self.full_speed {
            Priority::Interactive
        } else {
            Priority::Background
        };
        let handle = jobs.submit(PoolKind::Analysis, priority, "proxy", move |job| {
            let cancelled = || job.is_cancelled();
            let progress = |done: f32| {
                job.set_progress(done);
                reading::hold(hold_at, done, job);
            };
            let pause = || job.wait_while_held_back();
            let steps = Steps {
                cancelled: &cancelled,
                progress: &progress,
                pause: &pause,
            };
            // Decoding the original is most of the work: on the GPU's decoder when it can.
            let built = shelf.build(
                &path,
                stream,
                length,
                &fingerprint,
                media::Decoding::Best,
                pace.workers,
                &steps,
            );
            ctx.request_repaint();
            built
        });
        file.state = State::Building(match file.state {
            State::Judged(judged) => Some(judged),
            _ => None,
        });
        self.builds.push(Build {
            media,
            handle,
            asked,
            pace: Pace::default(),
            state: LoadState::Waiting,
        });
    }
}

/// Whether a build that ended was stopped on purpose (the file's state
/// moved on meanwhile).
fn build_was_stopped(state: &State) -> bool {
    matches!(state, State::Deleting(_) | State::Cleared)
}

/// Whether "Build proxies" asks for a file in `state`: it has no proxy and
/// none is being made or taken away.
fn wants_proxy(state: &State) -> bool {
    matches!(
        state,
        State::Unjudged | State::Judged(_) | State::Cleared | State::Failed(_)
    )
}

/// Whether a file in `state` is built next: by hand (`asked`) whatever
/// the mode, else as the mode says.
fn buildable(state: &State, asked: bool, mode: ProxyMode) -> bool {
    match state {
        State::Unjudged | State::Judged(_) if asked => true,
        State::Unjudged => mode == ProxyMode::On,
        State::Judged(judged) => mode == ProxyMode::On || (mode == ProxyMode::Auto && judged.needs_proxy),
        _ => false,
    }
}

/// The picture stream a proxy would be made of: video files only.
pub(crate) fn video_stream(media: &MediaRef) -> Option<u32> {
    (media.kind == MediaKind::Video)
        .then_some(media.info.video.as_ref())
        .flatten()
        .map(|video| video.index)
}

/// The drawer's short word for a status, if it has one.
pub fn label(status: &Status) -> Option<String> {
    match status {
        Status::Nothing | Status::NotNeeded => None,
        Status::Needed => Some("Needs proxy".into()),
        Status::Building(LoadState::Waiting) => Some("Proxy waiting".into()),
        Status::Building(LoadState::Reading { done, seconds_left }) => Some(match seconds_left {
            Some(left) => format!(
                "Proxy {} · {}",
                ui_kit::timeline::percent(*done),
                ui_kit::time_text::time_left(*left)
            ),
            None => format!("Proxy {}", ui_kit::timeline::percent(*done)),
        }),
        Status::Building(LoadState::Failed) | Status::Failed(_) => Some("Proxy failed".into()),
        Status::Ready => Some("Proxy".into()),
        Status::Deleting => Some("Deleting proxy".into()),
        Status::Cleared => Some("Proxy cleared · build from menu".into()),
    }
}

/// The colour a proxy's state is written in, the same in the media drawer
/// and on the Library page.
pub fn colour(status: &Status, theme: &ui_kit::Theme) -> egui::Color32 {
    match status {
        Status::Failed(_) | Status::Building(LoadState::Failed) => ui_kit::status::ERROR,
        Status::Needed => ui_kit::status::WARNING,
        Status::Ready => theme.accent,
        _ => theme.text_secondary,
    }
}

/// A proxy's state as a small badge on a Library card or row: its words
/// (`label`, shortened where long), fill and ink. `None` when there is
/// nothing to say (not a video, or no proxy needed).
pub fn badge(status: &Status, theme: &ui_kit::Theme) -> Option<(String, egui::Color32, egui::Color32)> {
    let text = match status {
        Status::Cleared => "Proxy cleared".to_owned(),
        _ => label(status)?,
    };
    let fill = colour(status, theme);
    let ink = if *status == Status::Ready {
        theme.accent_ink
    } else {
        theme.bg
    };
    Some((text, fill, ink))
}

/// What a status means, for the tooltip.
pub fn explanation(status: &Status, mode: ProxyMode) -> Option<String> {
    let text = match status {
        Status::Nothing => return None,
        Status::NotNeeded => "Jumps in this file are quick enough without a proxy. Right-click to build one anyway.".into(),
        Status::Needed if mode == ProxyMode::Off => {
            "Jumps in this file are slow. Proxies are Off in Settings; right-click to build one anyway.".into()
        }
        Status::Needed => "A proxy is built for this file in the background, newest files first.".into(),
        Status::Building(LoadState::Waiting) => "The proxy is built once the timeline stops playing.".into(),
        Status::Building(_) => {
            "Building a small copy that is quick to jump around in. You can keep editing and playing meanwhile.".into()
        }
        Status::Ready => {
            "Jumps show the proxy's frame at once, sharpening when the playhead rests. Export always uses the original.".into()
        }
        Status::Deleting => "The proxy is being deleted.".into(),
        Status::Cleared => {
            "Its proxy was cleared from the cache, so none is built by itself any more. Right-click and \"Build proxy now\" to make one again.".into()
        }
        Status::Failed(why) => format!("The proxy could not be built: {why}"),
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn judged(needs_proxy: bool) -> State {
        State::Judged(Judgement {
            longest_gap: 2.0,
            typical_jump: Duration::from_millis(if needs_proxy { 90 } else { 20 }),
            needs_proxy,
        })
    }

    #[test]
    fn more_is_built_at_once_on_more_cores_and_at_full_speed() {
        assert_eq!(
            BuildPace::background(2),
            BuildPace {
                at_once: 1,
                workers: 1
            }
        );
        assert_eq!(
            BuildPace::background(24),
            BuildPace {
                at_once: 2,
                workers: 3
            }
        );
        assert_eq!(
            BuildPace::full(24),
            BuildPace {
                at_once: 2,
                workers: 4
            }
        );
        for cores in [1, 2, 4, 8, 16, 24, 64, 128] {
            for pace in [BuildPace::background(cores), BuildPace::full(cores)] {
                assert!(pace.at_once >= 1 && pace.workers >= 1);
                assert!(
                    pace.at_once * pace.workers <= 8,
                    "graphics cards allow few encoders"
                );
            }
        }
    }

    #[test]
    fn what_is_built_follows_the_mode() {
        use ProxyMode::*;
        assert!(buildable(&judged(true), false, Auto));
        assert!(!buildable(&judged(false), false, Auto), "quick enough");
        assert!(buildable(&judged(false), false, On), "On: every file");
        assert!(buildable(&State::Unjudged, false, On));
        assert!(!buildable(&State::Unjudged, false, Auto), "judged first");
        assert!(!buildable(&judged(true), false, Off));
        assert!(
            buildable(&judged(false), true, Off),
            "asked for by hand: whatever the mode"
        );
        assert!(!buildable(&State::Ready, true, On));
        assert!(
            !buildable(&State::Cleared, false, On) && !buildable(&State::Cleared, false, Auto),
            "cleared on purpose: never again by itself"
        );
    }

    #[test]
    fn a_cleared_proxy_is_built_again_only_when_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let mut builds = ProxyBuilds::new(dir.path().to_path_buf(), ProxyMode::On, 1, None);
        let file = |size: u64| File {
            path: PathBuf::from(format!("{size}.mp4")),
            fingerprint: Fingerprint {
                size,
                sample_hash: format!("{size:x}"),
            },
            stream: 0,
            length: 10.0,
            state: State::Ready,
        };
        let (ours, other) = (MediaId(1), MediaId(2));
        builds.files.insert(ours, file(1));
        builds.files.insert(other, file(2));
        let cleared = builds.shelf.file_for(
            &Fingerprint {
                size: 1,
                sample_hash: "1".into(),
            },
            0,
        );
        builds.cleared(&[cleared]);
        assert_eq!(builds.status(ours), Status::Cleared);
        assert_eq!(builds.status(other), Status::Ready, "only what was cleared");
        assert!(!buildable(&builds.files[&ours].state, false, ProxyMode::On));
        builds.build_now(ours);
        assert!(
            buildable(&builds.files[&ours].state, true, ProxyMode::Off),
            "by hand it is"
        );
    }

    #[test]
    fn the_drawer_says_what_a_proxy_is_up_to() {
        assert_eq!(label(&Status::Needed).as_deref(), Some("Needs proxy"));
        assert_eq!(label(&Status::Ready).as_deref(), Some("Proxy"));
        let building = Status::Building(LoadState::Reading {
            done: 0.43,
            seconds_left: None,
        });
        assert_eq!(label(&building).as_deref(), Some("Proxy 43 %"));
        assert_eq!(label(&Status::NotNeeded), None);
        assert_eq!(
            label(&Status::Cleared).as_deref(),
            Some("Proxy cleared · build from menu")
        );
        assert!(explanation(&Status::Cleared, ProxyMode::On).is_some_and(|t| t.contains("Build proxy now")));
        assert!(explanation(&Status::Needed, ProxyMode::Off).is_some_and(|t| t.contains("Off")));
    }

    #[test]
    fn the_library_shows_the_drawer_s_words_as_badges() {
        let theme = ui_kit::ThemeName::Ash.theme();
        let text = |s: Status| badge(&s, &theme).map(|(t, _, _)| t);
        assert_eq!(text(Status::Ready).as_deref(), Some("Proxy"));
        assert_eq!(text(Status::Cleared).as_deref(), Some("Proxy cleared"));
        assert_eq!(text(Status::Needed).as_deref(), Some("Needs proxy"));
        assert_eq!(text(Status::NotNeeded), None);
        assert_eq!(colour(&Status::Failed("x".into()), &theme), ui_kit::status::ERROR);
    }

    #[test]
    fn video_files_only() {
        let mut media = MediaRef {
            id: MediaId(1),
            path: "/r/a.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: project::Time::seconds(1),
            info: project::MediaInfo::default(),
        };
        assert_eq!(video_stream(&media), None);
        media.kind = MediaKind::Video;
        assert_eq!(video_stream(&media), None, "no picture stream described");
    }

    #[test]
    fn build_all_and_switching_to_on_bring_cleared_proxies_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut builds = ProxyBuilds::new(dir.path().to_path_buf(), ProxyMode::On, 1, None);
        let file = |size: u64, state: State| File {
            path: PathBuf::from(format!("{size}.mp4")),
            fingerprint: Fingerprint {
                size,
                sample_hash: format!("{size:x}"),
            },
            stream: 0,
            length: 10.0,
            state,
        };
        builds.files.insert(MediaId(1), file(1, State::Cleared));
        builds.files.insert(MediaId(2), file(2, State::Ready));
        builds.files.insert(MediaId(3), file(3, State::Cleared));
        builds.files.insert(MediaId(4), file(4, judged(false)));
        assert_eq!(builds.switched_on(), 2, "the two cleared ones");
        assert_eq!(builds.status(MediaId(1)), Status::Needed, "On: built again");
        assert_eq!(builds.status(MediaId(2)), Status::Ready);
        builds.files.insert(MediaId(1), file(1, State::Cleared));
        builds
            .files
            .insert(MediaId(5), file(5, State::Failed("disk".into())));
        let asked = builds.build_all();
        assert_eq!(asked, 4, "every one without a proxy: 1, 3, 4 and 5");
        assert_eq!(builds.asked, [MediaId(1), MediaId(3), MediaId(4), MediaId(5)]);
        assert!(buildable(&builds.files[&MediaId(1)].state, true, ProxyMode::Off));
        assert_eq!(builds.build_all(), 4, "asking again adds nothing twice");
        assert_eq!(builds.asked.len(), 4);
    }
}
