//! How the timeline of a project was laid out when it was last open: each
//! track's height and the divider between video and audio.
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectView {
    /// Tracks with a height of their own: the track's id and how many times
    /// the usual height it is.
    pub track_heights: Vec<(u64, f32)>,
    /// The share of the timeline's body given to video (the divider's place).
    pub video_share: Option<f32>,
}

impl ProjectView {
    /// The part of `timeline`'s view that is kept.
    pub fn of(timeline: &TimelineView) -> ProjectView {
        ProjectView {
            track_heights: timeline.heights.to_save(),
            video_share: Some(timeline.split),
        }
    }

    /// Lays `timeline` out this way.
    pub fn apply(&self, timeline: &mut TimelineView) {
        timeline.heights = ui_kit::timeline::TrackHeights::from_saved(self.track_heights.iter().copied());
        if let Some(share) = self.video_share.filter(|s| s.is_finite()) {
            timeline.split = share.clamp(0.1, 0.9);
        }
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
        std::fs::write(path, text).or_explain(explain)
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
    /// What the file holds, or will once `writing` finishes.
    kept: ProjectView,
    /// The view's height revision and divider when last looked at.
    seen: Option<(u64, f32)>,
    changed_at: Option<Instant>,
    writing: Option<JobHandle<()>>,
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
            seen: Some((timeline.heights.revision(), timeline.split)),
            changed_at: None,
            writing: None,
        }
    }

    /// Call every frame: hands the view to a worker when it changed and has
    /// since settled.
    pub fn tick(&mut self, now: Instant, timeline: &TimelineView, jobs: &JobSystem) {
        let looks = (timeline.heights.revision(), timeline.split);
        if self.seen != Some(looks) {
            self.seen = Some(looks);
            self.changed_at = Some(now);
        }
        if let Some(handle) = &self.writing
            && let Some(result) = handle.try_result()
        {
            if let Err(err) = result {
                tracing::warn!("{}", err.report());
            }
            self.writing = None;
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
        self.kept = view.clone();
        let path = self.path.clone();
        self.writing = Some(jobs.submit(
            PoolKind::Export,
            Priority::Background,
            "timeline layout",
            move |_| view.save(&path),
        ));
    }

    /// On closing: writes the view now if it differs from what is kept.
    pub fn close(&mut self, timeline: &TimelineView) {
        if let Some(handle) = self.writing.take() {
            let _ = handle.wait();
        }
        let view = ProjectView::of(timeline);
        if view != self.kept && self.writable {
            if let Err(err) = view.save(&self.path) {
                tracing::warn!("{}", err.report());
            }
            self.kept = view;
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
        let handle = keeper.writing.take().expect("written once it settled");
        handle.wait().unwrap();
        assert_eq!(ProjectView::load(&path), ProjectView::of(&timeline));

        // Nothing new: nothing written again.
        keeper.tick(start + Duration::from_secs(5), &timeline, &jobs);
        assert!(keeper.writing.is_none());
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
}
