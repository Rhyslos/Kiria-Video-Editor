//! Copying the chosen keyframe marks and pasting them at the playhead on
//! the selected picture clip (Copy and Paste in the shortcut table). What
//! is copied stays in the window for the session; the system clipboard is
//! given a line saying what it is, so the window's paste key reaches the
//! timeline (the window only reports a paste when the clipboard holds
//! something) and pasting into a text field writes something sensible.

use super::{Editor, seconds};
use project::{Clip, Command, CopiedKeyframes, Rate, Sequence, Time};

/// The picture clip `selected` stands for in `seq`: a selected clip on a
/// video track (the top one), or the picture linked to a selected sound clip.
pub(crate) fn picture_of<'s>(seq: &'s Sequence, selected: &[project::ClipId]) -> Option<&'s Clip> {
    let pictures = || seq.video_tracks.iter().rev().flat_map(|track| track.clips.iter());
    pictures().find(|c| selected.contains(&c.id)).or_else(|| {
        let links: Vec<_> = seq
            .audio_tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .filter(|c| selected.contains(&c.id))
            .filter_map(|c| c.link)
            .collect();
        pictures().find(|c| c.link.is_some_and(|link| links.contains(&link)))
    })
}

/// Where the marks of `copied` land when pasted on `clip` at `at`, as the
/// paste puts them: the starts (in seconds) of the frames of those that
/// fit on the clip, and how many are left out because they would land
/// past its ends.
pub(crate) fn pasted_at(clip: &Clip, rate: Rate, at: Time, copied: &CopiedKeyframes) -> (Vec<f64>, usize) {
    let landing = copied.landing(clip, rate, at);
    let left_out = landing.iter().filter(|frame| frame.is_none()).count();
    let landed = landing
        .into_iter()
        .flatten()
        .map(|frame| rate.frame_start(frame).as_secs_f64())
        .collect();
    (landed, left_out)
}

/// The status line after a paste: what was pasted and, when some marks
/// would have landed past the clip's ends, how many were left out.
pub(crate) fn pasted_says(copied: &CopiedKeyframes, left_out: usize) -> String {
    let says = described(copied);
    match left_out {
        0 => format!("Pasted {says} at the playhead."),
        n => {
            let them = if n == 1 {
                "1 mark was".to_owned()
            } else {
                format!("{n} marks were")
            };
            let pasted = copied.len().saturating_sub(n);
            format!(
                "Pasted {pasted} of {says} at the playhead; {them} left out, landing past the clip's ends."
            )
        }
    }
}

/// "2 keyframe marks (opacity, turn)".
pub(crate) fn described(copied: &CopiedKeyframes) -> String {
    let marks = match copied.len() {
        1 => "1 keyframe mark".to_owned(),
        n => format!("{n} keyframe marks"),
    };
    match copied.says() {
        says if says.is_empty() => marks,
        says => format!("{marks} ({says})"),
    }
}

impl Editor {
    /// Copy: the chosen keyframe marks of the open timeline, all their numbers.
    pub(super) fn copy_keyframes(&mut self) -> diag::Result<()> {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return Ok(());
        };
        let rate = seq.settings.frame_rate;
        let chosen = self.timeline.chosen_keyframes().and_then(|(clip, at)| {
            let clip = seq
                .video_tracks
                .iter()
                .flat_map(|track| track.clips.iter())
                .find(|c| c.id.0 == clip)?;
            Some((
                clip,
                at.iter().map(|s| rate.snap(seconds(*s))).collect::<Vec<Time>>(),
            ))
        });
        let Some((clip, frames)) = chosen else {
            self.status = "Nothing to copy: choose keyframe marks on the selected clip first (click one, Shift-click more, or drag a box along the clip's bottom edge).".into();
            return Ok(());
        };
        let copied = CopiedKeyframes::copy(clip, rate, &frames)?;
        let says = described(&copied);
        self.status = format!(
            "Copied {says}. {} pastes them at the playhead on the selected clip.",
            self.shortcuts.keys_or_name(ui_kit::Action::Paste)
        );
        self.clipboard_text = Some(format!("Keyframes: {says}"));
        self.copied_keyframes = Some(copied);
        Ok(())
    }

    /// Paste: the copied keyframes at the playhead on the selected picture
    /// clip, as one undo step; the pasted marks are chosen afterwards.
    pub(super) fn paste_keyframes(&mut self) -> diag::Result<()> {
        let Some(copied) = self.copied_keyframes.clone() else {
            self.status = format!(
                "Nothing to paste: choose keyframe marks and copy them first ({}).",
                self.shortcuts.keys_or_name(ui_kit::Action::Copy)
            );
            return Ok(());
        };
        let at = self.viewer.playhead;
        let target = self.document.as_ref().and_then(|d| {
            let seq = d.project().active()?;
            let clip = picture_of(seq, &self.selected)?;
            let rate = seq.settings.frame_rate;
            Some((seq.id, clip.id, pasted_at(clip, rate, at, &copied)))
        });
        let Some((sequence, clip, (landed, left_out))) = target else {
            self.status =
                "Select a clip with a picture first: copied keyframes are pasted on it at the playhead."
                    .into();
            return Ok(());
        };
        let says = pasted_says(&copied, left_out);
        self.with_document(|d| {
            d.apply(Command::PasteKeyframes {
                sequence,
                clip,
                at,
                keyframes: copied,
            })
        })?;
        self.timeline.choose_keyframes(clip.0, landed);
        self.status = says;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_of_a_selection_is_a_picture_or_the_one_linked_to_its_sound() {
        let project = crate::inspector::tests::project_with_a_clip();
        let seq = project.active().unwrap();
        let picture = &seq.video_tracks[0].clips[0];
        assert_eq!(picture_of(seq, &[picture.id]).map(|c| c.id), Some(picture.id));
        let sound = seq
            .audio_tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.link == picture.link);
        if let Some(sound) = sound.filter(|_| picture.link.is_some()) {
            assert_eq!(picture_of(seq, &[sound.id]).map(|c| c.id), Some(picture.id));
        }
        assert!(picture_of(seq, &[]).is_none());
    }

    #[test]
    fn pasted_marks_land_from_the_playheads_frame() {
        let project = crate::inspector::tests::project_with_a_clip();
        let seq = project.active().unwrap();
        let rate = seq.settings.frame_rate;
        let clip = seq.video_tracks[0].clips[0].clone();
        let copied = CopiedKeyframes {
            marks: vec![
                project::clipboard::CopiedMark {
                    after: Time::ZERO,
                    keys: Vec::new(),
                },
                project::clipboard::CopiedMark {
                    after: Time::seconds(1),
                    keys: Vec::new(),
                },
            ],
        };
        let at = clip.start + Time::seconds(2) + Time::from_ratio(1, 1000).unwrap();
        let (landed, left_out) = pasted_at(&clip, rate, at, &copied);
        let start = rate
            .frame_start(rate.frame_at(clip.start + Time::seconds(2)))
            .as_secs_f64();
        assert_eq!((landed, left_out), (vec![start, start + 1.0], 0));
        // From the clip's last frame the second mark is left out.
        let last = clip.end() - Time::from_ratio(1, 1000).unwrap();
        let (landed, left_out) = pasted_at(&clip, rate, last, &copied);
        assert_eq!((landed.len(), left_out), (1, 1));
        assert_eq!(
            pasted_says(&copied, 0),
            "Pasted 2 keyframe marks at the playhead."
        );
        assert_eq!(
            pasted_says(&copied, 1),
            "Pasted 1 of 2 keyframe marks at the playhead; 1 mark was left out, landing past the clip's ends."
        );
        assert_eq!(described(&copied), "2 keyframe marks");
    }
}
