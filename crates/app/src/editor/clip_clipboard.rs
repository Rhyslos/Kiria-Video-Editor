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
use project::{ClipId, Command, CopiedClips};

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
            self.selected = all_clips(p).into_iter().filter(|c| !before.contains(c)).collect();
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
    /// Closes the gap `start`..`end` (seconds, put on frames) on every
    /// track, one undo step; refused with the reason when something on
    /// another track is in it.
    pub(super) fn close_gap(&mut self, start: f64, end: f64) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let rate = seq.settings.frame_rate;
            let (start, end) = (rate.snap(super::seconds(start)), rate.snap(super::seconds(end)));
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

/// Every clip of the open timeline.
fn all_clips(p: &project::Project) -> Vec<ClipId> {
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
}
