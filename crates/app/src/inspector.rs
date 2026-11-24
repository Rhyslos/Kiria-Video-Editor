//! The Edit page's inspector: the selected clip's Transform, Crop, Quick
//! color, Effects, Speed and Audio, on the right. A clip on an audio track shows
//! Speed and Audio alone; a picture's Audio is the sound linked to it.
//!
//! A value being dragged is shown in the viewer at once ([`Inspector::shown`]
//! is the project as it would be) and becomes one undo step when the drag
//! ends, so a slider swept from one end to the other is undone in one go.
//!
//! The picture can also be taken hold of in the viewer itself
//! (`picture_handles`). That change goes through the same look in waiting:
//! [`Inspector::subject`] says which clip the panel is about and how it looks
//! now, [`Inspector::changed_outside`] takes what the hand did and
//! [`Inspector::let_go`] finishes it, so the panel's numbers follow the
//! picture, the picture follows the numbers, and either way it is one undo step.
//!
//! Every number of the picture can change over time: the diamond at the end
//! of its row adds a keyframe at the playhead (or removes the one there),
//! the arrows beside it move the playhead to the previous or next one, and
//! a right-click on a filled diamond says how the value goes on from it.
//! The numbers shown are the clip's at the playhead, and a change to a
//! number that has keyframes is a keyframe at the playhead
//! (`Command::SetLookAt`), so the hand on the picture keys it too.

use project::animate::KeyframeChange;
use project::{ClipId, Command, Ease, Look, Project, Rate, Sound, Time, TrackKind};
use ui_kit::Theme;
use ui_kit::inspector::Touched;
use ui_kit::keyframe::KeyState;

use crate::pending::Pending;

mod effects;
mod keyframes;
mod look;
mod picture;
mod sound;
mod speed;
mod title;
mod transition;

pub use look::{cropped_to_fit, fitted};

use keyframes::KeyTable;
use look::Values;
use picture::{Asks, picture_sections};
use sound::{sound_clips, stored_sounds};

/// Whose look is being set, and at which moment of the timeline (where
/// the change began).
#[derive(Debug, Clone, Copy, PartialEq)]
struct LookAt {
    clip: ClipId,
    at: Time,
}

impl LookAt {
    fn command(self, sequence: project::SequenceId, look: Look) -> Command {
        Command::SetLookAt {
            sequence,
            clip: self.clip,
            at: self.at,
            look,
        }
    }

    /// The look the project holds there; `None` when the clip is gone.
    fn stored(&self, project: &Project) -> Option<Look> {
        let rate = project.active()?.settings.frame_rate;
        let clip = clip_in(project, self.clip)?;
        Some(clip.look_at(clip.moment(self.at, rate)))
    }
}

#[derive(Debug, Default)]
pub struct Inspector {
    /// A look being set and not yet an undo step.
    pending: Pending<LookAt, Look>,
    /// Sound being set and not yet an undo step: the sound clips and each
    /// one's new settings, in the same order (a picture's linked sound
    /// clips change together).
    pending_sound: Pending<Vec<ClipId>, Vec<Sound>>,
    /// A picture's effects with a setting being dragged, not yet an undo step.
    pending_effects: Pending<ClipId, project::Effects>,
    /// A title's words being typed or size dragged, not yet an undo step.
    pending_title: Pending<ClipId, project::Title>,
    /// A transition's length being dragged: the change as it is now.
    transition_dragged: Option<Command>,
    /// A title just added: its words' field takes the keys the next time
    /// its Title section is drawn, its words chosen to type over.
    focus_words: Option<ClipId>,
    /// A sound clip's volume (thousandths of a decibel) being dragged on
    /// the timeline, by its level line: shown as it moves. The timeline
    /// makes the undo step when it is let go, not the panel.
    pub volume_from_timeline: Option<(ClipId, i32)>,
    /// A hand outside the panel (on the picture in the viewer) is still on
    /// the look being set: the panel leaves finishing it to that hand.
    held: bool,
    /// Whether the selected clip can be moved, resized, cropped and turned on the picture in
    /// the viewer (its outline and handles are shown). Off unless asked for,
    /// so selecting a clip doesn't put a frame around it.
    pub on_picture: bool,
    /// The tooltip of the switch for that: what it does, and its keys.
    pub on_picture_tip: String,
    /// Where the playhead was asked to go (an arrow beside a keyframe
    /// diamond). The page takes it.
    pub seek: Option<Time>,
    /// "Follow it from here" on an effect's box: the clip and the effect.
    /// The page takes it and starts following.
    pub follow: Option<(ClipId, project::EffectId)>,
    /// "Analyse the clip" on a Stabilise effect: the clip and the effect.
    pub steady: Option<(ClipId, project::EffectId)>,
    /// "Black bars" in Crop: the clip whose bars to find and cut away.
    /// The page takes it and looks on a worker.
    pub find_bars: Option<ClipId>,
}

/// The clip the inspector is about, for what changes its look from outside
/// the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject {
    pub clip: ClipId,
    /// The moment of the timeline its look is shown at: the playhead,
    /// brought onto the clip.
    pub at: Time,
    /// Its look as the panel shows it: at that moment, with the change
    /// being made.
    pub look: Look,
    /// Its picture's size as shown, and the timeline's frame, in pixels.
    pub picture: (u32, u32),
    pub frame: (u32, u32),
    /// Where on the timeline it is, and whether its track is hidden.
    pub start: Time,
    pub end: Time,
    pub hidden: bool,
}

impl Subject {
    /// Whether the viewer shows this clip's place in the frame at `at`: the
    /// playhead is on it and its track isn't hidden.
    pub fn on_show_at(&self, at: Time) -> bool {
        !self.hidden && self.start <= at && at < self.end
    }
}

impl Inspector {
    /// The project with the change being made, for the viewer to show.
    /// `None` when nothing is being changed.
    pub fn shown(&self, project: &Project) -> Option<Project> {
        let sequence = project.active_sequence;
        let looked = self
            .pending
            .get()
            .and_then(|(key, look)| key.command(sequence, *look).apply(project).ok());
        let looked = match self.pending_effects.get() {
            Some((clip, effects)) => Command::SetEffects {
                sequence,
                clip: *clip,
                effects: Box::new(effects.clone()),
            }
            .apply(looked.as_ref().unwrap_or(project))
            .ok()
            .or(looked),
            None => looked,
        };
        let looked = match self.pending_title.get() {
            Some((clip, title)) => Command::SetTitle {
                sequence,
                clip: *clip,
                title: Box::new(title.clone()),
            }
            .apply(looked.as_ref().unwrap_or(project))
            .ok()
            .or(looked),
            None => looked,
        };
        let looked = match &self.transition_dragged {
            Some(command) => command.apply(looked.as_ref().unwrap_or(project)).ok().or(looked),
            None => looked,
        };
        if self.pending_sound.get().is_none() {
            return looked;
        }
        Command::SetSound {
            sequence,
            sounds: self.sounds_being_set(),
        }
        .apply(looked.as_ref().unwrap_or(project))
        .ok()
        .or(looked)
    }

    /// Sound being set and not yet an undo step, for the timeline to draw.
    pub fn sounds_being_set(&self) -> Vec<(ClipId, Sound)> {
        self.pending_sound.get().map_or_else(Vec::new, |(clips, sounds)| {
            clips.iter().copied().zip(sounds.iter().copied()).collect()
        })
    }

    /// The clip the panel is about (see [`shown_clip`]) with its look as it
    /// is shown now, at `playhead`. `None` when no selected clip has a picture.
    pub fn subject(&self, project: &Project, selected: &[ClipId], playhead: Time) -> Option<Subject> {
        let (seq, track, clip) = shown_clip(project, selected)?;
        if track.kind != TrackKind::Video {
            return None;
        }
        let picture = picture_size(project, clip)?;
        let at = clip.moment(playhead, seq.settings.frame_rate);
        let look = self.look_being_set(clip.id).unwrap_or_else(|| clip.look_at(at));
        Some(Subject {
            clip: clip.id,
            at,
            look,
            picture,
            frame: (seq.settings.width, seq.settings.height),
            start: clip.start,
            end: clip.end(),
            hidden: track.hidden,
        })
    }

    /// A change to `clip`'s look as seen at `at` on the timeline, made
    /// outside the panel, on the picture itself. It is shown at once, as the
    /// panel's own changes are. While `busy` the hand is still on it and
    /// more will follow; once it is not, the change is finished: the undo
    /// step comes back.
    pub fn changed_outside(
        &mut self,
        project: &Project,
        clip: ClipId,
        at: Time,
        look: Look,
        busy: bool,
    ) -> Option<Command> {
        // A change in waiting on another clip is not lost.
        let other = match self.pending.get() {
            Some((waiting, _)) if waiting.clip != clip => self.finish(project),
            _ => None,
        };
        // A change keeps the moment it began at.
        let at = self.pending.get().map_or(at, |(key, _)| key.at);
        self.pending.set(LookAt { clip, at }, look.within_limits());
        self.held = busy;
        if busy || other.is_some() {
            // The other clip's step goes first; this one follows when the
            // panel next looks (the hand is off it by then).
            return other;
        }
        self.finish(project)
    }

    /// The hand outside the panel is off the picture: what it was changing
    /// is finished. Returns the undo step, if anything changed.
    pub fn let_go(&mut self, project: &Project) -> Option<Command> {
        self.finish(project)
    }

    /// Ends the change in waiting: an undo step unless the look is the one
    /// the clip already has there.
    fn finish(&mut self, project: &Project) -> Option<Command> {
        self.held = false;
        let (key, look) = self.pending.finish(|key| key.stored(project))?;
        Some(key.command(project.active_sequence, look))
    }

    /// The look being set on `clip`, if it is the one being changed.
    fn look_being_set(&self, clip: ClipId) -> Option<Look> {
        self.pending
            .get()
            .filter(|(key, _)| key.clip == clip)
            .map(|(_, look)| *look)
    }

    /// Draws the inspector for the first of `selected` that has a picture,
    /// its numbers as they are at `playhead`. Returns the changes that are
    /// finished: each is one undo step.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
    ) -> Vec<Command> {
        let target = shown_clip(project, selected);
        // Set again below while a length is being dragged.
        self.transition_dragged = None;
        let mut commands = self.finish_left_behind(project, target);
        // Nothing selected: the page shows the timeline here instead.
        let Some((seq, track, clip)) = target else {
            return commands;
        };
        let at = Shown::new(project, seq, track, clip, playhead);
        header(ui, theme, &at, selected.len());
        commands.extend(self.title_step(ui, theme, &at));
        let (look, blend) = self.look_step(ui, theme, &at);
        commands.extend(blend);
        commands.extend(self.effects_step(ui, theme, &at));
        // A picture's transition, or a sound clip's crossfade.
        let asked = transition::section(ui, theme, at.sequence, track, clip);
        commands.extend(asked.command);
        self.transition_dragged = asked.dragging;
        commands.extend(self.speed_step(ui, theme, &at));
        commands.extend(self.audio_section(ui, theme, seq, clip));
        commands.extend(self.finish_look(&at, look));
        commands
    }

    /// Changes left hanging on a clip that is no longer the one shown
    /// (another was selected mid-drag, or none) are finished first: the
    /// look, the effects, a title's words and the sound, each an undo step
    /// only if it changes something.
    fn finish_left_behind(&mut self, project: &Project, target: Option<ShownClip<'_>>) -> Vec<Command> {
        let mut commands = Vec::new();
        let sequence = project.active_sequence;
        let showing = target.map(|(_, _, clip)| clip.id);
        if self
            .pending
            .get()
            .is_some_and(|(key, _)| Some(key.clip) != showing)
        {
            self.held = false;
            if let Some((key, look)) = self.pending.finish(|key| key.stored(project)) {
                commands.push(key.command(sequence, look));
            }
        }
        if let Some((clip, effects)) = self.pending_effects.finish_on_change(
            |clip| Some(*clip) == showing,
            |clip| clip_in(project, *clip).map(|c| c.effects.clone()),
        ) {
            commands.push(Command::SetEffects {
                sequence,
                clip,
                effects: Box::new(effects),
            });
        }
        if let Some((clip, title)) = self.pending_title.finish_on_change(
            |clip| Some(*clip) == showing,
            |clip| clip_in(project, *clip).and_then(|c| c.title.as_deref().cloned()),
        ) {
            commands.push(Command::SetTitle {
                sequence,
                clip,
                title: Box::new(title),
            });
        }
        let hearing: Vec<ClipId> = target
            .map(|(seq, _, clip)| sound_clips(seq, clip).iter().map(|c| c.id).collect())
            .unwrap_or_default();
        if let Some((clips, sounds)) = self.pending_sound.finish_on_change(
            |clips| clips.iter().all(|id| hearing.contains(id)),
            |clips| stored_sounds(project, clips),
        ) {
            commands.push(Command::SetSound {
                sequence,
                sounds: clips.into_iter().zip(sounds).collect(),
            });
        }
        commands
    }

    /// Gives the words' field of title `clip` the keys the next time its
    /// Title section is drawn (a title just added).
    pub fn focus_title_words(&mut self, clip: ClipId) {
        self.focus_words = Some(clip);
    }

    /// The Title section, on a title. Words typed into another title are
    /// finished before it, by [`Inspector::finish_left_behind`].
    fn title_step(&mut self, ui: &mut egui::Ui, theme: &Theme, at: &Shown) -> Vec<Command> {
        let (clip, sequence) = (at.clip, at.sequence);
        let mut commands = Vec::new();
        if let Some(stored) = clip.title.as_deref().filter(|_| clip.is_title()) {
            let shown = self.pending_title.value_for(&clip.id).unwrap_or(stored).clone();
            let focus = self.focus_words.take_if(|c| *c == clip.id).is_some();
            let asked = title::section(ui, theme, sequence, clip.id, stored, &shown, focus);
            match asked.changing {
                Some(title) => self.pending_title.set(clip.id, title),
                None => self.pending_title.clear(),
            }
            commands.extend(asked.command);
        }
        commands
    }

    /// The picture sections, for a clip on a video track: the look's
    /// numbers as touched, kept for [`Inspector::finish_look`], and a blend
    /// mode picked, one undo step at once.
    fn look_step(&mut self, ui: &mut egui::Ui, theme: &Theme, at: &Shown) -> (LookEdit, Vec<Command>) {
        let (clip, track, frame) = (at.clip, at.track, at.frame);
        let adjustment = clip.is_adjustment();
        let before = self
            .look_being_set(clip.id)
            .unwrap_or_else(|| clip.look_at(at.moment));
        let mut edit = LookEdit {
            before,
            values: Values::of(&before, frame),
            touched: Touched::default(),
            asks: Asks::default(),
            keys: KeyTable::at(clip, at.playhead, at.rate, at.on_clip),
        };
        let mut blend = Vec::new();
        if track.kind == TrackKind::Video {
            let on_picture = (&mut self.on_picture, self.on_picture_tip.as_str());
            if adjustment {
                ui.label(
                    egui::RichText::new(
                        "Its colour, at its opacity, changes everything below it in the picture while it lasts.",
                    )
                    .color(theme.muted)
                    .size(12.0),
                );
            }
            edit.asks = picture_sections(
                ui,
                theme,
                &before,
                frame,
                at.picture,
                on_picture,
                adjustment,
                clip.mode,
                &edit.keys,
                &mut edit.values,
                &mut edit.touched,
            );
            if let Some(mode) = edit.asks.mode {
                blend.push(Command::SetBlendMode {
                    sequence: at.sequence,
                    clip: clip.id,
                    mode,
                });
            }
            if edit.asks.find_bars {
                self.find_bars = Some(clip.id);
            }
            if let Some(size) = edit.asks.timeline_to
                && let Some(seq) = at.project.sequence(at.sequence)
            {
                blend.push(timeline_to(seq, at, &before, size));
            }
        }
        (edit, blend)
    }

    /// The Effects section, on a picture that plays something. A setting
    /// being dragged is kept in waiting; clicks are steps at once.
    fn effects_step(&mut self, ui: &mut egui::Ui, theme: &Theme, at: &Shown) -> Vec<Command> {
        let (project, clip, sequence) = (at.project, at.clip, at.sequence);
        let mut commands = Vec::new();
        if at.track.kind != TrackKind::Video || clip.is_adjustment() {
            return commands;
        }
        let shown = self
            .pending_effects
            .value_for(&clip.id)
            .unwrap_or(&clip.effects)
            .clone();
        let place = effects::At {
            clip,
            playhead: at.playhead,
            rate: at.rate,
        };
        let asked = effects::section(ui, theme, sequence, place, &shown);
        if let Some(to) = asked.seek {
            self.seek = Some(to);
        }
        if let Some(effect) = asked.follow {
            self.follow = Some((clip.id, effect));
        }
        if let Some(effect) = asked.steady {
            self.steady = Some((clip.id, effect));
        }
        if !asked.commands.is_empty() {
            self.pending_effects.clear();
            commands.extend(asked.commands);
        } else if let Some(effects) = asked.dragging {
            self.pending_effects.set(clip.id, effects);
        }
        if let Some((id, effects)) = self
            .pending_effects
            .finish_if_released(asked.busy, |id| clip_in(project, *id).map(|c| c.effects.clone()))
        {
            commands.push(Command::SetEffects {
                sequence,
                clip: id,
                effects: Box::new(effects),
            });
        }
        commands
    }

    /// The Speed section, for a clip that plays something.
    fn speed_step(&mut self, ui: &mut egui::Ui, theme: &Theme, at: &Shown) -> Vec<Command> {
        let clip = at.clip;
        let speed = match clip.is_adjustment() || clip.is_title() {
            // It plays nothing, so it has no speed.
            true => speed::Asked::default(),
            false => speed::section(
                ui,
                theme,
                &speed::Subject {
                    sequence: at.sequence,
                    clip,
                    kind: at.track.kind,
                    rate: at.rate,
                    recorded: at.media.and_then(|m| m.info.video.as_ref()).map(|v| v.frame_rate),
                    playhead: at.playhead,
                },
            ),
        };
        self.seek = speed.seek.or(self.seek);
        speed.commands
    }

    /// The look's numbers touched this frame: shown at once; an undo step
    /// once the hand is off it, be that a hand on the panel or one on the
    /// picture in the viewer. Then what Reset and the keyframe controls
    /// asked for.
    fn finish_look(&mut self, at: &Shown, edit: LookEdit) -> Vec<Command> {
        let (project, clip, sequence) = (at.project, at.clip, at.sequence);
        let mut commands = Vec::new();
        if edit.touched.changed {
            let begun = self.pending.get().map_or(at.moment, |(key, _)| key.at);
            self.pending.set(
                LookAt {
                    clip: clip.id,
                    at: begun,
                },
                edit.values.look(at.frame, &edit.before),
            );
        }
        if let Some((key, look)) = self
            .pending
            .finish_if_released(edit.touched.busy || self.held, |key| key.stored(project))
        {
            commands.push(key.command(sequence, look));
        }
        commands.extend(self.answer(&edit.asks, &edit.keys, sequence, clip.id, at.playhead, at.rate));
        commands
    }

    /// What the sections' Reset and keyframe controls asked for: each a
    /// command (one undo step), or a place for the playhead.
    fn answer(
        &mut self,
        asks: &Asks,
        keys: &KeyTable,
        sequence: project::SequenceId,
        clip: ClipId,
        playhead: Time,
        rate: Rate,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        for fields in &asks.resets {
            commands.push(Command::ResetLook {
                sequence,
                clip,
                fields: fields.to_vec(),
            });
        }
        for (group, clicks) in &asks.keys {
            let Some(here) = keys.of(group) else { continue };
            let keyframe = |change| Command::Keyframe {
                sequence,
                clip,
                at: playhead,
                fields: group.to_vec(),
                change,
            };
            if clicks.toggle {
                commands.push(keyframe(match here.state {
                    KeyState::At => KeyframeChange::Remove,
                    _ => KeyframeChange::Add,
                }));
            }
            if let Some(ease) = clicks.ease.and_then(|i| Ease::ALL.get(i)) {
                commands.push(keyframe(KeyframeChange::Ease(*ease)));
            }
            let jump = match (clicks.previous, clicks.next) {
                (true, _) => here.before,
                (_, true) => here.after,
                _ => None,
            };
            if let Some(frame) = jump {
                self.seek = Some(rate.frame_start(frame));
            }
        }
        commands
    }
}

/// The clip the panel is about, with its timeline and track.
type ShownClip<'a> = (&'a project::Sequence, &'a project::Track, &'a project::Clip);

/// What every section of one frame of the panel works from: the clip
/// shown, where it is and the moment its numbers are taken at.
struct Shown<'a> {
    project: &'a Project,
    track: &'a project::Track,
    clip: &'a project::Clip,
    sequence: project::SequenceId,
    rate: Rate,
    /// The timeline's frame and the clip's picture, in pixels.
    frame: (u32, u32),
    picture: (u32, u32),
    /// The file it plays, if it plays one.
    media: Option<&'a project::MediaRef>,
    playhead: Time,
    /// The moment its numbers are shown at: the playhead's, or its first
    /// or last frame's when the playhead is off it.
    moment: Time,
    on_clip: bool,
}

impl<'a> Shown<'a> {
    fn new(
        project: &'a Project,
        seq: &'a project::Sequence,
        track: &'a project::Track,
        clip: &'a project::Clip,
        playhead: Time,
    ) -> Shown<'a> {
        let frame = (seq.settings.width, seq.settings.height);
        let rate = seq.settings.frame_rate;
        Shown {
            project,
            track,
            clip,
            sequence: project.active_sequence,
            rate,
            frame,
            picture: picture_size(project, clip).unwrap_or(frame),
            media: project.media.get(&clip.media).filter(|_| clip.is_file()),
            playhead,
            moment: clip.moment(playhead, rate),
            on_clip: clip.start <= playhead && playhead < clip.end(),
        }
    }
}

/// One undo step: the timeline made `size` and the clip's cropped picture
/// fitted to it again, as large as fits.
fn timeline_to(seq: &project::Sequence, at: &Shown, before: &Look, size: (u32, u32)) -> Command {
    let crop = [
        before.crop_left,
        before.crop_top,
        before.crop_right,
        before.crop_bottom,
    ]
    .map(|c| f64::from(c) / f64::from(project::look::ONE));
    let look = look::cropped_to_fit(before, at.picture, crop, size);
    Command::Steps(vec![
        Command::SetSettings {
            sequence: at.sequence,
            settings: project::SequenceSettings {
                width: size.0,
                height: size.1,
                ..seq.settings
            },
        },
        Command::SetLookAt {
            sequence: at.sequence,
            clip: at.clip.id,
            at: at.moment,
            look,
        },
    ])
}

/// The look as the picture sections left it this frame, for
/// [`Inspector::finish_look`].
struct LookEdit {
    /// The look the numbers started from.
    before: Look,
    values: Values,
    touched: Touched,
    asks: Asks,
    keys: KeyTable,
}

/// What the clip is: its name, track and length, and why what is shown may
/// not be what is seen (several selected, its track hidden, the playhead
/// off it).
fn header(ui: &mut egui::Ui, theme: &Theme, at: &Shown, selected: usize) {
    let (project, track, clip) = (at.project, at.track, at.clip);
    let name = match clip.kind {
        project::ClipKind::Adjustment => project::timelines::ADJUSTMENT_NAME.to_owned(),
        project::ClipKind::Title => project.clip_name(clip),
        project::ClipKind::Timeline(id) => project.sequence(id).map_or_else(
            || "A missing timeline".to_owned(),
            |s| format!("{} (nested timeline)", s.name),
        ),
        project::ClipKind::File => at
            .media
            .and_then(|media| media.path.file_name())
            .map_or_else(|| "Clip".to_owned(), |name| name.to_string_lossy().into_owned()),
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(name)
                .family(ui_kit::fonts::semibold())
                .size(15.0),
        )
        .truncate(),
    );
    ui.label(
        egui::RichText::new(format!(
            "{} · {} long",
            track.name,
            clip.duration.timecode(at.rate)
        ))
        .color(theme.muted)
        .size(12.0),
    );
    if selected > 1 {
        ui.label(
            egui::RichText::new("Several clips are selected: this is the one on top.")
                .color(theme.muted)
                .size(12.0),
        );
    }
    if track.hidden {
        ui.label(
            egui::RichText::new(format!(
                "{} is hidden (the eye by its name), so the viewer doesn't show this clip.",
                track.name
            ))
            .color(ui_kit::status::WARNING)
            .size(12.0),
        );
    }
    // The numbers are the clip's at the playhead (at its first or last
    // frame when the playhead is off it).
    if track.kind == TrackKind::Video && !clip.keyframes.is_empty() && !at.on_clip {
        let end = if at.playhead < clip.start { "first" } else { "last" };
        ui.label(
            egui::RichText::new(format!(
                "The playhead is off this clip: these are its {end} frame's values, and changes go there."
            ))
            .color(theme.muted)
            .size(12.0),
        );
    }
    ui.add_space(6.0);
}

/// The size of the picture a clip shows: its file's, upright, or the
/// nested timeline's. `None` for an adjustment layer, or a file with no
/// picture.
fn picture_size(project: &Project, clip: &project::Clip) -> Option<(u32, u32)> {
    match clip.kind {
        project::ClipKind::File => project
            .media
            .get(&clip.media)?
            .info
            .video
            .as_ref()
            .map(|video| video.display_size()),
        project::ClipKind::Timeline(id) => project
            .sequence(id)
            .map(|s| (s.settings.width, s.settings.height)),
        // A title's picture is the frame.
        project::ClipKind::Adjustment | project::ClipKind::Title => None,
    }
}

/// The clip `id` on the open timeline, on any track.
fn clip_in(project: &Project, id: ClipId) -> Option<&project::Clip> {
    let seq = project.active()?;
    seq.video_tracks
        .iter()
        .chain(&seq.audio_tracks)
        .flat_map(|track| &track.clips)
        .find(|clip| clip.id == id)
}

/// Whether the inspector has a clip to show for `selected`; when it hasn't,
/// the page shows the timeline in its place.
pub fn shows_a_clip(project: Option<&Project>, selected: &[ClipId]) -> bool {
    project.is_some_and(|project| shown_clip(project, selected).is_some())
}

/// What the inspector says about a timeline when no clip is selected:
/// label and value, the first also being the folded section's summary.
pub fn timeline_rows(seq: &project::Sequence) -> Vec<(&'static str, String)> {
    let settings = seq.settings;
    let rate = settings.frame_rate;
    let clips: usize = seq
        .video_tracks
        .iter()
        .chain(seq.audio_tracks.iter())
        .map(|track| track.clips.len())
        .sum();
    let fps = format!("{:.2}", rate.as_f64());
    let fps = fps.trim_end_matches('0').trim_end_matches('.');
    vec![
        ("Size", format!("{} × {}", settings.width, settings.height)),
        ("Frame rate", format!("{fps} frames a second")),
        (
            "Sound",
            format!("{} kHz", f64::from(settings.sample_rate) / 1000.0),
        ),
        ("Length", seq.length().timecode(rate)),
        (
            "Tracks",
            format!(
                "{} video, {} audio",
                seq.video_tracks.len(),
                seq.audio_tracks.len()
            ),
        ),
        ("Clips", clips.to_string()),
        ("Master", format!("{:+.1} dB", seq.master_gain_db)),
    ]
}

/// The picture effects would go on: the clip the inspector shows, when it
/// is a picture (on a video track) that plays a file or a timeline.
pub fn effects_target(project: &Project, selected: &[ClipId]) -> Option<ClipId> {
    let (_, track, clip) = shown_clip(project, selected)?;
    (track.kind == TrackKind::Video && !clip.is_adjustment()).then_some(clip.id)
}

/// The clip the inspector is about, picture or sound.
pub fn shown_clip_id(project: &Project, selected: &[ClipId]) -> Option<ClipId> {
    shown_clip(project, selected).map(|(_, _, clip)| clip.id)
}

/// The clips a transition of `kind` asked for on `clip` goes into: the
/// clip itself for a picture transition; for a crossfade, the sound
/// clip and the other sound streams of its link (or a picture's sound
/// streams), those with a clip just before them. None found, the clip
/// itself, so the refusal says why.
pub fn transition_targets(project: &Project, kind: project::TransitionKind, clip: ClipId) -> Vec<ClipId> {
    let Some(seq) = project.active() else {
        return vec![clip];
    };
    if !kind.is_sound() {
        return vec![clip];
    }
    let link = seq
        .video_tracks
        .iter()
        .chain(seq.audio_tracks.iter())
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .and_then(|c| c.link);
    let found: Vec<ClipId> = seq
        .audio_tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| c.id == clip || (link.is_some() && c.link == link))
        .filter(|(t, c)| project::transitions::before(t, c).is_some())
        .map(|(_, c)| c.id)
        .collect();
    if found.is_empty() { vec![clip] } else { found }
}

/// The clip the inspector is about: of the selected ones with a picture,
/// the one on the highest track (what is seen on top), and on that track
/// the earliest. With no picture selected, the selected sound clip on the
/// highest audio track.
fn shown_clip<'a>(
    project: &'a Project,
    selected: &[ClipId],
) -> Option<(&'a project::Sequence, &'a project::Track, &'a project::Clip)> {
    let seq = project.active()?;
    seq.video_tracks
        .iter()
        .rev()
        .chain(seq.audio_tracks.iter())
        .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
        .find(|(_, clip)| selected.contains(&clip.id))
        .map(|(track, clip)| (seq, track, clip))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::keyframes::over_time;
    use super::picture::{COLOUR, CROP_FIELDS, TRANSFORM};
    use super::*;
    use project::look::ONE;
    use project::{
        Fingerprint, LookField, MediaId, MediaInfo, MediaKind, MediaRef, Rate, SequenceSettings, VideoStream,
    };
    use ui_kit::keyframe::KeyClicks;

    pub(crate) fn project_with_a_clip() -> Project {
        let media = MediaRef {
            id: MediaId(0),
            path: "facecam.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(10),
            info: MediaInfo {
                container: String::new(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "h264".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: Vec::new(),
                stated_length: None,
            },
        };
        let project = Project::new("T", SequenceSettings::GAME_CLIP);
        Command::Import {
            sequence: project.active_sequence,
            media,
        }
        .apply(&project)
        .unwrap()
    }

    #[test]
    fn a_change_being_made_is_shown_before_it_is_an_undo_step() {
        let project = project_with_a_clip();
        let clip = project.active().unwrap().video_tracks[0].clips[0].clone();
        let mut inspector = Inspector::default();
        assert!(inspector.shown(&project).is_none(), "nothing being changed");
        let look = Look {
            scale: 500_000,
            ..Look::PLAIN
        };
        inspector.pending.set(
            LookAt {
                clip: clip.id,
                at: Time::ZERO,
            },
            look,
        );
        let shown = inspector.shown(&project).unwrap();
        assert_eq!(shown.active().unwrap().video_tracks[0].clips[0].look, look);
        assert_eq!(
            project.active().unwrap().video_tracks[0].clips[0].look,
            Look::PLAIN,
            "the project itself is untouched"
        );
    }

    #[test]
    fn with_nothing_selected_the_timeline_is_described() {
        let project = project_with_a_clip();
        assert!(!shows_a_clip(Some(&project), &[]));
        assert!(!shows_a_clip(None, &[]));
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        assert!(shows_a_clip(Some(&project), &[clip]));
        let rows = timeline_rows(project.active().unwrap());
        let row = |name: &str| {
            rows.iter()
                .find(|(label, _)| *label == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(row("Size"), "1920 × 1080");
        assert_eq!(row("Frame rate"), "60 frames a second");
        assert_eq!(row("Clips"), "1");
        assert_eq!(row("Master"), "+0.0 dB");
    }

    #[test]
    fn of_clips_on_two_tracks_the_one_on_top_is_shown() {
        let project = project_with_a_clip();
        let seq = project.active_sequence;
        let with_track = Command::AddTrack {
            sequence: seq,
            kind: project::TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&project)
        .unwrap();
        let sequence = with_track.active().unwrap();
        let lower = sequence.video_tracks[0].clips[0].clone();
        let mut upper = lower.clone();
        upper.link = None;
        let placed = Command::PlaceClip {
            sequence: seq,
            track: sequence.video_tracks[1].id,
            clip: upper,
        }
        .apply(&with_track)
        .unwrap();
        let tracks = &placed.active().unwrap().video_tracks;
        let (lower, upper) = (tracks[0].clips[0].id, tracks[1].clips[0].id);
        let shown = |selected: &[ClipId]| {
            shown_clip(&placed, selected).map(|(_, track, clip)| (track.name.clone(), clip.id))
        };
        assert_eq!(shown(&[lower, upper]), Some(("V2".to_owned(), upper)));
        assert_eq!(shown(&[lower]), Some(("V1".to_owned(), lower)));
        assert_eq!(shown(&[]), None);
    }

    /// A recording with a picture and two sound streams (game and mic), 10 s.
    pub(crate) fn project_with_sound() -> Project {
        let mut media = project_with_a_clip().media.values().next().unwrap().clone();
        media.info.audio = ["Game", "Mic"]
            .iter()
            .enumerate()
            .map(|(i, name)| project::AudioStream {
                index: i as u32 + 1,
                codec: "aac".into(),
                channels: 2,
                sample_rate: 48_000,
                name: (*name).into(),
            })
            .collect();
        let project = Project::new("T", SequenceSettings::GAME_CLIP);
        Command::Import {
            sequence: project.active_sequence,
            media,
        }
        .apply(&project)
        .unwrap()
    }

    fn sound_ids(project: &Project) -> Vec<ClipId> {
        project
            .active()
            .unwrap()
            .audio_tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect()
    }

    #[test]
    fn a_picture_shows_its_linked_sound_and_a_sound_clip_itself() {
        let project = with_sound_and_spare();
        let seq = project.active().unwrap();
        let picture = &seq.video_tracks[0].clips[0];
        let sounds = sound_ids(&project);
        let (game, mic, spare) = (sounds[0], sounds[1], sounds[2]);
        let of = |clip: &project::Clip| sound_clips(seq, clip).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(of(picture), vec![game, mic], "both streams, not the unlinked one");
        let mic_clip = seq.audio_tracks[1].clips[0].clone();
        assert_eq!(of(&mic_clip), vec![mic], "a sound clip is set alone");
        // Selecting sound alone shows it; a picture selected with it wins.
        let shown = |selected: &[ClipId]| shown_clip(&project, selected).map(|(_, t, c)| (t.kind, c.id));
        assert_eq!(shown(&[spare]), Some((TrackKind::Audio, spare)));
        assert_eq!(shown(&[mic, picture.id]), Some((TrackKind::Video, picture.id)));
        // A sound clip has no picture to take hold of in the viewer.
        assert!(
            Inspector::default()
                .subject(&project, &[mic], Time::ZERO)
                .is_none()
        );
        assert!(
            Inspector::default()
                .subject(&project, &[picture.id], Time::ZERO)
                .is_some()
        );
    }

    /// The recording, plus a sound clip of its own (music) later on the mic's track.
    fn with_sound_and_spare() -> Project {
        let project = project_with_sound();
        let seq = project.active().unwrap();
        let mut spare = seq.audio_tracks[1].clips[0].clone();
        spare.link = None;
        spare.start = Time::seconds(20);
        Command::PlaceClip {
            sequence: seq.id,
            track: seq.audio_tracks[1].id,
            clip: spare,
        }
        .apply(&project)
        .unwrap()
    }

    #[test]
    fn a_sound_clip_shows_only_the_sections_that_apply() {
        let project = project_with_sound();
        let mic = sound_ids(&project)[1];
        let picture = project.active().unwrap().video_tracks[0].clips[0].id;
        let titles = |selected: &[ClipId]| {
            let ctx = egui::Context::default();
            let mut inspector = Inspector::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                inspector.ui(
                    ui,
                    &ui_kit::ThemeName::default().theme(),
                    &project,
                    selected,
                    Time::ZERO,
                );
            });
            output.textures_delta.clear();
            let text = format!("{:?}", output.shapes);
            ["Transform", "Crop", "Quick color", "Speed", "Audio"]
                .into_iter()
                .filter(|title| text.contains(&format!("\"{title}\"")))
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(&[mic]), ["Speed", "Audio"]);
        assert_eq!(
            titles(&[picture]),
            ["Transform", "Crop", "Quick color", "Speed", "Audio"]
        );
    }

    #[test]
    fn a_sound_change_is_shown_first_and_is_one_step_for_every_linked_stream() {
        let project = project_with_sound();
        let seq = project.active_sequence;
        let sounds = sound_ids(&project);
        let picture = project.active().unwrap().video_tracks[0].clips[0].id;
        let quiet = Sound {
            gain_mdb: -6_000,
            ..Sound::PLAIN
        };
        let mut inspector = Inspector::default();
        inspector
            .pending_sound
            .set(sounds.clone(), vec![quiet; sounds.len()]);
        assert_eq!(inspector.sounds_being_set().len(), 2);
        let shown = inspector.shown(&project).unwrap();
        assert!(
            shown
                .active()
                .unwrap()
                .audio_tracks
                .iter()
                .all(|t| t.clips[0].sound == quiet)
        );
        assert!(
            project
                .active()
                .unwrap()
                .audio_tracks
                .iter()
                .all(|t| t.clips[0].sound == Sound::PLAIN)
        );
        // The hand is off it: one step for both streams.
        let commands = frame(&mut inspector, &project, &[picture]);
        assert_eq!(
            commands,
            vec![Command::SetSound {
                sequence: seq,
                sounds: sounds.iter().map(|id| (*id, quiet)).collect()
            }]
        );
        assert!(inspector.sounds_being_set().is_empty());
        // Left hanging when another clip is chosen: finished, not lost.
        let mut inspector = Inspector::default();
        inspector.pending_sound.set(vec![sounds[0]], vec![quiet]);
        assert_eq!(frame(&mut inspector, &project, &[]).len(), 1);
    }

    /// Playback follows a volume being dragged in the Audio section before
    /// it is let go: the project as it would be, the stored one untouched.
    #[test]
    fn a_sound_being_dragged_is_what_playback_hears() {
        let project = project_with_sound();
        let clip = sound_ids(&project)[0];
        let mut inspector = Inspector::default();
        assert!(inspector.shown(&project).is_none(), "nothing being set");
        let quiet = Sound {
            gain_mdb: -12_000,
            ..Sound::PLAIN
        };
        inspector.pending_sound.set(vec![clip], vec![quiet]);
        let heard = inspector.shown(&project).unwrap();
        let sound_of = |p: &Project| {
            p.active()
                .unwrap()
                .tracks()
                .flat_map(|t| t.clips.iter())
                .find(|c| c.id == clip)
                .unwrap()
                .sound
        };
        assert_eq!(sound_of(&heard), quiet);
        assert_eq!(sound_of(&project), Sound::PLAIN);
    }

    /// The clip of `project_with_a_clip` (10 s from 0) with its opacity
    /// keyed: 0 at 2 s, full at 6 s (on the timeline).
    fn fading() -> (Project, ClipId) {
        let mut project = project_with_a_clip();
        let sequence = project.active_sequence;
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        for (at, opacity) in [(2, 0), (6, ONE)] {
            project = Command::Keyframe {
                sequence,
                clip,
                at: Time::seconds(at),
                fields: vec![LookField::Opacity],
                change: KeyframeChange::Add,
            }
            .apply(&project)
            .unwrap();
            project = Command::SetLookAt {
                sequence,
                clip,
                at: Time::seconds(at),
                look: Look {
                    opacity,
                    ..Look::PLAIN
                },
            }
            .apply(&project)
            .unwrap();
        }
        (project, clip)
    }

    #[test]
    fn diamonds_say_where_the_keyframes_are() {
        let (project, _) = fading();
        let clip = &project.active().unwrap().video_tracks[0].clips[0];
        let rate = Rate::FPS_60;
        let table = |seconds: i64| KeyTable::at(clip, Time::seconds(seconds), rate, true);
        let opacity = |seconds: i64| *table(seconds).of(&[LookField::Opacity]).unwrap();
        assert_eq!(opacity(2).state, KeyState::At);
        assert_eq!(opacity(2).ease, Some(0), "linear");
        assert_eq!(opacity(4).state, KeyState::Between);
        assert_eq!((opacity(4).before, opacity(4).after), (Some(120), Some(360)));
        assert_eq!((opacity(1).before, opacity(1).after), (None, Some(120)));
        assert_eq!(table(4).of(&[LookField::Y]).unwrap().state, KeyState::Still);
        assert!(table(4).animates(&TRANSFORM) && !table(4).animates(&CROP_FIELDS));
        assert_eq!(over_time(&table(4), &TRANSFORM), "Over time: opacity · ");
        assert_eq!(over_time(&table(4), &COLOUR), "");
        // The numbers shown are the ones at the playhead.
        let inspector = Inspector::default();
        let shown = |seconds| {
            inspector
                .subject(&project, &[clip.id], Time::seconds(seconds))
                .unwrap()
                .look
                .opacity
        };
        assert_eq!((shown(1), shown(4), shown(8)), (0, ONE / 2, ONE));
    }

    #[test]
    fn a_diamond_click_adds_or_removes_and_an_arrow_moves_the_playhead() {
        let (project, clip_id) = fading();
        let sequence = project.active_sequence;
        let clip = &project.active().unwrap().video_tracks[0].clips[0];
        let rate = Rate::FPS_60;
        let mut inspector = Inspector::default();
        let clicked = |group: &'static [LookField], clicks: KeyClicks| Asks {
            resets: Vec::new(),
            keys: vec![(group, clicks)],
            ..Asks::default()
        };
        let toggle = KeyClicks {
            toggle: true,
            ..KeyClicks::default()
        };
        let at = |seconds: i64| Time::seconds(seconds);
        let answer = |inspector: &mut Inspector, asks: &Asks, seconds: i64| {
            let keys = KeyTable::at(clip, at(seconds), rate, true);
            inspector.answer(asks, &keys, sequence, clip_id, at(seconds), rate)
        };
        let keyframe = |seconds: i64, fields: &[LookField], change| Command::Keyframe {
            sequence,
            clip: clip_id,
            at: at(seconds),
            fields: fields.to_vec(),
            change,
        };
        let on_opacity = clicked(LookField::Opacity.group(), toggle);
        assert_eq!(
            answer(&mut inspector, &on_opacity, 2),
            [keyframe(2, &[LookField::Opacity], KeyframeChange::Remove)]
        );
        assert_eq!(
            answer(&mut inspector, &on_opacity, 4),
            [keyframe(4, &[LookField::Opacity], KeyframeChange::Add)]
        );
        // A place starts changing over time with X and Y together.
        let on_place = clicked(LookField::X.group(), toggle);
        assert_eq!(
            answer(&mut inspector, &on_place, 4),
            [keyframe(4, &[LookField::X, LookField::Y], KeyframeChange::Add)]
        );
        let hold = KeyClicks {
            ease: Some(1),
            ..KeyClicks::default()
        };
        assert_eq!(
            answer(&mut inspector, &clicked(LookField::Opacity.group(), hold), 6),
            [keyframe(
                6,
                &[LookField::Opacity],
                KeyframeChange::Ease(Ease::Hold)
            )]
        );
        let back = KeyClicks {
            previous: true,
            ..KeyClicks::default()
        };
        assert!(answer(&mut inspector, &clicked(LookField::Opacity.group(), back), 4).is_empty());
        assert_eq!(inspector.seek, Some(at(2)));
        // Reset puts the section back and takes its keyframes.
        let reset = Asks {
            resets: vec![&TRANSFORM],
            ..Asks::default()
        };
        let commands = answer(&mut inspector, &reset, 4);
        assert_eq!(
            commands,
            [Command::ResetLook {
                sequence,
                clip: clip_id,
                fields: TRANSFORM.to_vec()
            }]
        );
        let after = commands[0].apply(&project).unwrap();
        assert!(
            after.active().unwrap().video_tracks[0].clips[0]
                .keyframes
                .is_empty()
        );
    }

    #[test]
    fn a_change_to_an_animated_number_is_a_keyframe_where_it_began() {
        let (project, clip) = fading();
        let mut inspector = Inspector::default();
        let look = Look {
            opacity: 100_000,
            ..Look::PLAIN
        };
        // Begun with the playhead at 4 s; the playhead moved on since.
        inspector.pending.set(
            LookAt {
                clip,
                at: Time::seconds(4),
            },
            look,
        );
        let commands = frame(&mut inspector, &project, &[]);
        let after = commands[0].apply(&project).unwrap();
        let stored = &after.active().unwrap().video_tracks[0].clips[0];
        assert_eq!(stored.keyframes.of(LookField::Opacity).len(), 3);
        assert_eq!(stored.look_at(Time::seconds(4)).opacity, 100_000);
        assert_eq!(stored.look_at(Time::seconds(2)).opacity, 0, "the others stay");
    }

    /// Runs the inspector for one frame without a window.
    fn frame(inspector: &mut Inspector, project: &Project, selected: &[ClipId]) -> Vec<Command> {
        let ctx = egui::Context::default();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            commands = inspector.ui(
                ui,
                &ui_kit::ThemeName::default().theme(),
                project,
                selected,
                Time::ZERO,
            );
        });
        // No window to hand the fonts' texture to.
        output.textures_delta.clear();
        commands
    }

    #[test]
    fn a_change_left_on_a_clip_no_longer_shown_becomes_its_undo_step() {
        let project = project_with_a_clip();
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        let look = Look {
            opacity: 400_000,
            ..Look::PLAIN
        };
        let at = Time::seconds(1);
        let mut inspector = Inspector::default();
        inspector.pending.set(LookAt { clip, at }, look);
        // Nothing selected any more: the change is finished, not lost.
        let commands = frame(&mut inspector, &project, &[]);
        assert_eq!(
            commands,
            vec![Command::SetLookAt {
                sequence: project.active_sequence,
                clip,
                at,
                look
            }]
        );
        assert!(inspector.pending.get().is_none());
        // With the clip selected and nothing touched, nothing is changed.
        assert!(frame(&mut inspector, &project, &[clip]).is_empty());
    }

    #[test]
    fn words_left_in_a_title_when_nothing_is_selected_become_its_undo_step() {
        use project::titles::{Title, TitleStyle};
        let project = project_with_a_clip();
        let project = Command::AddTitle {
            sequence: project.active_sequence,
            at: Time::ZERO,
            length: Time::seconds(4),
            track: None,
            title: Box::new(Title::new(TitleStyle::Centred)),
        }
        .apply(&project)
        .unwrap();
        let clip = project.active().unwrap().video_tracks[1].clips[0].id;
        let mut words = Title::new(TitleStyle::Centred);
        words.text = "Round two".into();
        let mut inspector = Inspector::default();
        // Typed with the field still focused, then the timeline's empty
        // space clicked: nothing is selected.
        inspector.pending_title.set(clip, words.clone());
        let commands = frame(&mut inspector, &project, &[]);
        assert_eq!(
            commands,
            vec![Command::SetTitle {
                sequence: project.active_sequence,
                clip,
                title: Box::new(words),
            }]
        );
        assert!(inspector.pending_title.get().is_none());
        assert!(frame(&mut inspector, &project, &[]).is_empty(), "only once");
    }

    #[test]
    fn choosing_another_clip_mid_drag_with_nothing_changed_adds_no_undo_step() {
        let project = project_with_sound();
        let seq = project.active().unwrap();
        let picture = seq.video_tracks[0].clips[0].clone();
        let sounds = sound_ids(&project);
        let mut inspector = Inspector::default();
        // A drag on the picture's look, effects and sound, each back where
        // it began, while the hand is still on it.
        inspector.pending.set(
            LookAt {
                clip: picture.id,
                at: Time::ZERO,
            },
            picture.look_at(Time::ZERO),
        );
        inspector.held = true;
        inspector.pending_effects.set(picture.id, picture.effects.clone());
        inspector
            .pending_sound
            .set(sounds.clone(), vec![Sound::PLAIN; sounds.len()]);
        // The mic's clip alone is chosen: each is finished, none a step.
        let commands = frame(&mut inspector, &project, &[sounds[1]]);
        assert!(commands.is_empty(), "{commands:?}");
        assert!(inspector.pending.get().is_none());
        assert!(inspector.pending_effects.get().is_none());
        assert!(inspector.sounds_being_set().is_empty());
    }
}
