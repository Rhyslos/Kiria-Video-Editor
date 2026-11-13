//! How the timeline of a project was laid out when it was last open: each
//! track's height, the divider between video and audio, which audio
//! tracks show their level lines, and which of its timelines was open.
//!
//! This is view state, not an edit. It lives in its own small file beside
//! the project (`<file>.view`), never in the project file, so making a track
//! taller is not an undo step, does not make the project "unsaved", and
//! needs no change to the project format. A missing or damaged file just
//! means the usual layout.

use diag::{AppError, Explain};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ui_kit::TimelineView;

/// The view is written once it has stayed the same for this long, so a drag
/// writes once at its end instead of on every frame.
const SETTLED_AFTER: Duration = Duration::from_millis(800);

/// What is kept between sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectView {
    /// Tracks with a height of their own: the track's id and how many times
    /// the usual height it is.
    pub track_heights: Vec<(u64, f32)>,
    /// The share of the timeline's body given to video (the divider's place).
    pub video_share: Option<f32>,
    /// The video area grows past the divider's place to show all its
    /// tracks. Off once the divider was dragged to where they don't all
    /// show. On when the file doesn't say (it was written before this).
    pub video_fits_tracks: bool,
    /// Audio tracks whose clips show their level line (none when the file
    /// doesn't say).
    pub level_lines: Vec<u64>,
    /// The timeline that was open, by id (the project's own when the file
    /// doesn't say, or that timeline is gone). Which timeline is open is
    /// not an edit, so it is kept here and not in the project.
    pub open_timeline: Option<u64>,
}

impl Default for ProjectView {
    fn default() -> Self {
        ProjectView {
            track_heights: Vec::new(),
            video_share: None,
            video_fits_tracks: true,
            level_lines: Vec::new(),
            open_timeline: None,
        }
    }
}

impl ProjectView {
    /// The part of `timeline`'s view that is kept.
    pub fn of(timeline: &TimelineView) -> ProjectView {
        ProjectView {
            track_heights: timeline.heights.to_save(),
            video_share: Some(timeline.split),
            video_fits_tracks: timeline.fit_video,
            level_lines: timeline.level_lines(),
            open_timeline: timeline.showing,
        }
    }

    /// Lays `timeline` out this way.
    pub fn apply(&self, timeline: &mut TimelineView) {
        timeline.heights = ui_kit::timeline::TrackHeights::from_saved(self.track_heights.iter().copied());
        if let Some(share) = self.video_share.filter(|s| s.is_finite()) {
            timeline.split = share.clamp(0.1, 0.9);
        }
        timeline.fit_video = self.video_fits_tracks;
        timeline.set_level_lines(self.level_lines.iter().copied());
        timeline.showing = self.open_timeline;
    }

    /// Reads the view kept for a project; the usual layout when there is
    /// none or it can't be read.
    pub fn load(path: &Path) -> ProjectView {
        let Ok(text) = std::fs::read_to_string(path) else {
            return ProjectView::default();
        };
        ron::from_str(&text).unwrap_or_else(|err| {
            tracing::info!(path = %path.display(), "the kept timeline layout can't be read, using the usual one: {err}");
            ProjectView::default()
        })
    }

    pub fn save(&self, path: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("The timeline's layout could not be kept")
                .why(format!("Writing {} failed.", path.display()))
                .fix("Nothing is lost but track heights: check that the project's folder can be written to and that the disk is not full.")
        };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(explain)?;
        project::disk::write_atomic(path, text.as_bytes()).or_explain(explain)
    }
}

/// What of the view is kept, cheaply compared every frame: the heights'
/// revision, the divider, whether the video area fits its tracks, the
/// tracks with level lines, and the timeline shown.
#[derive(Debug, Clone, PartialEq)]
struct Looks(u64, f32, bool, Vec<u64>, Option<u64>);

impl Looks {
    fn of(timeline: &TimelineView) -> Looks {
        Looks(
            timeline.heights.revision(),
            timeline.split,
            timeline.fit_video,
            timeline.level_lines(),
            timeline.showing,
        )
    }
}

/// Keeps a project's view file up to date: notices when the view changes and
/// writes it on a worker a moment after it stops changing.
#[derive(Debug)]
pub struct ViewKeeper {
    /// The view file: `project::view_path` of the project.
    path: PathBuf,
    /// False for a project open read-only: nothing is written beside it.
    writable: bool,
    /// What the file holds: set only once a write has succeeded, so a
    /// failed one is tried again (and on closing).
    kept: ProjectView,
    /// What of the view is kept, when last looked at.
    seen: Option<Looks>,
    changed_at: Option<Instant>,
    /// The write under way, with what it writes.
    writing: Option<(ProjectView, JobHandle<()>)>,
}

impl ViewKeeper {
    /// Reads the view kept at `path` and lays `timeline` out that way.
    /// Changes are written back there when `writable`.
    pub fn open(path: PathBuf, writable: bool, timeline: &mut TimelineView) -> ViewKeeper {
        ProjectView::load(&path).apply(timeline);
        ViewKeeper {
            path,
            writable,
            kept: ProjectView::of(timeline),
            seen: Some(Looks::of(timeline)),
            changed_at: None,
            writing: None,
        }
    }

    /// Call every frame: hands the view to a worker when it changed and has
    /// since settled.
    pub fn tick(&mut self, now: Instant, timeline: &TimelineView, jobs: &JobSystem) {
        let looks = Looks::of(timeline);
        if self.seen.as_ref() != Some(&looks) {
            self.seen = Some(looks);
            self.changed_at = Some(now);
        }
        if let Some((_, handle)) = &self.writing
            && let Some(result) = handle.try_result()
            && let Some((view, _)) = self.writing.take()
        {
            match result {
                Ok(()) => self.kept = view,
                Err(err) => {
                    tracing::warn!("{}", err.report());
                    // Tried again once it has settled anew.
                    self.changed_at = Some(now);
                }
            }
        }
        let settled = self
            .changed_at
            .is_some_and(|at| now.duration_since(at) >= SETTLED_AFTER);
        if !settled || self.writing.is_some() {
            return;
        }
        self.changed_at = None;
        let view = ProjectView::of(timeline);
        if view == self.kept || !self.writable {
            return;
        }
        let path = self.path.clone();
        let written = view.clone();
        let handle = jobs.submit(
            PoolKind::Export,
            Priority::Background,
            "timeline layout",
            move |_| written.save(&path),
        );
        self.writing = Some((view, handle));
    }

    /// On closing: writes the view now if it differs from what is kept.
    pub fn close(&mut self, timeline: &TimelineView) {
        if let Some((view, handle)) = self.writing.take()
            && handle.wait().is_ok()
        {
            self.kept = view;
        }
        let view = ProjectView::of(timeline);
        if view != self.kept && self.writable {
            match view.save(&self.path) {
                Ok(()) => self.kept = view,
                Err(err) => tracing::warn!("{}", err.report()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resized() -> TimelineView {
        let mut timeline = TimelineView::default();
        timeline.heights.set(7, 58.0, 174.0);
        timeline.heights.set(2, 54.0, 81.0);
        timeline.split = 0.3;
        timeline.fit_video = false;
        timeline.set_level_line(12, true);
        timeline.showing = Some(42);
        timeline
    }

    #[test]
    fn the_layout_comes_back_after_closing_and_reopening() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trip.project.view");
        ProjectView::of(&resized()).save(&path).unwrap();

        let mut reopened = TimelineView::default();
        let keeper = ViewKeeper::open(path, true, &mut reopened);
        assert_eq!(reopened.heights.height(7, 58.0), 174.0);
        assert_eq!(reopened.heights.height(2, 54.0), 81.0);
        assert_eq!(
            reopened.heights.height(1, 54.0),
            54.0,
            "untouched tracks keep the usual height"
        );
        assert_eq!(reopened.split, 0.3);
        assert!(!reopened.fit_video, "the divider was put where it is on purpose");
        assert!(
            reopened.level_line(12) && !reopened.level_line(7),
            "level lines where they were"
        );
        assert_eq!(reopened.showing, Some(42), "the timeline that was open");
        assert_eq!(keeper.kept, ProjectView::of(&reopened));
    }

    #[test]
    fn no_file_or_a_damaged_one_means_the_usual_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new.project.view");
        assert_eq!(ProjectView::load(&path), ProjectView::default());
        std::fs::write(&path, "not a layout").unwrap();
        assert_eq!(ProjectView::load(&path), ProjectView::default());
        // A file from a version that kept less still reads.
        std::fs::write(&path, "(track_heights: [(4, 2.0)])").unwrap();
        let mut timeline = TimelineView::default();
        ProjectView::load(&path).apply(&mut timeline);
        assert_eq!(timeline.heights.height(4, 50.0), 100.0);
        assert_eq!(timeline.split, TimelineView::default().split);
        assert!(timeline.fit_video, "and the video area shows its tracks");
        assert!(timeline.level_lines().is_empty(), "and no level lines");
        // So does one from before the video area grew by itself, also when
        // the timeline had been laid out otherwise meanwhile.
        timeline.fit_video = false;
        std::fs::write(&path, "(track_heights: [], video_share: Some(0.4))").unwrap();
        ProjectView::load(&path).apply(&mut timeline);
        assert!(timeline.fit_video && timeline.split == 0.4);
        // Nonsense for the divider is put back in range.
        std::fs::write(&path, "(video_share: Some(7.5))").unwrap();
        ProjectView::load(&path).apply(&mut timeline);
        assert_eq!(timeline.split, 0.9);
    }

    #[test]
    fn a_change_is_written_once_it_has_settled_and_not_before() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trip.project.view");
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut timeline = TimelineView::default();
        let mut keeper = ViewKeeper::open(path.clone(), true, &mut timeline);
        let start = Instant::now();
        keeper.tick(start, &timeline, &jobs);
        assert!(keeper.writing.is_none() && !path.exists(), "nothing changed");

        // A drag: the height changes on every frame for half a second.
        for frame in 0..30 {
            timeline.heights.set(7, 58.0, 60.0 + frame as f32);
            keeper.tick(start + Duration::from_millis(16 * frame), &timeline, &jobs);
        }
        assert!(keeper.writing.is_none(), "not while it is still changing");
        keeper.tick(start + Duration::from_millis(900), &timeline, &jobs);
        assert!(keeper.writing.is_none(), "nor right after");
        keeper.tick(start + Duration::from_millis(1400), &timeline, &jobs);
        let (_, handle) = keeper.writing.take().expect("written once it settled");
        handle.wait().unwrap();
        assert_eq!(ProjectView::load(&path), ProjectView::of(&timeline));

        // Nothing new: nothing written again.
        keeper.tick(start + Duration::from_secs(5), &timeline, &jobs);
        assert!(keeper.writing.is_none());

        // Turning a track's level lines on is kept too.
        timeline.set_level_line(9, true);
        keeper.tick(start + Duration::from_secs(6), &timeline, &jobs);
        keeper.tick(start + Duration::from_secs(7), &timeline, &jobs);
        keeper
            .writing
            .take()
            .expect("written once it settled")
            .1
            .wait()
            .unwrap();
        assert_eq!(ProjectView::load(&path).level_lines, [9]);
    }

    #[test]
    fn closing_writes_what_has_not_been_written_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trip.project.view");
        let mut timeline = TimelineView::default();
        let mut keeper = ViewKeeper::open(path.clone(), true, &mut timeline);
        keeper.close(&timeline);
        assert!(!path.exists(), "an untouched layout leaves no file");
        timeline.heights.set(3, 54.0, 100.0);
        keeper.close(&timeline);
        assert_eq!(ProjectView::load(&path), ProjectView::of(&timeline));
    }

    #[test]
    fn a_project_open_read_only_gets_nothing_written_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("newer.project.view");
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut timeline = TimelineView::default();
        let mut keeper = ViewKeeper::open(path.clone(), false, &mut timeline);
        timeline.heights.set(3, 54.0, 100.0);
        let start = Instant::now();
        keeper.tick(start, &timeline, &jobs);
        keeper.tick(start + Duration::from_secs(2), &timeline, &jobs);
        assert!(keeper.writing.is_none());
        keeper.close(&timeline);
        assert!(!path.exists());
    }

    #[test]
    fn a_failed_write_is_not_taken_as_kept_and_is_tried_again() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trip.project.view");
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut timeline = TimelineView::default();
        let mut keeper = ViewKeeper::open(path.clone(), true, &mut timeline);
        // A folder where the file goes: the write fails.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("x"), "").unwrap();
        timeline.heights.set(3, 54.0, 100.0);
        let start = Instant::now();
        keeper.tick(start, &timeline, &jobs);
        keeper.tick(start + Duration::from_secs(1), &timeline, &jobs);
        assert!(keeper.writing.is_some(), "handed over");
        // The same moment, over and over, until the failure is in.
        while keeper.writing.is_some() {
            std::thread::sleep(Duration::from_millis(2));
            keeper.tick(start + Duration::from_secs(2), &timeline, &jobs);
        }
        assert_ne!(keeper.kept, ProjectView::of(&timeline), "not kept: it failed");

        // The folder goes away: the next settled tick writes it.
        std::fs::remove_dir_all(&path).unwrap();
        keeper.tick(start + Duration::from_secs(4), &timeline, &jobs);
        let (_, handle) = keeper.writing.take().expect("tried again");
        handle.wait().unwrap();
        assert_eq!(ProjectView::load(&path), ProjectView::of(&timeline));
    }

    #[test]
    fn closing_after_a_failed_write_writes_again() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trip.project.view");
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut timeline = TimelineView::default();
        let mut keeper = ViewKeeper::open(path.clone(), true, &mut timeline);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("x"), "").unwrap();
        timeline.heights.set(3, 54.0, 100.0);
        let start = Instant::now();
        keeper.tick(start, &timeline, &jobs);
        keeper.tick(start + Duration::from_secs(1), &timeline, &jobs);
        while keeper.writing.is_some() {
            std::thread::sleep(Duration::from_millis(2));
            keeper.tick(start + Duration::from_secs(1), &timeline, &jobs);
        }
        std::fs::remove_dir_all(&path).unwrap();
        keeper.close(&timeline);
        assert_eq!(ProjectView::load(&path), ProjectView::of(&timeline));
    }
}
