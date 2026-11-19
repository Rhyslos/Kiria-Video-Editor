//! Timeline labels as the person works them on any page that shows the
//! timeline: a click chooses one (and no clips), a drag moves it, its
//! menu gives it a colour or deletes it, and Delete removes the chosen
//! one. Each change is one undo step. Naming one is the label window's
//! (`windows.rs`).

use super::Editor;
use project::{Command, LabelColor, LabelId, TimelineLabel};

impl Editor {
    /// Chooses timeline label `label` (none for `None`); choosing one lets
    /// go of the selected clips, so Delete knows what it removes.
    pub(super) fn select_label(&mut self, label: Option<u64>) {
        self.selected_label = label.map(LabelId);
        if label.is_some() {
            self.selected.clear();
            self.status = format!(
                "Label chosen: drag it to move it, double-click to name it, {} deletes it.",
                self.shortcuts.keys_or_name(ui_kit::Action::Delete)
            );
        }
    }

    /// The chosen timeline label, if it is still on the open timeline.
    pub(super) fn label_chosen(&self) -> Option<LabelId> {
        let id = self.selected_label?;
        let seq = self.document.as_ref()?.project().active()?;
        seq.labels.iter().any(|l| l.id == id).then_some(id)
    }

    /// Moves timeline label `label` to the frame starting at `at` seconds.
    pub(super) fn move_label(&mut self, label: u64, at: f64) {
        self.apply_on_open(|seq| {
            Some(Command::MoveLabel {
                sequence: seq.id,
                label: LabelId(label),
                at: seq.settings.frame_rate.snap(super::seconds(at)),
            })
        });
    }

    /// Gives timeline label `label` the colour at `colour` in the label
    /// colours, keeping its name.
    pub(super) fn colour_label(&mut self, label: u64, colour: usize) {
        let Some(&color) = LabelColor::ALL.get(colour) else {
            return;
        };
        let id = LabelId(label);
        self.apply_on_open(|seq| {
            let name = seq.labels.iter().find(|l| l.id == id)?.name.clone();
            Some(Command::EditLabel {
                sequence: seq.id,
                label: id,
                name,
                color,
            })
        });
    }

    /// Labels the region as a whole (one undo step), then asks for the
    /// label's name; without a region, says how to mark one.
    pub(super) fn label_region(&mut self) -> diag::Result<()> {
        let Some(region) = self.region() else {
            self.status = "Mark a region first: Shift-drag in the ruler, or I and O.".into();
            return Ok(());
        };
        let mut added = None;
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let (sequence, name) = (seq.id, seq.next_label_name());
            let before: Vec<LabelId> = seq.labels.iter().map(|l| l.id).collect();
            d.apply(Command::AddLabel {
                sequence,
                label: TimelineLabel {
                    id: LabelId(0),
                    name,
                    at: region.start,
                    color: LabelColor::Orange,
                    until: Some(region.end),
                },
            })?;
            added = d
                .project()
                .active()
                .and_then(|s| s.labels.iter().find(|l| !before.contains(&l.id)).cloned());
            Ok(())
        })?;
        if let Some(label) = added {
            self.select_label(Some(label.id.0));
            self.label_editor = Some(crate::label_editor::LabelEditor::new(&label));
            self.status = "Region labelled: give it a name. Its menu sets the region to it again.".into();
        }
        Ok(())
    }

    /// Deletes timeline label `label`; it is no longer the chosen one.
    pub(super) fn remove_label(&mut self, label: LabelId) {
        self.apply_on_open(|seq| {
            Some(Command::RemoveLabel {
                sequence: seq.id,
                label,
            })
        });
        if self.selected_label == Some(label) {
            self.selected_label = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A label's menu offers the colours in the project's order, so the
    /// index it hands back is the colour it showed.
    #[test]
    fn the_menu_offers_the_project_label_colours_in_order() {
        assert_eq!(LabelColor::ALL.len(), ui_kit::theme::labels::ALL.len());
        for (color, (name, shown)) in LabelColor::ALL.iter().zip(ui_kit::theme::labels::ALL) {
            assert_eq!(crate::timeline_panel::label_colour(*color), shown, "{name}");
            assert_eq!(format!("{color:?}"), name);
        }
    }
}
