//! What the jobs on one effect of a picture clip share: "Follow it from
//! here" (`follow.rs`) and "Analyse the clip" (`steady.rs`). Each runs on a
//! thread of its own while the page goes on: the clip, its file and the
//! effect are found when it starts (`find`), the status line says how far
//! it is (`started`, `going`; the command bound to Escape stops it), and
//! what it finds is written back to the effect as one undo step
//! (`write_back`). Settings are kept in millionths (`millionths`, `share`).

use super::ShortcutTable;
use project::{
    Clip, ClipId, Command, Document, Effect, EffectId, MediaRef, Project, Sequence, SequenceId, Time,
};
use project::{VideoStream, effects::Effects};

/// How often the window asks a job under way for news.
pub(super) const NEWS_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// A job, in the words its messages use.
#[derive(Debug, Clone, Copy)]
pub(super) struct Doing {
    /// What the status line says while it runs: "Following the box…".
    pub running: &'static str,
    /// What it needs a picture for: "follow", as in "There is no picture to
    /// follow".
    pub verb: &'static str,
    /// What was going on if its clip went away: "the box was being
    /// followed".
    pub during: &'static str,
}

/// The effect a job is on: where what it finds is written back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Target {
    pub sequence: SequenceId,
    pub clip: ClipId,
    pub effect: EffectId,
}

/// What a job starts from: its clip on the open timeline, the clip's file
/// with its picture, and the effect.
#[derive(Debug)]
pub(super) struct Found<'a> {
    pub target: Target,
    pub seq: &'a Sequence,
    pub clip: &'a Clip,
    pub media: &'a MediaRef,
    pub video: &'a VideoStream,
    pub effect: &'a Effect,
}

impl Found<'_> {
    /// The moment of the file at the start of the clip's last frame on the
    /// timeline (its first when it is shorter than a frame).
    pub fn last_frame(&self) -> Time {
        let rate = self.seq.settings.frame_rate;
        let last = rate
            .frame_start(rate.frame_at(self.clip.end()) - 1)
            .max(self.clip.start);
        self.clip.source_at(last)
    }
}

/// Finds `effect` on picture clip `clip` of the open timeline, with the
/// clip's file and its picture.
pub(super) fn find(
    project: &Project,
    clip: ClipId,
    effect: EffectId,
    doing: Doing,
) -> diag::Result<Found<'_>> {
    let seq = project.active().ok_or_else(|| missing("timeline", doing))?;
    let found = clip_in(seq, clip).ok_or_else(|| missing("clip", doing))?;
    let media = project
        .media
        .get(&found.media)
        .ok_or_else(|| missing("file", doing))?;
    let video = media.info.video.as_ref().ok_or_else(|| {
        diag::AppError::new(format!("There is no picture to {}", doing.verb))
            .why("The clip's file has no video.")
    })?;
    let fx = found
        .effects
        .get(effect)
        .ok_or_else(|| missing("effect", doing))?;
    Ok(Found {
        target: Target {
            sequence: project.active_sequence,
            clip,
            effect,
        },
        seq,
        clip: found,
        media,
        video,
        effect: fx,
    })
}

/// Writes `change` of the job's effect back as one undo step
/// (`SetEffects`), or says what went away while the job ran.
pub(super) fn write_back(
    document: &mut Document,
    target: Target,
    doing: Doing,
    change: impl FnOnce(&Effect) -> Effect,
) -> diag::Result<()> {
    let effects: &Effects = document
        .project()
        .sequence(target.sequence)
        .and_then(|seq| clip_in(seq, target.clip))
        .map(|c| &c.effects)
        .ok_or_else(|| missing("clip", doing))?;
    let mut next = effects.clone();
    let at = next
        .0
        .iter()
        .position(|e| e.id == target.effect)
        .ok_or_else(|| missing("effect", doing))?;
    next.0[at] = change(&next.0[at]);
    document.apply(Command::SetEffects {
        sequence: target.sequence,
        clip: target.clip,
        effects: Box::new(next),
    })
}

/// The status line as a job starts.
pub(super) fn started(doing: Doing, shortcuts: &ShortcutTable) -> String {
    format!("{}{}", doing.running, super::stops_it(shortcuts))
}

/// The status line while a job goes on, `done` of the way (0 to 1).
pub(super) fn going(doing: Doing, done: f32, shortcuts: &ShortcutTable) -> String {
    format!(
        "{} {:.0} %{}",
        doing.running,
        done * 100.0,
        super::stops_it(shortcuts)
    )
}

/// Millionths in one whole: how effect settings are kept.
const ONE: f64 = 1_000_000.0;

/// `whole` (a share, 1 is all) in millionths.
pub(super) fn millionths(whole: f64) -> i32 {
    (whole * ONE).round() as i32
}

/// A setting kept in millionths as a share.
pub(super) fn share(millionths: i32) -> f64 {
    f64::from(millionths) / ONE
}

/// Places found (shares of the picture) in millionths, as keyframes keep
/// them.
pub(super) fn places_in_millionths(places: &[(Time, (f64, f64))]) -> Vec<(Time, (i32, i32))> {
    places
        .iter()
        .map(|&(at, (x, y))| (at, (millionths(x), millionths(y))))
        .collect()
}

/// Picture clip `clip` of `seq`.
fn clip_in(seq: &Sequence, clip: ClipId) -> Option<&Clip> {
    seq.video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
}

/// "That clip no longer exists", removed while the job ran.
pub(super) fn missing(what: &str, doing: Doing) -> diag::AppError {
    diag::AppError::new(format!("That {what} no longer exists"))
        .why(format!("It may have been removed while {}.", doing.during))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::EffectKind;
    use project::effects::Blur;

    const DOING: Doing = Doing {
        running: "Doing it…",
        verb: "do it to",
        during: "it was being done",
    };

    /// The test project with a Blur on its clip, and the effect's number.
    fn blurred() -> (Project, ClipId, EffectId) {
        let project = crate::inspector::tests::project_with_a_clip();
        let seq = project.active().unwrap();
        let clip = seq.video_tracks[0].clips[0].id;
        let project = Command::AddEffect {
            sequence: seq.id,
            clip,
            kind: EffectKind::Blur(Blur::USUAL),
        }
        .apply(&project)
        .unwrap();
        let effect = project.active().unwrap().video_tracks[0].clips[0].effects.0[0].id;
        (project, clip, effect)
    }

    #[test]
    fn a_job_finds_its_clip_file_and_effect() {
        let (project, clip, effect) = blurred();
        let found = find(&project, clip, effect, DOING).unwrap();
        assert_eq!(
            found.target,
            Target {
                sequence: project.active_sequence,
                clip,
                effect
            }
        );
        assert_eq!(found.clip.id, clip);
        assert_eq!(found.effect.id, effect);
        assert_eq!(found.media.id, found.clip.media);
        // The last frame starts one frame before the clip's end.
        let rate = found.seq.settings.frame_rate;
        let last = rate.frame_start(rate.frame_at(found.clip.end()) - 1);
        assert_eq!(found.last_frame(), found.clip.source_at(last));
        assert!(found.last_frame() < found.clip.source_at(found.clip.end()));
    }

    #[test]
    fn what_went_away_is_named_with_what_the_job_was_doing() {
        let (project, clip, effect) = blurred();
        let err = find(&project, ClipId(9999), effect, DOING).unwrap_err();
        assert_eq!(err.what(), "That clip no longer exists");
        assert_eq!(
            err.why_text(),
            Some("It may have been removed while it was being done.")
        );
        let err = find(&project, clip, EffectId(9999), DOING).unwrap_err();
        assert_eq!(err.what(), "That effect no longer exists");
        // A file without a picture.
        let mut project = project;
        let media = project.active().unwrap().video_tracks[0].clips[0].media;
        let mut file = project.media.get(&media).unwrap().clone();
        file.info.video = None;
        project.media.insert(media, file);
        let err = find(&project, clip, effect, DOING).unwrap_err();
        assert_eq!(err.what(), "There is no picture to do it to");
    }

    #[test]
    fn what_a_job_finds_is_one_undo_step_on_its_effect() {
        let dir = tempfile::tempdir().unwrap();
        let (project, clip, effect) = blurred();
        let path = dir.path().join(format!("job.{}", brand::PROJECT_FILE_EXTENSION));
        let mut d = Document::create(&path, project.clone()).unwrap();
        let target = find(&project, clip, effect, DOING).unwrap().target;
        write_back(&mut d, target, DOING, |e| {
            let mut changed = e.clone();
            changed.off = true;
            changed
        })
        .unwrap();
        let after = &d.project().active().unwrap().video_tracks[0].clips[0];
        assert!(after.effects.get(effect).unwrap().off);
        assert!(d.undo().unwrap());
        assert_eq!(d.project(), &project, "one undo takes it back");
        // The effect taken away while the job ran: nothing is written.
        let gone = Target {
            effect: EffectId(9999),
            ..target
        };
        let err = write_back(&mut d, gone, DOING, Clone::clone).unwrap_err();
        assert_eq!(err.what(), "That effect no longer exists");
        assert_eq!(d.project(), &project);
    }

    #[test]
    fn the_status_line_says_how_far_a_job_is_and_how_to_stop_it() {
        let table = ShortcutTable::default();
        let stops = super::super::stops_it(&table);
        assert!(!stops.is_empty(), "Escape is bound at first");
        assert_eq!(started(DOING, &table), format!("Doing it…{stops}"));
        assert_eq!(going(DOING, 0.426, &table), format!("Doing it… 43 %{stops}"));
    }

    #[test]
    fn settings_go_to_millionths_and_back() {
        assert_eq!(millionths(0.25), 250_000);
        assert_eq!(millionths(0.000_000_6), 1, "rounded");
        assert_eq!(share(500_000), 0.5);
        let at = Time::seconds(1);
        assert_eq!(
            places_in_millionths(&[(at, (0.1, 0.9))]),
            vec![(at, (100_000, 900_000))]
        );
    }
}
