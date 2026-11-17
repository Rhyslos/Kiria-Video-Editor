//! "Follow it from here" on a blur or pixelate box: the box's Left and Top
//! keyframes from the playhead to the clip's end, found on a thread of its
//! own (`engine::track`) while the page goes on. The status line says how
//! far it is; Escape stops it, keeping what it found. What it finds is one
//! undo step on the effect.

use super::Editor;
use engine::track::{Follow, Followed, Following};
use project::{ClipId, Command, EffectId, SequenceId, Time};

/// A follow under way.
#[derive(Debug)]
pub struct FollowJob {
    sequence: SequenceId,
    clip: ClipId,
    effect: EffectId,
    following: Following,
}

/// Millionths in one whole.
const ONE: f64 = 1_000_000.0;

impl Editor {
    /// Starts following the box of `effect` on `clip` from the playhead.
    pub(super) fn start_follow(&mut self, clip: ClipId, effect: EffectId) -> diag::Result<()> {
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
            diag::AppError::new("There is no picture to follow").why("The clip's file has no video.")
        })?;
        let fx = found.effects.get(effect).ok_or_else(|| missing("effect"))?;
        let rate = seq.settings.frame_rate;
        let from = found.source_at(self.playhead_on_frame().max(found.start));
        let last = rate.frame_start(rate.frame_at(found.end()) - 1).max(found.start);
        let to = found.source_at(last);
        let numbers = fx.kind_at(from).numbers();
        let Some(range) = fx.kind.spot_numbers() else {
            return Ok(());
        };
        let area = [0, 1, 2, 3].map(|i| f64::from(numbers[range.start + i]) / ONE);
        let following = Following::start(Follow {
            path: media.path.clone(),
            stream: found.stream,
            rotation: video.rotation,
            rate: video.frame_rate,
            from,
            to,
            area,
        })?;
        self.following = Some(FollowJob {
            sequence,
            clip,
            effect,
            following,
        });
        self.status = "Following the box… (Escape stops it)".into();
        Ok(())
    }

    /// Takes in what a follow under way says; once it is done, sets the
    /// box's keyframes (one undo step). Escape stops it.
    pub(super) fn follow_news(&mut self, ctx: &egui::Context) {
        let Some(job) = self.following.as_ref() else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            job.following.stop();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        match job.following.news() {
            None => {}
            Some(Followed::Going(done)) => {
                self.status = format!("Following the box… {:.0} % (Escape stops it)", done * 100.0);
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
                let places: Vec<(Time, (i32, i32))> = places
                    .into_iter()
                    .map(|(at, (x, y))| (at, ((x * ONE).round() as i32, (y * ONE).round() as i32)))
                    .collect();
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
        let (sequence, clip, effect) = (job.sequence, job.clip, job.effect);
        self.with_document(|d| {
            let project = d.project();
            let effects = project
                .sequence(sequence)
                .and_then(|seq| {
                    seq.video_tracks
                        .iter()
                        .flat_map(|t| t.clips.iter())
                        .find(|c| c.id == clip)
                })
                .map(|c| c.effects.clone())
                .ok_or_else(|| missing("clip"))?;
            let mut next = effects.clone();
            let Some(at) = next.0.iter().position(|e| e.id == effect) else {
                return Err(missing("effect"));
            };
            next.0[at] = next.0[at].following(places);
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
        .why("It may have been removed while the box was being followed.")
}
