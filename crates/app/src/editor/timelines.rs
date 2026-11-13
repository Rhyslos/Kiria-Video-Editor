//! Several timelines in one project: the tabs above the timeline, opening
//! another timeline (not an edit: see `project::History::open`), and the
//! commands for timelines, nested timelines and adjustment layers.

use super::{Editor, seconds};
use project::{ClipId, Command, Project, SequenceId, Time, TrackId};
use ui_kit::timeline_tabs::{TabsEvent, TimelineTab};

/// The tabs for `project`'s timelines, in the project's order.
pub(crate) fn tabs(project: &Project) -> Vec<TimelineTab> {
    let open = project.active_sequence;
    project
        .sequences
        .iter()
        .map(|seq| TimelineTab {
            id: seq.id.0,
            name: seq.name.clone(),
            info: format!(
                "{} · {}",
                crate::import::settings_text(&seq.settings),
                ui_kit::time_text::length(seq.length().as_secs_f64())
            ),
            open: seq.id == open,
            cannot_place: if seq.id == open {
                Some("It is the open timeline: a timeline can't play inside itself.".into())
            } else if project.plays(seq.id, open) {
                Some("It plays the open timeline, which would end up inside itself.".into())
            } else {
                None
            },
        })
        .collect()
}

impl Editor {
    /// Call every frame: when another timeline has opened (a tab, undo,
    /// a new timeline), playing stops, the playhead goes where it was in
    /// that timeline, nothing is selected and the timeline is fitted.
    pub(super) fn follow_open_timeline(&mut self) {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return;
        };
        let open = project.active_sequence;
        let first = project.sequences.front().map(|s| s.id);
        // The layout keeps which one is open; the first needs no note.
        self.timeline.showing = (Some(open) != first).then_some(open.0);
        let Some(before) = self.shown_sequence.replace(open) else {
            return;
        };
        if before == open {
            return;
        }
        self.playheads.insert(before, self.viewer.playhead);
        if self.playback.take().is_some() {
            self.after_play_change();
        }
        self.selected.clear();
        self.fitted = false;
        let at = self.playheads.get(&open).copied().unwrap_or(Time::ZERO);
        self.go_to(at);
    }

    /// Opens the timeline `id` of the project.
    pub(super) fn open_timeline(&mut self, id: SequenceId) {
        if let Some(document) = self.document.as_mut()
            && document.open_sequence(id)
        {
            self.follow_open_timeline();
            if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                self.status = format!("{} is open.", seq.name);
            }
        }
    }

    /// Something done with the timeline tabs.
    pub(super) fn tabs_event(&mut self, event: TabsEvent) {
        let result = match event {
            TabsEvent::Open(id) => {
                self.open_timeline(SequenceId(id));
                Ok(())
            }
            TabsEvent::New => self.new_timeline(false),
            TabsEvent::Duplicate(id) => {
                self.open_timeline(SequenceId(id));
                self.new_timeline(true)
            }
            TabsEvent::Rename(id, name) => self.with_document(|d| {
                d.apply(Command::RenameSequence {
                    sequence: SequenceId(id),
                    name,
                })
            }),
            TabsEvent::Remove(id) => {
                let name = self.timeline_name(SequenceId(id));
                let result = self.with_document(|d| {
                    d.apply(Command::RemoveSequence {
                        sequence: SequenceId(id),
                    })
                });
                if result.is_ok() {
                    let undo = self.shortcuts.keys_or_name(ui_kit::Action::Undo);
                    self.status = format!("{name} was removed ({undo} brings it back).");
                }
                result
            }
            TabsEvent::Place(id) => self.place_timeline(SequenceId(id), None, None),
            TabsEvent::Settings(id) => {
                self.open_timeline(SequenceId(id));
                self.show_timeline_settings = true;
                Ok(())
            }
        };
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }

    fn timeline_name(&self, id: SequenceId) -> String {
        self.document
            .as_ref()
            .and_then(|d| d.project().sequence(id))
            .map_or_else(|| "The timeline".into(), |s| s.name.clone())
    }

    /// A new, empty timeline the size and rate of the open one ("Timeline
    /// 2"), or (with `copy`) a copy of the open one; it opens, and its tab
    /// is renamed by a double-click like any other.
    pub(super) fn new_timeline(&mut self, copy: bool) -> diag::Result<()> {
        self.with_document(|d| {
            let p = d.project();
            let Some(seq) = p.active() else { return Ok(()) };
            let command = match copy {
                false => Command::AddSequence {
                    name: p.new_sequence_name(),
                    settings: seq.settings,
                },
                true => Command::DuplicateSequence {
                    sequence: seq.id,
                    name: p.free_sequence_name(&format!("{} copy", seq.name)),
                },
            };
            d.apply(command)
        })?;
        self.follow_open_timeline();
        if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
            self.status = format!(
                "{} is open. Double-click its tab to rename it; the other timelines are beside it.",
                seq.name
            );
        }
        Ok(())
    }

    /// Moves the selected clips into a timeline of their own, one clip
    /// playing it in their place.
    pub(super) fn nest_selection(&mut self) -> diag::Result<()> {
        if self.selected.is_empty() {
            self.status = "Select the clips to nest first.".into();
            return Ok(());
        }
        let clips = self.selected.clone();
        let mut made = None;
        self.with_document(|d| {
            let p = d.project();
            let sequence = p.active_sequence;
            let name = p.free_sequence_name("Nested");
            d.apply(Command::NestClips {
                sequence,
                clips,
                name: name.clone(),
            })?;
            made = Some(name);
            Ok(())
        })?;
        self.selected.clear();
        if let Some(name) = made {
            self.status = format!(
                "The clips are in {name} now; double-click its clip to open it, or its tab to rename it."
            );
        }
        Ok(())
    }

    /// Lays an adjustment layer at `at` on the video track `track` or the
    /// first free one above it, and selects it for the inspector.
    pub(super) fn add_adjustment(&mut self, at: Time, track: Option<TrackId>) -> diag::Result<()> {
        let at = self.snapped(at);
        self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::AddAdjustment {
                sequence,
                at,
                length: project::timelines::ADJUSTMENT_LENGTH,
                track,
            })
        })?;
        let added = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|seq| {
                seq.video_tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| c.is_adjustment() && c.start == at)
                    .map(|c| c.id)
                    .max()
            });
        if let Some(clip) = added {
            self.selected = vec![clip];
        }
        self.status =
            "Adjustment layer added: its colour and opacity in the inspector change everything below it."
                .into();
        Ok(())
    }

    /// Places the timeline `nested` in the open one: at `at` on the video
    /// track `track`, or at its end.
    pub(super) fn place_timeline(
        &mut self,
        nested: SequenceId,
        at: Option<f64>,
        track: Option<TrackId>,
    ) -> diag::Result<()> {
        let at = at.map(|at| self.snapped(seconds(at)));
        self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::PlaceSequence {
                sequence,
                nested,
                at,
                track,
            })
        })
    }

    /// Opens the timeline a nested clip plays.
    pub(super) fn open_nested(&mut self, clip: ClipId) {
        let nested = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|seq| {
                seq.tracks()
                    .flat_map(|t| t.clips.iter())
                    .find(|c| c.id == clip)
                    .and_then(project::Clip::nested)
            });
        if let Some(id) = nested {
            self.open_timeline(id);
        }
    }

    /// "Fit to the nested timeline": a nested clip longer than what its
    /// timeline has left to play is trimmed to end where that ends (on a
    /// frame, never past it), as one undo step. Never done by itself: a
    /// timeline that gets shorter leaves the clips playing it as they are.
    pub(super) fn fit_nested(&mut self, clip: ClipId) {
        let result = self.with_document(|d| {
            let p = d.project();
            let Some(seq) = p.active() else { return Ok(()) };
            let Some(c) = seq.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == clip) else {
                return Ok(());
            };
            let Some(end) = crate::timeline_panel::nested_end(p, c) else {
                return Ok(());
            };
            d.apply(project::Command::Trim {
                sequence: seq.id,
                clip,
                edge: project::Edge::End,
                to: end,
            })
        });
        match result {
            Ok(()) => self.status = "The clip now ends where its timeline does.".into(),
            Err(err) => self.errors.push(&err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_for_every_timeline_says_which_can_be_placed() {
        let p = Project::new("T", project::SequenceSettings::GAME_CLIP);
        let p = Command::AddSequence {
            name: "Short".into(),
            settings: project::SequenceSettings::VERTICAL_SHORT,
        }
        .apply(&p)
        .unwrap();
        let shown = tabs(&p);
        assert_eq!(shown.len(), 2);
        assert_eq!((shown[1].name.as_str(), shown[1].open), ("Short", true));
        assert!(shown[1].cannot_place.is_some(), "the open one");
        assert!(shown[0].cannot_place.is_none());
        assert!(shown[1].info.starts_with("1080×1920"), "{}", shown[1].info);
    }
}
