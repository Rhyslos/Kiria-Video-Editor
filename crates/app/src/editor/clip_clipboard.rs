//! Copy, Cut and Paste for clips, sharing their keys with keyframes: Copy
//! copies what is chosen (keyframe marks on the selected clip when some are
//! chosen, else the selected clips with their linked clips), and Paste
//! pastes whichever was copied last. Clips are pasted at the edit point (the
//! edit needle, or the playhead when they are merged in Settings) of the
//! open timeline, which may be another one than they came from, as one undo
//! step (`Command::PasteClips`); the pasted clips are selected. Cut copies
//! the selected clips and removes them, closing the gap when Settings says
//! so ("Cut closes the gap").

use super::Editor;
use project::{ClipId, Command, CopiedClips, Time};
use std::collections::HashSet;

/// What the status line says after pasting `copied`, `left_out` of them
/// left out, at `place` ("the edit needle", "the playhead").
pub(crate) fn pasted_says(copied: &CopiedClips, left_out: usize, place: &str) -> String {
    match left_out {
        0 => format!("Pasted {} at {place}.", copied.says()),
        n => {
            let them = if n == 1 {
                "1 was left out: it".to_owned()
            } else {
                format!("{n} were left out: they")
            };
            format!(
                "Pasted {} of {} at {place}; {them} would play this timeline inside itself, or what it played is no longer in the project.",
                copied.len().saturating_sub(n),
                copied.says()
            )
        }
    }
}

impl Editor {
    /// Copy: the chosen keyframe marks, or else the selected clips.
    pub(super) fn copy(&mut self) -> diag::Result<()> {
        if self.timeline.chosen_keyframes().is_some() || self.selected.is_empty() {
            return self.copy_keyframes();
        }
        self.copy_clips().map(|_| ())
    }

    /// Paste: whichever was copied last.
    pub(super) fn paste(&mut self) -> diag::Result<()> {
        if self.copied_clips.is_some() {
            self.paste_clips()
        } else {
            self.paste_keyframes()
        }
    }

    /// Cut: the selected clips copied, then removed (the gap closed when
    /// Settings says so). Keyframe marks are not cut.
    pub(super) fn cut(&mut self) -> diag::Result<()> {
        if self.selected.is_empty() {
            self.status = "Nothing to cut: click a clip on the timeline first (Shift-click more).".into();
            return Ok(());
        }
        let Some(copied) = self.copy_clips()? else {
            return Ok(());
        };
        let ripple = self.settings.cut_closes_gap;
        self.remove_selected(ripple)?;
        self.status = format!(
            "Cut {}{}. {} pastes them at the edit point.",
            copied.says(),
            if ripple { " and closed the gap" } else { "" },
            self.shortcuts.keys_or_name(ui_kit::Action::Paste)
        );
        Ok(())
    }

    /// The selected clips and their linked clips, kept to paste; `None`
    /// when nothing is open.
    fn copy_clips(&mut self) -> diag::Result<Option<CopiedClips>> {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return Ok(None);
        };
        let sequence = project.active_sequence;
        let selected = self.selected_that_exist();
        let copied = CopiedClips::copy(project, sequence, &selected)?;
        self.status = format!(
            "Copied {}. {} pastes them at the edit point, in this timeline or another.",
            copied.says(),
            self.shortcuts.keys_or_name(ui_kit::Action::Paste)
        );
        self.clipboard_text = Some(format!("Clips: {}", copied.says()));
        self.copied_keyframes = None;
        self.copied_clips = Some(copied.clone());
        Ok(Some(copied))
    }

    /// The copied clips at the edit point of the open timeline, one undo
    /// step; the pasted clips are selected.
    fn paste_clips(&mut self) -> diag::Result<()> {
        let Some(copied) = self.copied_clips.clone() else {
            return Ok(());
        };
        let at = self.edit_point();
        let place = if self.settings.edit_at_playhead {
            "the playhead"
        } else {
            "the edit needle"
        };
        let mut before: Vec<ClipId> = Vec::new();
        let mut left_out = 0;
        self.with_document(|d| {
            let p = d.project();
            let sequence = p.active_sequence;
            before = all_clips(p);
            left_out = copied.left_out(p, sequence);
            d.apply(Command::PasteClips {
                sequence,
                at,
                clips: copied.clone(),
            })
        })?;
        if let Some(p) = self.document.as_ref().map(|d| d.project()) {
            self.selected = added_clips(&before, all_clips(p));
        }
        self.status = pasted_says(&copied, left_out, place);
        Ok(())
    }

    /// The selected clips that are still on the open timeline (an undo may
    /// have taken some away).
    pub(super) fn selected_that_exist(&self) -> Vec<ClipId> {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return Vec::new();
        };
        self.selected
            .iter()
            .copied()
            .filter(|clip| seq.tracks().any(|t| t.clips.iter().any(|c| c.id == *clip)))
            .collect()
    }

    /// Removes the selected clips as one step, closing the gaps on every
    /// track with `ripple`; the selection is emptied when they went.
    pub(super) fn remove_selected(&mut self, ripple: bool) -> diag::Result<()> {
        let clips = self.selected_that_exist();
        self.with_document(|d| {
            let sequence = d.project().active_sequence;
            // One step: every clip is checked first, then all go or none do.
            d.apply(if ripple {
                Command::RippleRemoveClips { sequence, clips }
            } else {
                Command::RemoveClips {
                    sequence,
                    clips,
                    ripple: false,
                }
            })
        })?;
        self.selected.clear();
        Ok(())
    }
}

impl Editor {
    /// Closes `gap` on every track, one undo step; refused with the reason
    /// when something on another track is in it.
    pub(super) fn close_gap(&mut self, gap: ui_kit::Gap) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let (start, end) = gap_edges(seq, &gap);
            d.apply(Command::CloseGap {
                sequence: seq.id,
                start,
                end,
            })
        })?;
        self.timeline.clear_gap();
        self.status = "Closed the gap: everything after it moved back, on every track.".into();
        Ok(())
    }
}

/// The exact edges of `gap`: the end of the clip before it (or the
/// timeline's start) and the start of the clip after, found by their ids,
/// so edges off the timeline's frames (sound lined up to the sample) are
/// kept. A clip no longer there falls back to the nearest frame.
fn gap_edges(seq: &project::Sequence, gap: &ui_kit::Gap) -> (Time, Time) {
    let rate = seq.settings.frame_rate;
    let clip = |id: u64| {
        seq.tracks()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == ClipId(id))
    };
    let start = match gap.before {
        None => Time::ZERO,
        Some(id) => clip(id).map_or_else(|| rate.snap(super::seconds(gap.start)), |c| c.end()),
    };
    let end = clip(gap.after).map_or_else(|| rate.snap(super::seconds(gap.end)), |c| c.start);
    (start, end)
}

/// The clips in `after` that are not in `before`, in `after`'s order; a
/// set, so a paste on a timeline of thousands of clips stays quick.
pub(super) fn added_clips(before: &[ClipId], after: Vec<ClipId>) -> Vec<ClipId> {
    let before: HashSet<ClipId> = before.iter().copied().collect();
    after.into_iter().filter(|c| !before.contains(c)).collect()
}

/// Every clip of the open timeline.
pub(super) fn all_clips(p: &project::Project) -> Vec<ClipId> {
    p.active()
        .map(|s| s.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_says_what_was_pasted_and_what_was_left_out() {
        let project = crate::inspector::tests::project_with_a_clip();
        let sequence = project.active_sequence;
        let first = project.active().unwrap().video_tracks[0].clips[0].id;
        let copied = CopiedClips::copy(&project, sequence, &[first]).unwrap();
        let n = copied.len();
        assert!(pasted_says(&copied, 0, "the edit needle").starts_with("Pasted "));
        assert!(pasted_says(&copied, 0, "the edit needle").ends_with("at the edit needle."));
        let some = pasted_says(&copied, 1, "the playhead");
        assert!(some.contains(&format!("{} of", n - 1)), "{some}");
        assert!(some.contains("1 was left out"), "{some}");
    }

    #[test]
    fn the_pasted_clips_are_the_new_ones_in_timeline_order() {
        let before: Vec<ClipId> = [1, 2, 3].map(ClipId).to_vec();
        let after: Vec<ClipId> = [7, 1, 5, 2, 3, 6].map(ClipId).to_vec();
        assert_eq!(added_clips(&before, after), [7, 5, 6].map(ClipId).to_vec());
        assert!(added_clips(&before, before.clone()).is_empty());
        assert_eq!(added_clips(&[], vec![ClipId(4)]), vec![ClipId(4)]);
    }

    /// Prints how long finding the pasted clips takes on a timeline of
    /// 5 000 clips, against the list-scanning way it was found before.
    #[test]
    #[ignore = "a measurement: cargo test -p app -- --ignored --nocapture pasted_clips"]
    fn finding_pasted_clips_among_five_thousand_is_quick() {
        let before: Vec<ClipId> = (0..5000).map(ClipId).collect();
        let after: Vec<ClipId> = (0..5010).map(ClipId).collect();
        let start = std::time::Instant::now();
        let found = added_clips(&before, after.clone());
        let now = start.elapsed();
        let start = std::time::Instant::now();
        let scanned: Vec<ClipId> = after.into_iter().filter(|c| !before.contains(c)).collect();
        let scanning = start.elapsed();
        assert_eq!(found, scanned);
        println!("pasted clips, 5 000 clips: set {now:?}, list scan {scanning:?}");
    }

    /// A gap after sound lined up to the sample (its end off the frames)
    /// closes at that exact end, even with another clip's end less than a
    /// microsecond before it on another track.
    #[test]
    fn a_gap_off_the_frames_closes_at_the_exact_edges() {
        let mut p = crate::inspector::tests::project_with_a_clip();
        let video = p.active().unwrap().video_tracks[0].clips[0].clone();
        let sound = |p: &mut project::Project, start: Time, duration: Time| project::Clip {
            id: p.allocate_id(),
            start,
            duration,
            stream: 0,
            ..video.clone()
        };
        let edge = Time::seconds(1) + Time::from_ratio(1, 48_000).unwrap();
        let decoy = sound(&mut p, Time::ZERO, edge - Time::from_ratio(1, 2_000_000).unwrap());
        let before = sound(&mut p, Time::ZERO, edge);
        let after = sound(&mut p, Time::seconds(3), Time::seconds(2));
        let mut a2 = p.new_track(project::TrackKind::Audio, "A2");
        a2.clips.push_back(before.clone());
        a2.clips.push_back(after.clone());
        let seq = &mut p.sequences[0];
        seq.video_tracks[0].clips.clear();
        seq.audio_tracks[0].clips.push_back(decoy);
        seq.audio_tracks.push_back(a2);
        let gap = ui_kit::Gap {
            track: 0,
            start: edge.as_secs_f64(),
            end: 3.0,
            before: Some(before.id.0),
            after: after.id.0,
        };
        let seq = p.active().unwrap();
        let (start, end) = gap_edges(seq, &gap);
        assert_eq!((start, end), (edge, Time::seconds(3)));
        let closed = Command::CloseGap {
            sequence: seq.id,
            start,
            end,
        }
        .apply(&p)
        .unwrap();
        let moved = &closed.active().unwrap().audio_tracks[1].clips[1];
        assert_eq!(moved.start, edge, "right after the sound, to the sample");
        // A gap from the timeline's start, and a clip that is gone: the
        // nearest frame of the seconds given.
        let from_start = ui_kit::Gap {
            before: None,
            after: 999,
            end: 2.01,
            ..gap
        };
        let rate = seq.settings.frame_rate;
        assert_eq!(
            gap_edges(seq, &from_start),
            (Time::ZERO, rate.snap(Time::from_ratio(201, 100).unwrap()))
        );
    }
}
