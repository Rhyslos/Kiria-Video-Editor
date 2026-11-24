//! "Black bars" in the inspector's Crop: the bars along a recording's
//! edges found on a worker (`engine::bars`, frames spread over the clip)
//! and cut away, the part left as large as fits the frame, in one undo
//! step. The Crop section then offers to make the timeline that size.

use super::Editor;
use jobs::{JobHandle, PoolKind, Priority};
use project::{ClipId, Command, SequenceId};

/// Bars being looked for.
#[derive(Debug)]
pub struct FindingBars {
    sequence: SequenceId,
    clip: ClipId,
    job: JobHandle<engine::bars::Bars>,
}

impl Editor {
    /// Starts looking for `clip`'s black bars.
    pub(super) fn find_bars(&mut self, clip: ClipId) {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return;
        };
        let Some(seq) = project.active() else {
            return;
        };
        let Some(found) = seq.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == clip) else {
            return;
        };
        let Some(media) = project.media.get(&found.media).filter(|_| found.is_file()) else {
            self.status = "Only a recording's picture has bars to find.".into();
            return;
        };
        let rotation = media.info.video.as_ref().map_or(0, |v| v.rotation);
        let (path, stream) = (media.path.clone(), found.stream);
        let from = found.source_in.as_secs_f64();
        let to = from + found.source_len().as_secs_f64();
        let job = self.jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "find black bars",
            move |_| engine::bars::find(&path, stream, rotation, from, to),
        );
        self.finding_bars = Some(FindingBars {
            sequence: seq.id,
            clip,
            job,
        });
        self.status = "Looking for black bars…".into();
    }

    /// Cuts the bars away once they are found.
    pub(super) fn bars_news(&mut self, ctx: &egui::Context) {
        let Some(finding) = self.finding_bars.as_ref() else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let Some(result) = finding.job.try_result() else {
            return;
        };
        let (sequence, clip) = (finding.sequence, finding.clip);
        self.finding_bars = None;
        let bars = match result {
            Ok(bars) => bars,
            Err(err) => {
                self.errors.push(&err);
                return;
            }
        };
        if !bars.any() {
            self.status = "No black bars found along the picture's edges.".into();
            return;
        }
        let playhead = self.playhead_on_frame();
        let mut applied = None;
        let result = self.with_document(|d| {
            let project = d.project();
            let Some(seq) = project.sequence(sequence) else {
                return Ok(());
            };
            let Some(found) = seq.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == clip) else {
                return Ok(());
            };
            let rate = seq.settings.frame_rate;
            let at = found.moment(playhead, rate);
            let frame = (seq.settings.width, seq.settings.height);
            let look =
                crate::inspector::cropped_to_fit(&found.look_at(at), bars.picture, bars.shares(), frame);
            applied = Some(bars.inside());
            d.apply(Command::SetLookAt {
                sequence,
                clip,
                at,
                look,
            })
        });
        match (result, applied) {
            (Err(err), _) => self.errors.push(&err),
            (Ok(()), Some((w, h))) => {
                self.status = format!(
                    "Black bars cut away: the picture is {w}×{h}. Crop › Timeline makes the timeline that size."
                );
            }
            (Ok(()), None) => {}
        }
    }
}
