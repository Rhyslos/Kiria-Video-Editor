//! The Edit page's inspector: the selected clip's Transform, Crop, Quick
//! color and Speed, on the right.
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

use project::look::ONE;
use project::{ClipId, Command, Look, Project, Speed, Time};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number, Touched};

/// The speeds offered as buttons, as fractions.
const SPEEDS: [(u32, u32); 5] = [(1, 4), (1, 2), (1, 1), (2, 1), (4, 1)];

#[derive(Debug, Default)]
pub struct Inspector {
    /// A look being set and not yet an undo step: whose, and what.
    pending: Option<(ClipId, Look)>,
    /// A hand outside the panel (on the picture in the viewer) is still on
    /// the look being set: the panel leaves finishing it to that hand.
    held: bool,
    /// Whether the selected clip can be moved and resized on the picture in
    /// the viewer (its outline and handles are shown). Off unless asked for,
    /// so selecting a clip doesn't put a frame around it.
    pub on_picture: bool,
    /// The tooltip of the switch for that: what it does, and its keys.
    pub on_picture_tip: String,
}

/// The clip the inspector is about, for what changes its look from outside
/// the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject {
    pub clip: ClipId,
    /// Its look as the panel shows it: with the change being made.
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
        let (clip, look) = self.pending?;
        Command::SetLook {
            sequence: project.active_sequence,
            clip,
            look,
        }
        .apply(project)
        .ok()
    }

    /// The clip the panel is about (see [`shown_clip`]) with its look as it
    /// is shown now. `None` when no selected clip has a picture.
    pub fn subject(&self, project: &Project, selected: &[ClipId]) -> Option<Subject> {
        let (seq, track, clip) = shown_clip(project, selected)?;
        let video = project.media.get(&clip.media)?.info.video.as_ref()?;
        let look = match self.pending {
            Some((id, look)) if id == clip.id => look,
            _ => clip.look,
        };
        Some(Subject {
            clip: clip.id,
            look,
            picture: video.display_size(),
            frame: (seq.settings.width, seq.settings.height),
            start: clip.start,
            end: clip.end(),
            hidden: track.hidden,
        })
    }

    /// A change to `clip`'s look made outside the panel, on the picture
    /// itself. It is shown at once, as the panel's own changes are. While
    /// `busy` the hand is still on it and more will follow; once it is not,
    /// the change is finished: the undo step comes back.
    pub fn changed_outside(
        &mut self,
        project: &Project,
        clip: ClipId,
        look: Look,
        busy: bool,
    ) -> Option<Command> {
        // A change in waiting on another clip is not lost.
        let other = match self.pending {
            Some((waiting, _)) if waiting != clip => self.finish(project),
            _ => None,
        };
        self.pending = Some((clip, look.within_limits()));
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
    /// the clip already has.
    fn finish(&mut self, project: &Project) -> Option<Command> {
        self.held = false;
        let (clip, look) = self.pending.take()?;
        let stored = project
            .active()?
            .video_tracks
            .iter()
            .flat_map(|track| &track.clips)
            .find(|candidate| candidate.id == clip)?
            .look;
        (look != stored).then_some(Command::SetLook {
            sequence: project.active_sequence,
            clip,
            look,
        })
    }

    /// Draws the inspector for the first of `selected` that has a picture.
    /// Returns the changes that are finished: each is one undo step.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        selected: &[ClipId],
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let sequence = project.active_sequence;
        let target = shown_clip(project, selected);
        // A change left hanging on a clip that is no longer the one shown
        // (another was selected mid-drag) is finished first.
        let showing = target.map(|(_, _, clip)| clip.id);
        if let Some((clip, look)) = self.pending
            && Some(clip) != showing
        {
            commands.push(Command::SetLook { sequence, clip, look });
            self.pending = None;
            self.held = false;
        }
        // Nothing selected: the page shows the timeline here instead.
        let Some((seq, track, clip)) = target else {
            return commands;
        };
        let frame = (seq.settings.width, seq.settings.height);
        let media = project.media.get(&clip.media);
        let picture = media
            .and_then(|media| media.info.video.as_ref())
            .map_or(frame, |video| video.display_size());

        // What it is.
        let name = media
            .and_then(|media| media.path.file_name())
            .map_or_else(|| "Clip".to_owned(), |name| name.to_string_lossy().into_owned());
        ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .family(ui_kit::fonts::semibold())
                    .size(15.0),
            )
            .truncate(),
        );
        let rate = seq.settings.frame_rate;
        ui.label(
            egui::RichText::new(format!("{} · {} long", track.name, clip.duration.timecode(rate)))
                .color(theme.muted)
                .size(12.0),
        );
        if selected.len() > 1 {
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
        ui.add_space(6.0);

        let stored = clip.look;
        let before = self.pending.map_or(stored, |(_, look)| look);
        let mut values = Values::of(&before, frame);
        let mut touched = Touched::default();

        let clicks = inspector::section(
            ui,
            theme,
            "Transform",
            &transform_summary(&before, frame),
            before.is_moved() || before.opacity != ONE,
            |ui| {
                touched = touched.and(inspector::pair(
                    ui,
                    theme,
                    "Position",
                    ("X", &mut values.x, pixels(frame.0)),
                    ("Y", &mut values.y, pixels(frame.1)),
                ));
                touched = touched.and(inspector::number(ui, theme, "Size", &mut values.scale, SIZE));
                touched = touched.and(inspector::number(ui, theme, "Turn", &mut values.turn, TURN));
                touched = touched.and(inspector::slider(
                    ui,
                    theme,
                    "Opacity",
                    &mut values.opacity,
                    PERCENT_FULL,
                ));
                ui.horizontal(|ui| {
                    ui.add_space(90.0);
                    if ui
                        .button("Fit")
                        .on_hover_text("As large as fits the frame, centred: nothing cut off")
                        .clicked()
                    {
                        let fit = Values::of(&fitted(&before), frame);
                        (values.x, values.y, values.scale) = (fit.x, fit.y, fit.scale);
                        touched.changed = true;
                    }
                    if ui
                        .button("Fill")
                        .on_hover_text("Large enough to fill the frame, centred: no bars")
                        .clicked()
                    {
                        (values.x, values.y, values.scale) = (0.0, 0.0, fill_percent(picture, frame));
                        touched.changed = true;
                    }
                    ui.toggle_value(&mut self.on_picture, "On the picture")
                        .on_hover_text(&self.on_picture_tip);
                });
                // One click for the usual places: a small picture in a
                // corner (a facecam over a recording), or one half of the
                // frame (two pictures stacked on a vertical timeline).
                let now = values.look(frame, &before);
                let mut placed = None;
                ui.horizontal(|ui| {
                    inspector::label_cell(ui, theme, "Layout", 86.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (label, tip, right, bottom) in [
                        ("↖", "Small, in the top left corner", false, false),
                        ("↗", "Small, in the top right corner", true, false),
                        ("↙", "Small, in the bottom left corner", false, true),
                        ("↘", "Small, in the bottom right corner", true, true),
                    ] {
                        if ui.button(label).on_hover_text(tip).clicked() {
                            placed = Some(in_corner(&now, picture, frame, right, bottom));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(90.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (label, tip, bottom) in [
                        ("Top half", "Fills the top half of the frame, cut to fit", false),
                        (
                            "Bottom half",
                            "Fills the bottom half of the frame, cut to fit",
                            true,
                        ),
                    ] {
                        if ui.button(label).on_hover_text(tip).clicked() {
                            placed = Some(in_half(&now, picture, frame, bottom));
                        }
                    }
                });
                if let Some(look) = placed {
                    values = Values::of(&look, frame);
                    touched.changed = true;
                }
            },
        );
        if clicks.reset {
            let plain = Values::of(&Look::PLAIN, frame);
            (values.x, values.y, values.scale, values.turn, values.opacity) =
                (plain.x, plain.y, plain.scale, plain.turn, plain.opacity);
            touched.changed = true;
        }

        let clicks = inspector::section(
            ui,
            theme,
            "Crop",
            &crop_summary(&before),
            before.is_cropped(),
            |ui| {
                for (label, value) in ["Left", "Top", "Right", "Bottom"]
                    .into_iter()
                    .zip(&mut values.crop)
                {
                    touched = touched.and(inspector::slider(ui, theme, label, value, CROP));
                }
            },
        );
        if clicks.reset {
            values.crop = [0.0; 4];
            touched.changed = true;
        }

        let clicks = inspector::section(
            ui,
            theme,
            "Quick color",
            &colour_summary(&before),
            before.is_coloured(),
            |ui| {
                touched = touched.and(inspector::slider(
                    ui,
                    theme,
                    "Exposure",
                    &mut values.exposure,
                    EXPOSURE,
                ));
                for (label, value) in [
                    ("Contrast", &mut values.contrast),
                    ("Saturation", &mut values.saturation),
                    ("Temperature", &mut values.temperature),
                    ("Tint", &mut values.tint),
                ] {
                    touched = touched.and(inspector::slider(ui, theme, label, value, EITHER_WAY));
                }
            },
        );
        if clicks.reset {
            (
                values.exposure,
                values.contrast,
                values.saturation,
                values.temperature,
                values.tint,
            ) = (0.0, 0.0, 0.0, 0.0, 0.0);
            touched.changed = true;
        }

        inspector::section(ui, theme, "Speed", &clip.speed.percent(), false, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (num, den) in SPEEDS {
                    let Ok(speed) = Speed::new(num, den) else { continue };
                    if ui
                        .selectable_label(clip.speed == speed, speed.percent())
                        .clicked()
                        && clip.speed != speed
                    {
                        commands.push(Command::SetSpeed {
                            sequence,
                            clip: clip.id,
                            speed,
                        });
                    }
                }
            });
            ui.label(
                egui::RichText::new(
                    "The clip gets longer or shorter, with its sound, and what follows moves along.",
                )
                .color(theme.muted)
                .size(12.0),
            );
        });

        // Shown at once; an undo step once the hand is off it, be that a
        // hand on the panel or one on the picture in the viewer.
        if touched.changed {
            self.pending = Some((clip.id, values.look(frame, &before)));
        }
        if !touched.busy
            && !self.held
            && let Some((id, look)) = self.pending.take()
            && look != stored
        {
            commands.push(Command::SetLook {
                sequence,
                clip: id,
                look,
            });
        }
        commands
    }
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

/// The clip the inspector is about: of the selected ones with a picture,
/// the one on the highest track (what is seen on top), and on that track
/// the earliest.
fn shown_clip<'a>(
    project: &'a Project,
    selected: &[ClipId],
) -> Option<(&'a project::Sequence, &'a project::Track, &'a project::Clip)> {
    let seq = project.active()?;
    seq.video_tracks
        .iter()
        .rev()
        .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
        .find(|(_, clip)| selected.contains(&clip.id))
        .map(|(track, clip)| (seq, track, clip))
}

/// A look's numbers as the inspector shows them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Values {
    /// The picture's centre from the frame's, in frame pixels.
    x: f64,
    y: f64,
    /// Percent of the size that fits the frame.
    scale: f64,
    /// Degrees clockwise.
    turn: f64,
    /// Percent cut off the left, top, right and bottom.
    crop: [f64; 4],
    /// Percent.
    opacity: f64,
    /// Stops.
    exposure: f64,
    /// -100 to 100.
    contrast: f64,
    saturation: f64,
    temperature: f64,
    tint: f64,
}

const ONE_F: f64 = ONE as f64;

fn pixels(side: u32) -> Number {
    let reach = f64::from(Look::AWAY) / ONE_F * f64::from(side);
    Number {
        range: (-reach, reach),
        per_point: 1.0,
        decimals: 0,
        suffix: " px",
        usual: 0.0,
    }
}

const SIZE: Number = Number {
    range: (1.0, 10_000.0),
    per_point: 0.5,
    decimals: 1,
    suffix: " %",
    usual: 100.0,
};
const TURN: Number = Number {
    range: (-360.0, 360.0),
    per_point: 0.5,
    decimals: 1,
    suffix: "°",
    usual: 0.0,
};
const PERCENT_FULL: Number = Number {
    range: (0.0, 100.0),
    per_point: 1.0,
    decimals: 0,
    suffix: " %",
    usual: 100.0,
};
const CROP: Number = Number {
    range: (0.0, 99.0),
    per_point: 0.2,
    decimals: 1,
    suffix: " %",
    usual: 0.0,
};
const EXPOSURE: Number = Number {
    range: (-5.0, 5.0),
    per_point: 0.02,
    decimals: 2,
    suffix: "",
    usual: 0.0,
};
const EITHER_WAY: Number = Number {
    range: (-100.0, 100.0),
    per_point: 1.0,
    decimals: 0,
    suffix: "",
    usual: 0.0,
};

impl Values {
    fn of(look: &Look, frame: (u32, u32)) -> Values {
        let part = |value: i32| f64::from(value) / ONE_F;
        Values {
            x: part(look.x) * f64::from(frame.0),
            y: part(look.y) * f64::from(frame.1),
            scale: part(look.scale) * 100.0,
            turn: part(look.turn),
            crop: [look.crop_left, look.crop_top, look.crop_right, look.crop_bottom]
                .map(|side| part(side) * 100.0),
            opacity: part(look.opacity) * 100.0,
            exposure: part(look.exposure),
            contrast: part(look.contrast) * 100.0,
            saturation: part(look.saturation) * 100.0,
            temperature: part(look.temperature) * 100.0,
            tint: part(look.tint) * 100.0,
        }
    }

    /// The look these numbers mean, within its limits. A number that was
    /// not touched keeps `before`'s exact value, so showing a look and
    /// reading it back never changes it.
    fn look(&self, frame: (u32, u32), before: &Look) -> Look {
        let was = Values::of(before, frame);
        let whole = |now: f64, then: f64, stored: i32, one: f64| {
            if now == then {
                stored
            } else {
                (now / one * ONE_F).round() as i32
            }
        };
        Look {
            x: whole(self.x, was.x, before.x, f64::from(frame.0.max(1))),
            y: whole(self.y, was.y, before.y, f64::from(frame.1.max(1))),
            scale: whole(self.scale, was.scale, before.scale, 100.0),
            turn: whole(self.turn, was.turn, before.turn, 1.0),
            crop_left: whole(self.crop[0], was.crop[0], before.crop_left, 100.0),
            crop_top: whole(self.crop[1], was.crop[1], before.crop_top, 100.0),
            crop_right: whole(self.crop[2], was.crop[2], before.crop_right, 100.0),
            crop_bottom: whole(self.crop[3], was.crop[3], before.crop_bottom, 100.0),
            opacity: whole(self.opacity, was.opacity, before.opacity, 100.0),
            exposure: whole(self.exposure, was.exposure, before.exposure, 1.0),
            contrast: whole(self.contrast, was.contrast, before.contrast, 100.0),
            saturation: whole(self.saturation, was.saturation, before.saturation, 100.0),
            temperature: whole(self.temperature, was.temperature, before.temperature, 100.0),
            tint: whole(self.tint, was.tint, before.tint, 100.0),
        }
        .within_limits()
    }
}

/// `look` with the picture as large as fits the frame, centred: what the
/// Fit button and a double-click on the picture in the viewer mean. Its
/// turn, crop, opacity and colour stay.
pub fn fitted(look: &Look) -> Look {
    Look {
        x: 0,
        y: 0,
        scale: ONE,
        ..*look
    }
}

/// The size, in percent of the fitted size, at which a picture of
/// `picture`'s shape just fills a frame of `frame`'s.
fn fill_percent(picture: (u32, u32), frame: (u32, u32)) -> f64 {
    let across = f64::from(frame.0) / f64::from(picture.0.max(1));
    let down = f64::from(frame.1) / f64::from(picture.1.max(1));
    across.max(down) / across.min(down) * 100.0
}

/// How large a picture in a corner is, as a share of its fitted size, and
/// how far it stays from the frame's edges, as a share of the frame's width.
const CORNER_SIZE: f64 = 0.3;
const CORNER_MARGIN: f64 = 0.03;

/// The size a picture of `picture`'s shape has when fitted into `frame`, in
/// frame pixels.
fn fitted_size(picture: (u32, u32), frame: (u32, u32)) -> (f64, f64) {
    let (pw, ph) = (f64::from(picture.0.max(1)), f64::from(picture.1.max(1)));
    let scale = (f64::from(frame.0) / pw).min(f64::from(frame.1) / ph);
    (pw * scale, ph * scale)
}

fn millionths(share: f64) -> i32 {
    (share * ONE_F).round() as i32
}

/// `look` with the picture small in a corner of the frame: a third of its
/// fitted size, a little in from the edges, upright and uncut. Its opacity
/// and quick colour stay.
fn in_corner(look: &Look, picture: (u32, u32), frame: (u32, u32), right: bool, bottom: bool) -> Look {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (w, h) = fitted_size(picture, frame);
    let (w, h) = (w * CORNER_SIZE, h * CORNER_SIZE);
    let margin = CORNER_MARGIN * fw;
    // The centre's distance from the frame's centre, as a share of the frame.
    let x = (fw / 2.0 - margin - w / 2.0) / fw;
    let y = (fh / 2.0 - margin - h / 2.0) / fh;
    Look {
        x: millionths(if right { x } else { -x }),
        y: millionths(if bottom { y } else { -y }),
        scale: millionths(CORNER_SIZE),
        turn: 0,
        crop_left: 0,
        crop_top: 0,
        crop_right: 0,
        crop_bottom: 0,
        ..*look
    }
    .within_limits()
}

/// `look` with the picture filling the top or bottom half of the frame:
/// large enough to cover it, centred in it, and what sticks out cut off.
fn in_half(look: &Look, picture: (u32, u32), frame: (u32, u32), bottom: bool) -> Look {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (w, h) = fitted_size(picture, frame);
    let (half_w, half_h) = (fw, fh / 2.0);
    // Covers the half: the larger of the two scales.
    let scale = (half_w / w).max(half_h / h);
    let (w, h) = (w * scale, h * scale);
    let cut_x = ((w - half_w) / 2.0 / w).max(0.0);
    let cut_y = ((h - half_h) / 2.0 / h).max(0.0);
    Look {
        x: 0,
        y: millionths(if bottom { 0.25 } else { -0.25 }),
        scale: millionths(scale),
        turn: 0,
        crop_left: millionths(cut_x),
        crop_right: millionths(cut_x),
        crop_top: millionths(cut_y),
        crop_bottom: millionths(cut_y),
        ..*look
    }
    .within_limits()
}

/// A number without a trailing ".0".
fn short(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text,
    }
}

/// With its sign, also when positive.
fn signed(value: f64, decimals: usize) -> String {
    let text = short(value.abs(), decimals);
    if value < 0.0 {
        format!("−{text}")
    } else {
        format!("+{text}")
    }
}

fn transform_summary(look: &Look, frame: (u32, u32)) -> String {
    let values = Values::of(look, frame);
    let mut parts = Vec::new();
    if look.scale != ONE {
        parts.push(format!("{} %", short(values.scale, 1)));
    }
    if (look.x, look.y) != (0, 0) {
        parts.push(format!("at {}, {}", signed(values.x, 0), signed(values.y, 0)));
    }
    if look.turn != 0 {
        parts.push(format!("turned {}°", short(values.turn, 1)));
    }
    if look.opacity != ONE {
        parts.push(format!("{} % opaque", short(values.opacity, 0)));
    }
    if parts.is_empty() {
        "Fitted, centred".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn crop_summary(look: &Look) -> String {
    let values = Values::of(look, (1, 1));
    let parts: Vec<String> = ["Left", "Top", "Right", "Bottom"]
        .iter()
        .zip(values.crop)
        .filter(|(_, cut)| *cut > 0.0)
        .map(|(side, cut)| format!("{side} {} %", short(cut, 1)))
        .collect();
    if parts.is_empty() {
        "Nothing cut off".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn colour_summary(look: &Look) -> String {
    let values = Values::of(look, (1, 1));
    let mut parts = Vec::new();
    if look.exposure != 0 {
        parts.push(format!("Exposure {}", signed(values.exposure, 2)));
    }
    for (name, value) in [
        ("Contrast", values.contrast),
        ("Saturation", values.saturation),
        ("Temperature", values.temperature),
        ("Tint", values.tint),
    ] {
        if value != 0.0 {
            parts.push(format!("{name} {}", signed(value, 0)));
        }
    }
    if parts.is_empty() {
        "As it is".to_owned()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use project::{
        Fingerprint, MediaId, MediaInfo, MediaKind, MediaRef, Rate, SequenceSettings, VideoStream,
    };

    const FRAME: (u32, u32) = (1920, 1080);

    #[test]
    fn showing_a_look_and_reading_it_back_changes_nothing() {
        let look = Look {
            x: 333_333,
            y: -123_457,
            scale: 314_159,
            turn: -2_500_001,
            crop_left: 120_001,
            crop_bottom: 49_999,
            opacity: 899_999,
            exposure: 250_001,
            temperature: 150_003,
            ..Look::PLAIN
        };
        assert_eq!(Values::of(&look, FRAME).look(FRAME, &look), look);
        assert_eq!(
            Values::of(&Look::PLAIN, FRAME).look(FRAME, &Look::PLAIN),
            Look::PLAIN
        );
    }

    #[test]
    fn numbers_are_pixels_percent_and_degrees() {
        let look = Look {
            x: ONE / 4,
            y: -ONE / 10,
            scale: ONE * 3 / 10,
            turn: 90 * ONE,
            crop_left: ONE / 8,
            opacity: ONE / 2,
            ..Look::PLAIN
        };
        let values = Values::of(&look, FRAME);
        assert_eq!((values.x, values.y), (480.0, -108.0));
        assert_eq!((values.scale, values.turn, values.opacity), (30.0, 90.0, 50.0));
        assert_eq!(values.crop, [12.5, 0.0, 0.0, 0.0]);

        // A changed number is stored to the nearest millionth, and kept within limits.
        let mut changed = values;
        changed.x = 96.0;
        changed.scale = 0.2;
        let stored = changed.look(FRAME, &look);
        assert_eq!(stored.x, 50_000);
        assert_eq!(stored.scale, Look::SCALE.0, "never smaller than a hundredth");
        assert_eq!(stored.y, look.y, "what wasn't touched is exactly as before");
    }

    #[test]
    fn fill_is_how_much_larger_than_fitted_covers_the_frame() {
        assert_eq!(fill_percent((1920, 1080), (1920, 1080)), 100.0);
        // A wide recording on a tall timeline: fitted it is 1080 wide and
        // 607.5 high; to cover 1920 of height it must be 3.16 times that.
        let percent = fill_percent((1920, 1080), (1080, 1920));
        assert!((percent - 316.05).abs() < 0.01, "{percent}");
    }

    #[test]
    fn headers_say_what_is_set_in_a_line() {
        assert_eq!(transform_summary(&Look::PLAIN, FRAME), "Fitted, centred");
        let corner = Look {
            scale: 300_000,
            x: 320_000,
            y: 280_000,
            turn: -2_500_000,
            opacity: 900_000,
            ..Look::PLAIN
        };
        assert_eq!(
            transform_summary(&corner, FRAME),
            "30 % · at +614, +302 · turned -2.5° · 90 % opaque"
        );
        assert_eq!(crop_summary(&Look::PLAIN), "Nothing cut off");
        let cut = Look {
            crop_left: 120_000,
            crop_bottom: 50_000,
            ..Look::PLAIN
        };
        assert_eq!(crop_summary(&cut), "Left 12 % · Bottom 5 %");
        assert_eq!(colour_summary(&Look::PLAIN), "As it is");
        let warm = Look {
            exposure: 250_000,
            temperature: 150_000,
            saturation: -100_000,
            ..Look::PLAIN
        };
        assert_eq!(
            colour_summary(&warm),
            "Exposure +0.25 · Saturation −10 · Temperature +15"
        );
    }

    #[test]
    fn a_corner_layout_is_a_third_of_the_size_a_little_in_from_the_edges() {
        let tinted = Look {
            opacity: ONE / 2,
            temperature: 100_000,
            crop_left: 200_000,
            turn: 5 * ONE,
            ..Look::PLAIN
        };
        let corner = in_corner(&tinted, (1920, 1080), FRAME, true, true);
        assert_eq!(corner.scale, 300_000);
        // 576 x 324, 57.6 px in from the right and the bottom edge.
        let values = Values::of(&corner, FRAME);
        assert!((values.x - (960.0 - 57.6 - 288.0)).abs() < 0.01, "{}", values.x);
        assert!((values.y - (540.0 - 57.6 - 162.0)).abs() < 0.01, "{}", values.y);
        assert_eq!((corner.turn, corner.crop_left), (0, 0), "upright and uncut");
        assert_eq!(
            (corner.opacity, corner.temperature),
            (ONE / 2, 100_000),
            "the rest stays"
        );
        let top_left = in_corner(&tinted, (1920, 1080), FRAME, false, false);
        assert_eq!((top_left.x, top_left.y), (-corner.x, -corner.y));
    }

    #[test]
    fn a_half_layout_covers_its_half_and_cuts_what_sticks_out() {
        // A wide recording on a tall timeline: fitted it is 1080 x 607.5.
        let tall = (1080, 1920);
        let top = in_half(&Look::PLAIN, (1920, 1080), tall, false);
        // To cover 1080 x 960 it is scaled by 960 / 607.5.
        assert_eq!(top.scale, 1_580_247);
        assert_eq!(top.y, -250_000);
        // 1706.7 wide then: 313.3 cut off each side, nothing above or below.
        assert!(
            (f64::from(top.crop_left) / ONE_F - 313.33 / 1706.67).abs() < 1e-4,
            "{}",
            top.crop_left
        );
        assert_eq!(top.crop_left, top.crop_right);
        assert_eq!((top.crop_top, top.crop_bottom), (0, 0));
        assert_eq!(in_half(&Look::PLAIN, (1920, 1080), tall, true).y, 250_000);
        // On a wide frame the half is wider than high: top and bottom are cut.
        let wide = in_half(&Look::PLAIN, (1920, 1080), FRAME, false);
        assert_eq!(wide.scale, ONE);
        assert_eq!(
            (wide.crop_left, wide.crop_top, wide.crop_bottom),
            (0, 250_000, 250_000)
        );
    }

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
        inspector.pending = Some((clip.id, look));
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
        // Sound alone has nothing to show.
        let sound: Vec<ClipId> = placed
            .active()
            .unwrap()
            .audio_tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        assert_eq!(shown(&sound), None);
    }

    /// Runs the inspector for one frame without a window.
    fn frame(inspector: &mut Inspector, project: &Project, selected: &[ClipId]) -> Vec<Command> {
        let ctx = egui::Context::default();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            commands = inspector.ui(ui, &ui_kit::ThemeName::default().theme(), project, selected);
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
        let mut inspector = Inspector {
            pending: Some((clip, look)),
            ..Inspector::default()
        };
        // Nothing selected any more: the change is finished, not lost.
        let commands = frame(&mut inspector, &project, &[]);
        assert_eq!(
            commands,
            vec![Command::SetLook {
                sequence: project.active_sequence,
                clip,
                look
            }]
        );
        assert!(inspector.pending.is_none());
        // With the clip selected and nothing touched, nothing is changed.
        assert!(frame(&mut inspector, &project, &[clip]).is_empty());
    }
}
