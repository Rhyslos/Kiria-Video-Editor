//! The Edit page's inspector: the selected clip's Transform, Crop, Quick
//! color and Speed, on the right.
//!
//! A value being dragged is shown in the viewer at once ([`Inspector::shown`]
//! is the project as it would be) and becomes one undo step when the drag
//! ends, so a slider swept from one end to the other is undone in one go.

use project::look::ONE;
use project::{ClipId, Command, Look, Project, Speed};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number, Touched};

/// The speeds offered as buttons, as fractions.
const SPEEDS: [(u32, u32); 5] = [(1, 4), (1, 2), (1, 1), (2, 1), (4, 1)];

#[derive(Debug, Default)]
pub struct Inspector {
    /// A look being set and not yet an undo step: whose, and what.
    pending: Option<(ClipId, Look)>,
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
        let target = project.active().and_then(|seq| {
            let clip = seq
                .video_tracks
                .iter()
                .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
                .find(|(_, clip)| selected.contains(&clip.id))?;
            Some((seq, clip.0, clip.1))
        });
        // A change left hanging on a clip that is no longer the one shown
        // (another was selected mid-drag) is finished first.
        let showing = target.map(|(_, _, clip)| clip.id);
        if let Some((clip, look)) = self.pending
            && Some(clip) != showing
        {
            commands.push(Command::SetLook { sequence, clip, look });
            self.pending = None;
        }
        let Some((seq, track, clip)) = target else {
            inspector::note(
                ui,
                theme,
                "Select a clip on a video track to place, crop and colour its picture.",
            );
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
                egui::RichText::new("Several clips are selected: this is the first.")
                    .color(theme.muted)
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
                        (values.x, values.y, values.scale) = (0.0, 0.0, 100.0);
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
                });
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

        // Shown at once; an undo step once the hand is off it.
        if touched.changed {
            self.pending = Some((clip.id, values.look(frame, &before)));
        }
        if !touched.busy
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

/// The size, in percent of the fitted size, at which a picture of
/// `picture`'s shape just fills a frame of `frame`'s.
fn fill_percent(picture: (u32, u32), frame: (u32, u32)) -> f64 {
    let across = f64::from(frame.0) / f64::from(picture.0.max(1));
    let down = f64::from(frame.1) / f64::from(picture.1.max(1));
    across.max(down) / across.min(down) * 100.0
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
mod tests {
    use super::*;
    use project::{
        Fingerprint, MediaId, MediaInfo, MediaKind, MediaRef, Rate, SequenceSettings, Time, VideoStream,
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

    fn project_with_a_clip() -> Project {
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
