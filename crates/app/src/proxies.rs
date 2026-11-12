//! Proxies for the open project's video files (`cache::proxy`): looked at,
//! judged, built and deleted on workers, one build at a time, newest file
//! first; and what each file's proxy is up to, for the media drawer.
//!
//! - Proxies Auto: a file gets a proxy when a jump in it can't make 50 ms
//!   (`cache::proxy::judge`, timed once per file per session).
//! - On: every video file gets one.
//! - Off: none is built (and none is shown); one asked for by hand still is.
//!
//! Builds are background work on the analysis pool: they wait while the
//! timeline plays, also in the middle of a file. "Build proxy now" puts a
//! file first in line (an automatic build under way gives way to it);
//! "Delete proxy" removes it, and it is not built again by itself this
//! session. After every build the proxies used longest ago, other than the
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
    /// Deleted by hand: not built again by itself this session.
    Declined,
    Failed(String),
}

/// What looking at a file found.
#[derive(Debug, Clone, Copy)]
struct Look {
    ready: bool,
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

/// The build under way.
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
    build: Option<Build>,
    /// Files asked for by hand, first in line.
    asked: Vec<MediaId>,
    /// The disk budget for proxies, in bytes.
    budget: u64,
    trim: Option<JobHandle<usize>>,
    /// Trim once after start, before anything is built.
    trimmed_once: bool,
    /// `--hold-proxy`: builds stop at this share, for screenshots.
    hold_at: Option<f32>,
}

impl ProxyBuilds {
    /// Proxies in `folder`, `mode` as set, within `disk_gb` of disk.
    pub fn new(folder: PathBuf, mode: ProxyMode, disk_gb: u64, hold_at: Option<f32>) -> ProxyBuilds {
        ProxyBuilds {
            shelf: Shelf::new(folder, mode),
            files: HashMap::new(),
            build: None,
            asked: Vec::new(),
            budget: disk_gb.max(1).saturating_mul(GB),
            trim: None,
            trimmed_once: false,
            hold_at,
        }
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
            State::Judged(_) | State::Declined => Status::NotNeeded,
            State::Building(_) => match &self.build {
                Some(build) if build.media == media => Status::Building(build.state),
                _ => Status::Needed,
            },
            State::Ready => Status::Ready,
            State::Deleting(_) => Status::Deleting,
            State::Failed(why) => Status::Failed(why.clone()),
        }
    }

    /// Builds `media`'s proxy next, whatever the mode and the judgement.
    pub fn build_now(&mut self, media: MediaId) {
        if let Some(file) = self.files.get_mut(&media)
            && matches!(file.state, State::Declined | State::Failed(_))
        {
            file.state = State::Unjudged;
        }
        self.asked.retain(|m| *m != media);
        self.asked.insert(0, media);
        // An automatic build gives way.
        if let Some(build) = &self.build
            && !build.asked
            && build.media != media
        {
            build.handle.cancel();
        }
    }

    /// Deletes `media`'s proxy on a worker.
    pub fn delete(&mut self, jobs: &JobSystem, media: MediaId) {
        let Some(file) = self.files.get_mut(&media) else {
            return;
        };
        if let Some(build) = &self.build
            && build.media == media
        {
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
        self.build.as_ref().is_some_and(|build| match build.state {
            LoadState::Reading { done, .. } => done >= hold_at,
            _ => false,
        })
    }

    /// Stops the build under way (the app is closing).
    pub fn stop(&self) {
        if let Some(build) = &self.build {
            build.handle.cancel();
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
        if self.build.is_some() {
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
                let judgement = if judge && !ready {
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
                Ok(Look { ready, judgement })
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
                        file.state = match (look.ready, look.judgement) {
                            (true, _) => State::Ready,
                            (false, Some(judged)) => State::Judged(judged),
                            (false, None) => State::Unjudged,
                        };
                    }
                    Some(Err(err)) => {
                        tracing::info!(file = %file.path.display(), "no proxy: {}", err.report());
                        file.state = State::Failed(err.what().to_owned());
                    }
                    None => {}
                },
                State::Deleting(handle) => match handle.try_result() {
                    Some(Ok(_)) => file.state = State::Declined,
                    Some(Err(err)) => {
                        file.state = State::Declined;
                        problem = Some(err);
                    }
                    None => {}
                },
                _ => {}
            }
        }
        problem
    }

    /// Takes in the build under way when it has finished, and reads how far
    /// it is otherwise.
    fn finish_build(&mut self, jobs: &JobSystem, project: &Project, now: Instant) {
        let Some(build) = &mut self.build else {
            return;
        };
        let Some(result) = build.handle.try_result() else {
            build.state = build.pace.of_job(build.handle.progress(), now);
            return;
        };
        let media = build.media;
        self.build = None;
        let Some(file) = self.files.get_mut(&media) else {
            return;
        };
        match result {
            Ok(written) => {
                tracing::info!(file = %file.path.display(), encoder = %written.encoder, frames = written.frames, "proxy built");
                file.state = State::Ready;
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
        project
            .media
            .values()
            .filter_map(|m| video_stream(m).map(|stream| self.shelf.file_for(&m.fingerprint, stream)))
            .collect()
    }

    /// The cache is being cleared (Settings › Cache): the build under way
    /// stops, and the project's files are not built again by themselves
    /// this session, as after "Delete proxy" (a build asked for by hand
    /// still is).
    pub fn cleared(&mut self) {
        if let Some(build) = &self.build {
            build.handle.cancel();
        }
        self.asked.clear();
        for file in self.files.values_mut() {
            if !matches!(file.state, State::Looking(_) | State::Deleting(_)) {
                file.state = State::Declined;
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

    /// Starts the next build when none is under way: a file asked for by
    /// hand first, then (unless proxies are Off) the newest file that
    /// needs one.
    fn start_build(&mut self, jobs: &JobSystem, ctx: &egui::Context, mode: ProxyMode) {
        if let Some(build) = &self.build {
            // Proxies turned Off: an automatic build stops.
            if mode == ProxyMode::Off && !build.asked {
                build.handle.cancel();
            }
            return;
        }
        let asked = self.asked.iter().copied().find(|id| {
            self.files
                .get(id)
                .is_some_and(|f| buildable(&f.state, true, mode))
        });
        let next = asked.or_else(|| {
            self.files
                .iter()
                .filter(|(_, f)| buildable(&f.state, false, mode))
                .map(|(id, _)| *id)
                // Newest files first: ids grow as files are imported.
                .max_by_key(|id| id.0)
        });
        let Some(media) = next else {
            return;
        };
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
        let handle = jobs.submit(PoolKind::Analysis, Priority::Background, "proxy", move |job| {
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
            let built = shelf.build(&path, stream, length, &fingerprint, media::Decoding::Best, &steps);
            ctx.request_repaint();
            built
        });
        file.state = State::Building(match file.state {
            State::Judged(judged) => Some(judged),
            _ => None,
        });
        self.build = Some(Build {
            media,
            handle,
            asked: asked.is_some(),
            pace: Pace::default(),
            state: LoadState::Waiting,
        });
    }
}

/// Whether a build that ended was stopped on purpose (the file's state
/// moved on meanwhile).
fn build_was_stopped(state: &State) -> bool {
    matches!(state, State::Deleting(_) | State::Declined)
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
fn video_stream(media: &MediaRef) -> Option<u32> {
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
    }
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
            !buildable(&State::Declined, false, On),
            "deleted by hand: not again by itself"
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
        assert!(explanation(&Status::Needed, ProxyMode::Off).is_some_and(|t| t.contains("Off")));
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
}
