//! "Remove silence": pick a sound track and how quiet counts as silent; the
//! stretches found show on the timeline of every page that has one (the
//! shared timeline's `highlights`), and one click cuts them all out of
//! every track as a single undoable step.
//!
//! It starts on the first Dialogue track (the microphone), else the track
//! the Audio page shows, else the first. It reads the recording as it is,
//! before the track's own effects, and says so: hiss that Clean-up takes
//! out still counts as sound. So it also says how loud the recording is at
//! its quietest, and offers a threshold over that when the one set is
//! under it.

use engine::silence::{self, ClipSound, Settings};
use project::{Project, Rate, Region, Sequence, Time, TrackId, TrackRole};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The peaks of sound that counts as never silent: one at full level, read
/// wherever the clip is (`per_second` 0).
static LOUD: [f32; 1] = [1.0];

#[derive(Default)]
pub struct SilencePanel {
    pub open: bool,
    track: Option<TrackId>,
    settings: Settings,
    /// The stretches found with the current choices (seconds).
    pub found: Vec<(f64, f64)>,
    /// How loud the track's recording is at its quietest, dBFS.
    floor_db: Option<f32>,
}

/// The track the window starts on: the first Dialogue track, else
/// `shown` (the track the Audio page shows), else the first.
pub fn first_choice(seq: &Sequence, shown: Option<TrackId>) -> Option<TrackId> {
    seq.audio_tracks
        .iter()
        .find(|t| t.sound.role == TrackRole::Dialogue)
        .map(|t| t.id)
        .or_else(|| shown.filter(|id| seq.audio_tracks.iter().any(|t| t.id == *id)))
        .or_else(|| seq.audio_tracks.front().map(|t| t.id))
}

/// What the window asked for this frame.
pub enum Asked {
    Nothing,
    Remove(Vec<Region>),
}

impl SilencePanel {
    /// Opens the window on its first choice of track ([`first_choice`]).
    pub fn open_on(&mut self, project: &Project, shown: Option<TrackId>) {
        self.open = true;
        self.track = project.active().and_then(|seq| first_choice(seq, shown));
    }

    /// Looks for silence again (cheap: the waveforms are already read).
    /// `peaks` gives a clip's file peaks when they are ready. Returns false
    /// while some are still being read.
    pub fn refresh(
        &mut self,
        project: &Project,
        mut peaks: impl FnMut(&project::MediaRef, u32) -> Option<cache::Peaks>,
    ) -> bool {
        self.found.clear();
        let Some(seq) = project.active() else { return true };
        let track = self
            .track
            .and_then(|id| seq.audio_tracks.iter().find(|t| t.id == id))
            .or_else(|| seq.audio_tracks.front());
        let Some(track) = track else { return true };
        self.track = Some(track.id);
        let mut read = Vec::new();
        let mut nested = Vec::new();
        for clip in &track.clips {
            // A nested timeline's sound has no waveform of its own: it
            // counts as sound throughout, so it is never cut as silence.
            if clip.nested().is_some() {
                nested.push(clip);
                continue;
            }
            let Some(media) = project.media.get(&clip.media) else {
                continue;
            };
            match peaks(media, clip.stream) {
                Some(p) => read.push((clip, p)),
                None => return false,
            }
        }
        let clips: Vec<ClipSound> = read
            .iter()
            .map(|(c, p)| ClipSound {
                start: c.start.as_secs_f64(),
                duration: c.duration.as_secs_f64(),
                source_in: c.source_in.as_secs_f64(),
                peaks: &p.values,
                per_second: p.per_second,
                speed: c.speed.as_f64(),
                ramp: (!c.ramp.is_empty()).then_some(&c.ramp),
            })
            .chain(nested.iter().map(|c| ClipSound {
                start: c.start.as_secs_f64(),
                duration: c.duration.as_secs_f64(),
                source_in: 0.0,
                // One peak at full level, read wherever the clip is.
                peaks: &LOUD,
                per_second: 0,
                speed: 1.0,
                ramp: None,
            }))
            .collect();
        self.found = silence::find(&clips, self.settings);
        self.floor_db = silence::floor_db(&clips);
        true
    }

    /// The window. `ready` is false while waveforms are still being read.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        project: &Project,
        ready: bool,
        undo: &str,
    ) -> Asked {
        let Some(seq) = project.active() else {
            return Asked::Nothing;
        };
        let mut asked = Asked::Nothing;
        let mut open = self.open;
        egui::Window::new("Remove silence")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .max_width(320.0)
            .show(ctx, |ui| {
                // Long lines wrap; the window never grows wider.
                ui.set_max_width(320.0);
                if seq.audio_tracks.is_empty() {
                    ui.label(egui::RichText::new("There is no sound track to listen to.").color(theme.muted));
                    return;
                }
                let name = |id: Option<TrackId>| {
                    seq.audio_tracks
                        .iter()
                        .find(|t| Some(t.id) == id)
                        .map_or_else(String::new, |t| t.name.clone())
                };
                egui::ComboBox::from_label("Listen to")
                    .selected_text(name(self.track))
                    .show_ui(ui, |ui| {
                        for t in &seq.audio_tracks {
                            ui.selectable_value(&mut self.track, Some(t.id), &t.name);
                        }
                    });
                ui.add_space(4.0);
                let usual = Settings::default();
                let mut threshold = f64::from(self.settings.threshold_db);
                inspector::slider(
                    ui,
                    theme,
                    "Quieter than",
                    &mut threshold,
                    Number {
                        range: (-70.0, -20.0),
                        per_point: 0.2,
                        decimals: 0,
                        suffix: " dB",
                        usual: f64::from(usual.threshold_db),
                    },
                );
                self.settings.threshold_db = threshold as f32;
                inspector::slider(
                    ui,
                    theme,
                    "For at least",
                    &mut self.settings.min_length,
                    Number {
                        range: (0.3, 5.0),
                        per_point: 0.02,
                        decimals: 1,
                        suffix: " s",
                        usual: usual.min_length,
                    },
                );
                inspector::slider(
                    ui,
                    theme,
                    "Keep at each edge",
                    &mut self.settings.keep,
                    Number {
                        range: (0.0, 0.5),
                        per_point: 0.005,
                        decimals: 2,
                        suffix: " s",
                        usual: usual.keep,
                    },
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "It listens to the recording as it is, before the track's effects: hiss that Clean-up takes out still counts as sound here.",
                    )
                    .size(11.5)
                    .color(theme.muted),
                );
                if let Some(floor) = self.floor_db.filter(|f| *f > self.settings.threshold_db - 3.0) {
                    let better = silence::suggested_threshold(floor);
                    ui.label(
                        egui::RichText::new(format!(
                            "Its quietest moments are about {floor:.0} dB (its hiss), so pauses are louder than {:.0} dB.",
                            self.settings.threshold_db
                        ))
                        .size(11.5)
                        .color(ui_kit::theme::status::WARNING),
                    );
                    if better > self.settings.threshold_db
                        && ui.small_button(format!("Use {better:.0} dB")).clicked()
                    {
                        self.settings.threshold_db = better;
                    }
                }
                ui.add_space(6.0);
                if !ready {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the sound…");
                    });
                    return;
                }
                let text = match self.found.len() {
                    0 => "No silence found with these settings.".to_owned(),
                    n => format!(
                        "{n} quiet stretch{} found, {:.1} s in all. They are marked red on the timeline.",
                        if n == 1 { "" } else { "es" },
                        silence::total(&self.found)
                    ),
                };
                ui.label(egui::RichText::new(text).color(theme.text_secondary));
                ui.label(
                    egui::RichText::new(format!(
                        "Removing cuts them out of every track and closes the gaps. {undo} undoes it."
                    ))
                    .size(11.5)
                    .color(theme.muted),
                );
                ui.add_space(6.0);
                if !self.found.is_empty() && ui_kit::accent_button(ui, theme, "Remove them").clicked() {
                    asked = Asked::Remove(regions(&self.found, seq.settings.frame_rate));
                }
            });
        self.open = open && matches!(asked, Asked::Nothing);
        if !self.open {
            self.found.clear();
        }
        asked
    }
}

/// The stretches on whole frames; any that round to nothing are dropped.
pub fn regions(found: &[(f64, f64)], rate: Rate) -> Vec<Region> {
    let at = |s: f64| {
        rate.snap(Time::from_ratio((s * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO))
    };
    found
        .iter()
        .filter_map(|&(a, b)| Region::between(at(a), at(b)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The microphone (the Dialogue track) is listened to first, even
    /// when the first track is another.
    #[test]
    fn it_starts_on_the_dialogue_track() {
        let p = Project::new_automatic("T");
        let seq_id = p.active_sequence;
        let p = project::Command::AddTrack {
            sequence: seq_id,
            kind: project::TrackKind::Audio,
            name: "Mic".into(),
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let (first, mic) = (seq.audio_tracks[0].id, seq.audio_tracks[1].id);
        assert_eq!(first_choice(seq, None), Some(first), "nothing better");
        assert_eq!(first_choice(seq, Some(mic)), Some(mic), "the track shown");
        let mut sound = seq.audio_tracks[1].sound.clone();
        sound.set_role(TrackRole::Dialogue);
        let p = project::Command::SetTrackSound {
            sequence: seq_id,
            track: mic,
            sound: Box::new(sound),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_choice(p.active().unwrap(), Some(first)), Some(mic));
    }

    #[test]
    fn stretches_land_on_frames() {
        let r = regions(&[(1.004, 2.0), (3.0, 3.001)], Rate::FPS_60);
        assert_eq!(r.len(), 1, "the second rounds to nothing");
        assert_eq!(r[0].start, Rate::FPS_60.snap(Time::from_ratio(1, 1).unwrap()));
    }
}
