//! Unlinking and linking clips: "Unlink" and "Link the selected clips" in
//! the timeline's right-click menu, and Settings › Editing › "Always unlink
//! audio and video" (`project::Document::set_always_unlinked`), which keeps
//! every clip of every project on its own.

use super::*;
use project::ClipId;

/// Said in the status line when a project opens with clips linked while
/// "Always unlink audio and video" is on.
pub(crate) const UNLINKED_ON_OPEN: &str = "Clips unlinked, as Settings › Editing › \"Always unlink audio and video\" asks (undo brings the links back)";

impl Editor {
    /// "Unlink" on `clip`: it, with the other selected clips when it is one
    /// of them, and every clip linked to any of them, each on its own (a
    /// picture and every one of its sound streams apart), as one undo step.
    pub(super) fn unlink(&mut self, clip: ClipId) {
        let clips = acted_on(&self.selected, clip);
        self.apply_and_say(
            |sequence| Command::Unlink { sequence, clips },
            "Unlinked: each clip is clicked, moved and trimmed on its own.",
        );
    }

    /// "Link the selected clips": they are selected, moved, trimmed and
    /// split together from now on (one undo step).
    pub(super) fn link_selected(&mut self) {
        if self.settings.always_unlinked {
            self.status =
                "Settings › Editing › \"Always unlink audio and video\" is on: turn it off to link clips."
                    .into();
            return;
        }
        let clips = self.selected.clone();
        self.apply_and_say(
            |sequence| Command::Link { sequence, clips },
            "Linked: the selected clips move together.",
        );
    }

    /// Applies the command `make` builds for the open timeline and says
    /// `done` in the status line, or shows what went wrong.
    fn apply_and_say(&mut self, make: impl FnOnce(project::SequenceId) -> Command, done: &str) {
        let result = self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(make(sequence))
        });
        match result {
            Ok(()) => self.status = done.into(),
            Err(err) => self.errors.push(&err),
        }
    }

    /// The setting was switched: the open project follows it at once.
    pub(super) fn always_unlinked_changed(&mut self) {
        let on = self.settings.always_unlinked;
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match document.set_always_unlinked(on) {
            Ok(true) => {
                self.status = "Every clip unlinked: pictures and sound streams are worked on apart (undo links them again)."
                    .into();
            }
            Ok(false) => {}
            Err(err) => self.errors.push(&err),
        }
    }
}

/// The clips "Unlink" on `clip` acts on: the selection when `clip` is in
/// it, else `clip` alone.
fn acted_on(selected: &[ClipId], clip: ClipId) -> Vec<ClipId> {
    match selected.contains(&clip) {
        true => selected.to_vec(),
        false => vec![clip],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlink_acts_on_the_selection_only_when_the_clip_is_in_it() {
        let selected = [ClipId(1), ClipId(2)];
        assert_eq!(acted_on(&selected, ClipId(2)), selected);
        assert_eq!(acted_on(&selected, ClipId(3)), [ClipId(3)]);
        assert_eq!(acted_on(&[], ClipId(3)), [ClipId(3)]);
    }
}
