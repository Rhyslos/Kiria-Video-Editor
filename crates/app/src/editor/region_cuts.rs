//! What can be done with the region on the Edit page, besides looping it:
//! remove it (closing the gap), remove it leaving a gap, or keep only it.
//! Each is one undo step on every track: `RemoveRanges` (the stretch, or
//! what lies before and after it) or `LiftRange`. The region goes once its
//! stretch is gone, and the playhead goes to where the cut was.

use super::Editor;
use project::{Command, Region, Time};
use ui_kit::RegionCut;

/// The command for `cut` of `region` on a timeline `length` long; `None`
/// when keeping it would remove nothing.
pub(crate) fn command(
    sequence: project::SequenceId,
    region: Region,
    length: Time,
    cut: RegionCut,
) -> Option<Command> {
    Some(match cut {
        RegionCut::Remove => Command::RemoveRanges {
            sequence,
            ranges: vec![region],
        },
        RegionCut::Lift => Command::LiftRange {
            sequence,
            range: region,
        },
        RegionCut::Keep => {
            let ranges: Vec<Region> = [
                Region::between(Time::ZERO, region.start),
                Region::between(region.end, length.max(region.end)),
            ]
            .into_iter()
            .flatten()
            .collect();
            if ranges.is_empty() {
                return None;
            }
            Command::RemoveRanges { sequence, ranges }
        }
    })
}

/// What the status line says after `cut`.
pub(crate) fn says(cut: RegionCut) -> &'static str {
    match cut {
        RegionCut::Remove => "Removed the region from every track and closed the gap.",
        RegionCut::Lift => "Removed the region from every track, leaving a gap.",
        RegionCut::Keep => "Kept only the region: what was before and after it is gone.",
    }
}

impl Editor {
    /// Does `cut` to the open timeline's region, as one undo step.
    pub(super) fn cut_region(&mut self, cut: RegionCut) -> diag::Result<()> {
        let mut done = false;
        let mut start = Time::ZERO;
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let Some(region) = seq.region else {
                return Err(diag::AppError::new("There is no region")
                    .why("These act on the region: the stretch marked in the ruler.")
                    .fix("Mark in and out (I and O), or Shift-drag in the ruler."));
            };
            start = region.start;
            let Some(command) = command(seq.id, region, seq.length(), cut) else {
                return Ok(());
            };
            d.apply(command)?;
            done = true;
            Ok(())
        })?;
        if done {
            if cut != RegionCut::Lift {
                self.set_region(None)?;
            }
            let at = if cut == RegionCut::Keep { Time::ZERO } else { start };
            self.go_to(at);
            self.status = says(cut).into();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeping_the_region_removes_what_is_before_and_after_it() {
        let project = crate::inspector::tests::project_with_a_clip();
        let seq = project.active().unwrap();
        let length = seq.length();
        let region = Region::between(Time::seconds(2), Time::seconds(4)).unwrap();
        let kept = command(seq.id, region, length, RegionCut::Keep)
            .unwrap()
            .apply(&project)
            .unwrap();
        assert_eq!(kept.active().unwrap().length(), Time::seconds(2));
        let removed = command(seq.id, region, length, RegionCut::Remove)
            .unwrap()
            .apply(&project)
            .unwrap();
        assert_eq!(removed.active().unwrap().length(), length - Time::seconds(2));
        let lifted = command(seq.id, region, length, RegionCut::Lift)
            .unwrap()
            .apply(&project)
            .unwrap();
        assert_eq!(lifted.active().unwrap().length(), length, "nothing moved");
        // The whole timeline marked: nothing to keep away.
        let all = Region::between(Time::ZERO, length).unwrap();
        assert!(command(seq.id, all, length, RegionCut::Keep).is_none());
    }
}
