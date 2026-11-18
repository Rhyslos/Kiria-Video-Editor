//! "Analyse the clip" on a Stabilise effect: how the camera moves over the
//! whole clip is found on a thread of its own (`engine::steady`) while the
//! page goes on; the status line says how far it is and the command bound
//! to Escape stops it
//! (nothing set). What it finds becomes the effect's view keyframes and
//! zoom, one undo step.

use super::Editor;
use engine::steady::{Analyse, Analysed, Analysing, Steadied};
use project::effects::Stabilise;
use project::{ClipId, Command, EffectId, EffectKind, SequenceId, Time};

/// An analysis under way.
#[derive(Debug)]
pub struct SteadyJob {
    sequence: SequenceId,
    clip: ClipId,
    effect: EffectId,
    analysing: Analysing,
}

impl SteadyJob {
    /// Stops the analysis; nothing is set.
    pub(super) fn stop(&self) {
        self.analysing.stop();
    }
}

/// Millionths in one whole.
const ONE: f64 = 1_000_000.0;

impl Editor {
    /// Starts analysing `clip` for its Stabilise `effect`.
    pub(super) fn start_steady(&mut self, clip: ClipId, effect: EffectId) -> diag::Result<()> {
        let Some(document) = self.document.as_ref() else {
            return Ok(());
        };
        let project = document.project();
        let sequence = project.active_sequence;
        let seq = project.active().ok_or_else(|| missing("timeline"))?;
        let found = seq
            .video_tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == clip)
            .ok_or_else(|| missing("clip"))?;
        let media = project.media.get(&found.media).ok_or_else(|| missing("file"))?;
        let video = media.info.video.as_ref().ok_or_else(|| {
            diag::AppError::new("There is no picture to steady").why("The clip's file has no video.")
        })?;
        let Some(EffectKind::Stabilise(settings)) = found.effects.get(effect).map(|e| e.kind) else {
            return Err(missing("effect"));
        };
        let rate = seq.settings.frame_rate;
        let last = rate.frame_start(rate.frame_at(found.end()) - 1).max(found.start);
        let analysing = Analysing::start(Analyse {
            path: media.path.clone(),
            stream: found.stream,
            rotation: video.rotation,
            rate: video.frame_rate,
            from: found.source_at(found.start),
            to: found.source_at(last),
            smoothing: f64::from(settings.smoothing) / ONE,
        })?;
        self.steadying = Some(SteadyJob {
            sequence,
            clip,
            effect,
            analysing,
        });
        self.status = format!(
            "Analysing the camera's movement…{}",
            super::stops_it(&self.shortcuts)
        );
        Ok(())
    }

    /// Takes in what an analysis under way says; once it is done, sets the
    /// effect's view and zoom (one undo step). The command bound to Escape
    /// stops it (`Editor::stop_tasks`).
    pub(super) fn steady_news(&mut self, ctx: &egui::Context) {
        let Some(job) = self.steadying.as_ref() else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        match job.analysing.news() {
            None => {}
            Some(Analysed::Going(done)) => {
                self.status = format!(
                    "Analysing the camera's movement… {:.0} %{}",
                    done * 100.0,
                    super::stops_it(&self.shortcuts)
                );
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
        let (sequence, clip, effect) = (job.sequence, job.clip, job.effect);
        let places: Vec<(Time, (i32, i32))> = found
            .places
            .iter()
            .map(|(at, (x, y))| (*at, ((x * ONE).round() as i32, (y * ONE).round() as i32)))
            .collect();
        self.with_document(|d| {
            let effects = d
                .project()
                .sequence(sequence)
                .and_then(|seq| {
                    seq.video_tracks
                        .iter()
                        .flat_map(|t| t.clips.iter())
                        .find(|c| c.id == clip)
                })
                .map(|c| c.effects.clone())
                .ok_or_else(|| missing("clip"))?;
            let mut next = effects;
            let at = next
                .0
                .iter()
                .position(|e| e.id == effect)
                .ok_or_else(|| missing("effect"))?;
            let mut changed = next.0[at].clone();
            // The analysis covers the whole clip: its view keys start afresh.
            changed.keys.clear(Stabilise::MIDDLE..Stabilise::MIDDLE + 2);
            changed = changed.keyed_pair(Stabilise::MIDDLE, &places);
            if let EffectKind::Stabilise(mut settings) = changed.kind {
                settings.zoom = (found.zoom * ONE).round() as i32;
                changed.kind = EffectKind::Stabilise(settings);
            }
            next.0[at] = changed;
            d.apply(Command::SetEffects {
                sequence,
                clip,
                effects: Box::new(next),
            })
        })
    }
}

fn missing(what: &str) -> diag::AppError {
    diag::AppError::new(format!("That {what} no longer exists"))
        .why("It may have been removed while the clip was being analysed.")
}
