//! The project as it looks while something is being changed: every value
//! being dragged anywhere in the window (a fader, a knob, a slider, a
//! line, a setting in a tool, a grade's wheel), folded into one project once a frame. Every
//! view draws from it and playback follows it, so a change is seen and
//! heard everywhere as it moves, not only where the hand is; it becomes
//! one undo step when let go, where it was made.

use crate::editor::{Editor, view_on};
use project::{Command, Project};

impl Editor {
    /// `p` with everything being dragged, or `None` when nothing is.
    pub(super) fn live_project(&self, p: &Project) -> Option<Project> {
        // The Audio page's tools and mixer, the inspector's values.
        let mut live = self.audio_page.shown(p);
        // A grade being dragged on the Color page (and its before view
        // while there).
        let graded = (self.page == crate::editor::Page::Color)
            .then(|| p.active())
            .flatten()
            .and_then(|seq| {
                self.color_page
                    .graded(seq, &self.selected, self.viewer.playhead, false)
            });
        live = self.color_page.shown(live.as_ref().unwrap_or(p), graded).or(live);
        live = self.inspector.shown(live.as_ref().unwrap_or(p)).or(live);
        // A line's point, a track's header knob or fader, on the timeline
        // shown.
        let view = view_on(self.page, &self.timeline, &self.audio_page.timeline);
        if let Some(line) = view.line_being_set() {
            live = crate::audio_page::lines::with_line_being_set(live.as_ref().unwrap_or(p), line).or(live);
        }
        let turned = view
            .gain_being_set()
            .or_else(|| self.pinned_columns.values().find_map(|c| c.gain_being_set()));
        if let Some((track, db)) = turned {
            live = crate::timeline_panel::with_gain_being_set(live.as_ref().unwrap_or(p), track, db).or(live);
        }
        // A track's pan knob, a clip's level line, a fade's handle.
        if let Some((track, pan)) = view.pan_being_set() {
            let event = ui_kit::TimelineEvent::TrackPan { track, pan };
            live = on_open(live.as_ref().unwrap_or(p), |seq| {
                crate::timeline_panel::track_option_command(seq, &event)
            })
            .or(live);
        }
        if let Some((clip, db)) = view.level_being_set() {
            live = on_open(live.as_ref().unwrap_or(p), |seq| {
                crate::timeline_panel::clip_volume_command(seq, project::ClipId(clip), db)
            })
            .or(live);
        }
        if let Some((clip, sound, fades)) = view.fade_being_set() {
            live = on_open(live.as_ref().unwrap_or(p), |seq| {
                crate::timeline_panel::clip_fade_command(seq, project::ClipId(clip), sound, fades)
            })
            .or(live);
        }
        // The whole mix's fader, beside the timeline or on the Audio page.
        let master = self
            .master
            .gain_being_set()
            .or_else(|| self.audio_page.master.gain_being_set());
        if let Some(db) = master {
            live = with_master_gain(live.as_ref().unwrap_or(p), db).or(live);
        }
        live
    }
}

/// `project` with the change `command` asks of its open timeline, or
/// `None` when it asks none or it can't be made.
fn on_open(
    project: &Project,
    command: impl FnOnce(&project::Sequence) -> Option<Command>,
) -> Option<Project> {
    command(project.active()?)?.apply(project).ok()
}

/// `project` with its open timeline's mix at `db`, or `None` when it is
/// there already.
fn with_master_gain(project: &Project, db: f32) -> Option<Project> {
    let seq = project.active()?;
    if seq.master_gain_db == db {
        return None;
    }
    Command::SetMasterGain {
        sequence: seq.id,
        gain_db: db,
    }
    .apply(project)
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mixs_volume_being_dragged_is_the_projects_while_it_moves() {
        let p = Project::new_automatic("T");
        assert!(with_master_gain(&p, 0.0).is_none(), "as it is");
        let live = with_master_gain(&p, -6.0).expect("changed");
        assert_eq!(live.active().unwrap().master_gain_db, -6.0);
        assert_eq!(
            p.active().unwrap().master_gain_db,
            0.0,
            "the project itself untouched"
        );
    }
}
