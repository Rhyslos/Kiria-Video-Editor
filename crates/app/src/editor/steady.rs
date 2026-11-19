//! "Analyse the clip" on a Stabilise effect: how the camera moves over the
//! whole clip is found on a thread of its own (`engine::steady`) while the
//! page goes on. The status line says how far it is; the command bound to
//! Escape stops it, setting nothing. What it finds becomes the effect's
//! view keyframes and zoom, one undo step. What it shares with following a
//! box is in `clip_job.rs`.

use super::Editor;
use super::clip_job::{self, Doing, Target};
use engine::steady::{Analyse, Analysed, Analysing, Steadied};
use project::effects::Stabilise;
use project::{ClipId, EffectId, EffectKind};

/// Analysing a clip, in the words its messages use.
const ANALYSING: Doing = Doing {
    running: "Analysing the camera's movement…",
    verb: "steady",
    during: "the clip was being analysed",
};

/// An analysis under way.
#[derive(Debug)]
pub struct SteadyJob {
    target: Target,
    analysing: Analysing,
}

impl SteadyJob {
    /// Stops the analysis; nothing is set.
    pub(super) fn stop(&self) {
        self.analysing.stop();
    }
}

impl Editor {
    /// Starts analysing `clip` for its Stabilise `effect`.
    pub(super) fn start_steady(&mut self, clip: ClipId, effect: EffectId) -> diag::Result<()> {
        let Some(document) = self.document.as_ref() else {
            return Ok(());
        };
        let found = clip_job::find(document.project(), clip, effect, ANALYSING)?;
        let EffectKind::Stabilise(settings) = found.effect.kind else {
            return Err(clip_job::missing("effect", ANALYSING));
        };
        let analysing = Analysing::start(Analyse {
            path: found.media.path.clone(),
            stream: found.clip.stream,
            rotation: found.video.rotation,
            rate: found.video.frame_rate,
            from: found.clip.source_at(found.clip.start),
            to: found.last_frame(),
            smoothing: clip_job::share(settings.smoothing),
        })?;
        self.steadying = Some(SteadyJob {
            target: found.target,
            analysing,
        });
        self.status = clip_job::started(ANALYSING, &self.shortcuts);
        Ok(())
    }

    /// Takes in what an analysis under way says; once it is done, sets the
    /// effect's view and zoom (one undo step). The command bound to Escape
    /// stops it (`Editor::stop_tasks`).
    pub(super) fn steady_news(&mut self, ctx: &egui::Context) {
        let Some(job) = self.steadying.as_ref() else {
            return;
        };
        ctx.request_repaint_after(clip_job::NEWS_EVERY);
        match job.analysing.news() {
            None => {}
            Some(Analysed::Going(done)) => {
                self.status = clip_job::going(ANALYSING, done, &self.shortcuts);
            }
            Some(Analysed::Stopped) => {
                self.steadying = None;
                self.status = "Analysis stopped: nothing changed".into();
            }
            Some(Analysed::Failed(why)) => {
                self.steadying = None;
                self.errors
                    .push(&diag::AppError::new("The clip couldn't be analysed").why(why));
            }
            Some(Analysed::Done(found)) => {
                let Some(job) = self.steadying.take() else {
                    return;
                };
                match self.finish_steady(&job, &found) {
                    Ok(()) => self.status = "Stabilised: the view follows the camera's smoothed path".into(),
                    Err(err) => self.errors.push(&err),
                }
            }
        }
    }

    fn finish_steady(&mut self, job: &SteadyJob, found: &Steadied) -> diag::Result<()> {
        let target = job.target;
        let places = clip_job::places_in_millionths(&found.places);
        self.with_document(|d| {
            clip_job::write_back(d, target, ANALYSING, |e| steadied(e, &places, found.zoom))
        })
    }
}

/// `effect` with the analysis's view keyframes (`places`, in millionths)
/// and `zoom`.
fn steadied(effect: &project::Effect, places: &[(project::Time, (i32, i32))], zoom: f64) -> project::Effect {
    let mut changed = effect.clone();
    // The analysis covers the whole clip: its view keys start afresh.
    changed.keys.clear(Stabilise::MIDDLE..Stabilise::MIDDLE + 2);
    changed = changed.keyed_pair(Stabilise::MIDDLE, places);
    if let EffectKind::Stabilise(mut settings) = changed.kind {
        settings.zoom = clip_job::millionths(zoom);
        changed.kind = EffectKind::Stabilise(settings);
    }
    changed
}
