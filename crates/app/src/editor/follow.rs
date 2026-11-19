//! "Follow it from here" on a blur or pixelate box: the box's Left and Top
//! keyframes from the playhead to the clip's end, found on a thread of its
//! own (`engine::track`) while the page goes on. The status line says how
//! far it is; the command bound to Escape stops it, keeping what it found.
//! What it finds is one undo step on the effect. What it shares with
//! Stabilise is in `clip_job.rs`.

use super::Editor;
use super::clip_job::{self, Doing, Target};
use engine::track::{Follow, Followed, Following};
use project::{ClipId, EffectId, Time};

/// Following a box, in the words its messages use.
const FOLLOWING: Doing = Doing {
    running: "Following the box…",
    verb: "follow",
    during: "the box was being followed",
};

/// A follow under way.
#[derive(Debug)]
pub struct FollowJob {
    target: Target,
    following: Following,
}

impl FollowJob {
    /// Stops following; what it found so far comes as its news.
    pub(super) fn stop(&self) {
        self.following.stop();
    }
}

impl Editor {
    /// Starts following the box of `effect` on `clip` from the playhead.
    pub(super) fn start_follow(&mut self, clip: ClipId, effect: EffectId) -> diag::Result<()> {
        let Some(document) = self.document.as_ref() else {
            return Ok(());
        };
        let found = clip_job::find(document.project(), clip, effect, FOLLOWING)?;
        let from = found
            .clip
            .source_at(self.playhead_on_frame().max(found.clip.start));
        let numbers = found.effect.kind_at(from).numbers();
        let Some(range) = found.effect.kind.spot_numbers() else {
            return Ok(());
        };
        let area = [0, 1, 2, 3].map(|i| clip_job::share(numbers[range.start + i]));
        let following = Following::start(Follow {
            path: found.media.path.clone(),
            stream: found.clip.stream,
            rotation: found.video.rotation,
            rate: found.video.frame_rate,
            from,
            to: found.last_frame(),
            area,
        })?;
        self.following = Some(FollowJob {
            target: found.target,
            following,
        });
        self.status = clip_job::started(FOLLOWING, &self.shortcuts);
        Ok(())
    }

    /// Takes in what a follow under way says; once it is done, sets the
    /// box's keyframes (one undo step). The command bound to Escape stops
    /// it (`Editor::stop_tasks`).
    pub(super) fn follow_news(&mut self, ctx: &egui::Context) {
        let Some(job) = self.following.as_ref() else {
            return;
        };
        ctx.request_repaint_after(clip_job::NEWS_EVERY);
        match job.following.news() {
            None => {}
            Some(Followed::Going(done)) => {
                self.status = clip_job::going(FOLLOWING, done, &self.shortcuts);
            }
            Some(Followed::Failed(why)) => {
                self.following = None;
                self.errors
                    .push(&diag::AppError::new("The box couldn't be followed").why(why));
            }
            Some(Followed::Done { places, lost_at }) => {
                let Some(job) = self.following.take() else {
                    return;
                };
                let places = clip_job::places_in_millionths(&places);
                if let Err(err) = self.finish_follow(&job, &places) {
                    self.errors.push(&err);
                    return;
                }
                self.status = match lost_at {
                    Some(_) if places.len() <= 1 => {
                        "The box lost what it covers at once: try a box around something with more detail"
                            .into()
                    }
                    Some(_) => {
                        "The box followed until it lost what it covers: its keyframes end there".into()
                    }
                    None => "The box follows what it covers: Left and Top keyframes set".into(),
                };
            }
        }
    }

    /// Sets the found places as the effect's Left and Top keyframes.
    fn finish_follow(&mut self, job: &FollowJob, places: &[(Time, (i32, i32))]) -> diag::Result<()> {
        if places.len() < 2 {
            return Ok(());
        }
        let target = job.target;
        self.with_document(|d| clip_job::write_back(d, target, FOLLOWING, |e| e.following(places)))
    }
}
