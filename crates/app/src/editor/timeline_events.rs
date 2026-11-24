//! What was done on the timeline (`ui_kit::TimelineEvent`), turned into
//! commands on the open timeline: most of them one command each, built from
//! the open timeline at a time on its frames and applied by `apply_on_open`,
//! which reports what went wrong.

use super::*;
use project::Sequence;

impl Editor {
    pub(super) fn timeline_event(&mut self, event: ui_kit::TimelineEvent) {
        use ui_kit::TimelineEvent as E;
        match event {
            E::Seek(t) => self.seek(t),
            E::MoveNeedle(t) => self.apply_on_open(|seq| {
                Some(Command::SetEditNeedle {
                    sequence: seq.id,
                    at: on_frame(seq, t),
                })
            }),
            E::Line { owner, kind, points } => self.line_event(owner, kind, &points),
            E::TrackRole { track, role } => self.role_event(track, role),
            E::Select { clip, add } => {
                self.selected_label = None;
                timeline_panel::select(&mut self.selected, project::ClipId(clip), add);
            }
            E::SelectLabel(label) => self.select_label(label),
            E::MoveLabel { label, at } => self.move_label(label, at),
            E::ColourLabel { label, colour } => self.colour_label(label, colour),
            E::RemoveLabel(label) => self.remove_label(project::LabelId(label)),
            E::LabelRegion => {
                if let Err(err) = self.label_region() {
                    self.errors.push(&err);
                }
            }
            E::PrerenderRegion => self.prerender_region(),
            E::CropBars { clip } => self.find_bars(project::ClipId(clip)),
            E::ClearSelection => self.selected.clear(),
            E::RegionCut(cut) => {
                if let Err(err) = self.cut_region(cut) {
                    self.errors.push(&err);
                }
            }
            E::CloseGap(gap) => {
                if let Err(err) = self.close_gap(gap) {
                    self.errors.push(&err);
                }
            }
            E::Move {
                clip,
                by,
                track,
                new_track,
            } => self.move_event(project::ClipId(clip), by, track, new_track),
            E::Trim { clip, edge, to } | E::RippleTrim { clip, edge, to } => {
                let ripple = matches!(event, E::RippleTrim { .. });
                self.apply_on_open(|seq| {
                    let (sequence, clip) = (seq.id, project::ClipId(clip));
                    let (edge, to) = (project_edge(edge), on_frame(seq, to));
                    Some(if ripple {
                        Command::RippleTrim {
                            sequence,
                            clip,
                            edge,
                            to,
                        }
                    } else {
                        Command::Trim {
                            sequence,
                            clip,
                            edge,
                            to,
                        }
                    })
                });
            }
            E::TrackMix {
                track,
                muted,
                solo,
                gain_db,
            } => {
                self.apply_on_open(|seq| {
                    Some(Command::SetTrackMix {
                        sequence: seq.id,
                        track: project::TrackId(track),
                        muted,
                        solo,
                        gain_db,
                    })
                });
                // Heard within a block while playing: `follow_playback` sends it.
            }
            E::Roll { clip, to } => self.apply_on_open(|seq| {
                Some(Command::Roll {
                    sequence: seq.id,
                    clip: project::ClipId(clip),
                    to: on_frame(seq, to),
                })
            }),
            E::Slip { clip, by } | E::Slide { clip, by } => {
                let slide = matches!(event, E::Slide { .. });
                self.apply_on_open(|seq| {
                    // Whole frames only, so cuts stay on frames.
                    let step = on_frame(seq, by.abs());
                    if step == Time::ZERO {
                        return None;
                    }
                    let by = if by < 0.0 { Time::ZERO - step } else { step };
                    let (sequence, clip) = (seq.id, project::ClipId(clip));
                    Some(if slide {
                        Command::Slide { sequence, clip, by }
                    } else {
                        Command::Slip { sequence, clip, by }
                    })
                });
            }
            E::SetRegion(range) => {
                if let Err(err) = self.set_region(range.map(|(a, b)| (seconds(a), seconds(b)))) {
                    self.errors.push(&err);
                }
            }
            E::DropMedia {
                media,
                at,
                insert,
                track,
            } => {
                let (media, track) = (project::MediaId(media), track.map(project::TrackId));
                if insert {
                    // Ctrl held: everything from there on moves later.
                    self.apply_on_open(|seq| {
                        Some(Command::DropMedia {
                            sequence: seq.id,
                            media,
                            at: on_frame(seq, at),
                            track,
                            insert,
                        })
                    });
                } else {
                    let at = self
                        .document
                        .as_ref()
                        .and_then(|d| d.project().active())
                        .map_or(Time::ZERO, |seq| on_frame(seq, at));
                    self.lay_media(media, at, track, None);
                }
            }
            E::DropEffect { effect, clip } => {
                if let Some(kind) = crate::effects_drawer::by_index(effect) {
                    self.add_effect(kind, project::ClipId(clip));
                }
            }
            E::DropTitle { title, at, track } => {
                if let Some(style) = crate::effects_drawer::title_by_index(title)
                    && let Err(err) = self.add_title(style, seconds(at), track.map(project::TrackId))
                {
                    self.errors.push(&err);
                }
            }
            E::DropTransition { transition, clip } => {
                if let Some(kind) = crate::effects_drawer::transition_by_index(transition) {
                    self.add_transition(kind, project::ClipId(clip));
                }
            }
            E::OpenNested { clip } => self.open_nested(project::ClipId(clip)),
            E::FitNested { clip } => self.fit_nested(project::ClipId(clip)),
            E::AddTitleAt { at, track } => {
                let style = project::TitleStyle::LowerThird;
                if let Err(err) = self.add_title(style, seconds(at), track.map(project::TrackId)) {
                    self.errors.push(&err);
                }
            }
            E::AddAdjustment { at, track } => {
                if let Err(err) = self.add_adjustment(seconds(at), track.map(project::TrackId)) {
                    self.errors.push(&err);
                }
            }
            E::NestSelected => {
                if let Err(err) = self.nest_selection() {
                    self.errors.push(&err);
                }
            }
            E::SyncBySound => self.start_sync(),
            E::Unlink { clip } => self.unlink(project::ClipId(clip)),
            E::LinkSelected => self.link_selected(),
            E::DropTimeline { timeline, at, track } => {
                let result = self.place_timeline(
                    project::SequenceId(timeline),
                    Some(at),
                    track.map(project::TrackId),
                );
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::AddTrack { audio } => {
                let kind = if audio {
                    project::TrackKind::Audio
                } else {
                    project::TrackKind::Video
                };
                if let Err(err) = self.add_track(kind) {
                    self.errors.push(&err);
                }
            }
            E::LockTrack { .. }
            | E::SyncTrack { .. }
            | E::ColourTrack { .. }
            | E::DuplicateTrack { .. }
            | E::MoveTrack { .. }
            | E::InsertTrack { .. }
            | E::ResetTrackMix { .. }
            | E::SoloOnly { .. }
            | E::RemoveEmptyTracks => {
                self.apply_on_open(|seq| timeline_panel::track_option_command(seq, &event));
            }
            E::TrackPan { .. } => {
                self.apply_on_open(|seq| timeline_panel::track_option_command(seq, &event));
                self.play_on_with_new_mix();
            }
            E::SelectTrackClips { track, from } => {
                if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                    self.selected = timeline_panel::track_clips(seq, project::TrackId(track), from);
                    self.selected_label = None;
                }
            }
            E::CopyTrackEffects { track } => self.copy_track_effects(project::TrackId(track)),
            E::PasteTrackEffects { track } => self.paste_track_effects(project::TrackId(track)),
            E::RecordOnTrack { track } => {
                if let Err(err) = self.record_voice_over(Some(project::TrackId(track))) {
                    self.errors.push(&err);
                }
            }
            E::OpenTrackEffects { track } => {
                self.audio_page.track = Some(project::TrackId(track));
                self.page = ui_kit::Page::Audio;
            }
            E::RemoveTrack { .. } | E::HideTrack { .. } | E::RenameTrack { .. } => {
                self.apply_on_open(|seq| timeline_panel::track_command(seq.id, event));
            }
            E::ClipVolume { clip, gain_db } => {
                self.apply_on_open(|seq| {
                    timeline_panel::clip_volume_command(seq, project::ClipId(clip), gain_db)
                });
                self.play_on_with_new_mix();
            }
            E::ClipFade { clip, sound, fades } => {
                self.apply_on_open(|seq| {
                    timeline_panel::clip_fade_command(seq, project::ClipId(clip), sound, fades)
                });
                if sound {
                    self.play_on_with_new_mix();
                }
            }
            E::SetSpeed { clip, num, den } => match project::Speed::new(num, den) {
                Ok(speed) => self.apply_on_open(|seq| {
                    Some(Command::SetSpeed {
                        sequence: seq.id,
                        clip: project::ClipId(clip),
                        speed,
                    })
                }),
                Err(err) => self.errors.push(&err),
            },
            E::Keyframes { clip, marks, change } => {
                self.apply_on_open(|seq| timeline_panel::keyframes_command(seq, clip, &marks, change));
            }
            E::EditLabel(id) => {
                self.label_editor = self
                    .document
                    .as_ref()
                    .and_then(|d| d.project().active())
                    .and_then(|s| s.labels.iter().find(|l| l.id.0 == id))
                    .map(crate::label_editor::LabelEditor::new);
            }
            E::AddLabel(at) => {
                if let Err(err) = self.add_label(seconds(at)) {
                    self.errors.push(&err);
                }
            }
            // A split where the pointer is (`clip` or every clip there), or
            // the blade on one clip: the same command.
            E::SplitAt { at, clip } => self.split_on_open(at, clip.map(project::ClipId)),
            E::Blade { clip, at } => self.split_on_open(at, Some(project::ClipId(clip))),
        }
    }

    /// Keeps the effects of `track` to paste on another track.
    fn copy_track_effects(&mut self, track: project::TrackId) {
        let sound = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|seq| seq.track(track))
            .map(|t| (t.name.clone(), t.sound.clone()));
        if let Some((name, sound)) = sound {
            self.status = match sound.effects_on() {
                true => format!("The effects of {name} are copied: right-click another track to paste them"),
                false => format!("{name} has no effects on: pasting them turns another track's effects off"),
            };
            self.track_effects = Some(sound);
        }
    }

    /// The copied effects replace those of `track`: one undo step.
    fn paste_track_effects(&mut self, track: project::TrackId) {
        let Some(copied) = self.track_effects.clone() else {
            return;
        };
        self.apply_on_open(|seq| {
            let t = seq.track(track)?;
            let sound = t.sound.with_effects_of(&copied);
            (sound != t.sound).then(|| Command::SetTrackSound {
                sequence: seq.id,
                track,
                sound: Box::new(sound),
            })
        });
        self.play_on_with_new_mix();
    }

    /// Builds one command from the open timeline (`None`: nothing to do)
    /// and applies it, reporting what went wrong.
    pub(super) fn apply_on_open(&mut self, f: impl FnOnce(&Sequence) -> Option<Command>) {
        let result = self.with_document(|d| match d.project().active().and_then(f) {
            Some(command) => d.apply(command),
            None => Ok(()),
        });
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }

    /// Splits `clip` (with its linked clips), or every clip there, at `at`
    /// seconds on the open timeline's frames.
    fn split_on_open(&mut self, at: f64, clip: Option<project::ClipId>) {
        self.apply_on_open(|seq| {
            Some(Command::Split {
                sequence: seq.id,
                at: on_frame(seq, at),
                clip,
            })
        });
    }

    /// Moves `clip` by `by` seconds, on frames: with the other selected
    /// clips when it is one of them, else with its linked clips; to another
    /// track, or past the last one to a new track, saying where it went.
    fn move_event(&mut self, clip: project::ClipId, by: f64, track: Option<u64>, new_track: bool) {
        let mut says = None;
        let selected = self.selected.clone();
        let result = self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let together = timeline_panel::moves_together(seq, &selected, clip);
            let Some(start) = seq
                .tracks()
                .flat_map(|t| t.clips.iter())
                .find(|c| c.id == clip)
                .map(|c| c.start)
            else {
                return Ok(());
            };
            let target = seq.settings.frame_rate.snap(start + seconds(by)).max(Time::ZERO);
            let (sequence, by) = (seq.id, target - start);
            if together {
                // One of several selected clips: they all move, one step.
                return d.apply(Command::MoveClips {
                    sequence,
                    clips: selected,
                    by,
                });
            }
            if track.is_none() && !new_track {
                return d.apply(Command::MoveLinked { sequence, clip, by });
            }
            // To another track, or past the last one to a new
            // track: linked sound finds a free place by itself.
            let before = seq.clone();
            d.apply(Command::MoveBetweenTracks {
                sequence,
                clip,
                track: track.map(project::TrackId),
                by,
            })?;
            says = d
                .project()
                .active()
                .map(|after| timeline_panel::moved_says(&before, after, clip));
            Ok(())
        });
        match result {
            Ok(()) => self.status = says.unwrap_or_default(),
            Err(err) => self.errors.push(&err),
        }
    }
}

/// `s` seconds (as the timeline widget gives them) on the nearest frame of
/// `seq`.
fn on_frame(seq: &Sequence, s: f64) -> Time {
    seq.settings.frame_rate.snap(seconds(s))
}

/// The widget's clip edge as the project names it.
fn project_edge(edge: ui_kit::ClipEdge) -> project::Edge {
    match edge {
        ui_kit::ClipEdge::Start => project::Edge::Start,
        ui_kit::ClipEdge::End => project::Edge::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widget's seconds land on the open timeline's nearest frame, as
    /// every timeline edit places them.
    #[test]
    fn seconds_land_on_the_nearest_frame() {
        let p = project::Project::new("T", project::SequenceSettings::GAME_CLIP);
        let seq = p.active().expect("a timeline");
        let rate = seq.settings.frame_rate;
        let frame = rate.frame_start(1).as_secs_f64();
        assert_eq!(on_frame(seq, frame * 0.4), Time::ZERO);
        assert_eq!(on_frame(seq, frame * 0.6), rate.frame_start(1));
        assert_eq!(on_frame(seq, frame * 10.0), rate.frame_start(10));
    }

    #[test]
    fn each_clip_edge_is_the_same_edge_of_the_clip() {
        assert_eq!(project_edge(ui_kit::ClipEdge::Start), project::Edge::Start);
        assert_eq!(project_edge(ui_kit::ClipEdge::End), project::Edge::End);
    }
}
