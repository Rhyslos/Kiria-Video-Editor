//! "Remove silence": pick a sound track and how quiet counts as silent; the
//! stretches found show on the timeline, and one click cuts them all out of
//! every track as a single undoable step.

use engine::silence::{self, ClipSound, Settings};
use project::{Project, Rate, Region, Time, TrackId};
use ui_kit::Theme;

#[derive(Default)]
pub struct SilencePanel {
    pub open: bool,
    track: Option<TrackId>,
    settings: Settings,
    /// The stretches found with the current choices (seconds).
    pub found: Vec<(f64, f64)>,
}

/// What the window asked for this frame.
pub enum Asked {
    Nothing,
    Remove(Vec<Region>),
}

impl SilencePanel {
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
        for clip in &track.clips {
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
            })
            .collect();
        self.found = silence::find(&clips, self.settings);
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
            .show(ctx, |ui| {
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
                ui.add(
                    egui::Slider::new(&mut self.settings.threshold_db, -70.0..=-20.0)
                        .suffix(" dB")
                        .text("Quieter than"),
                );
                ui.add(
                    egui::Slider::new(&mut self.settings.min_length, 0.3..=5.0)
                        .suffix(" s")
                        .text("For at least"),
                );
                ui.add(
                    egui::Slider::new(&mut self.settings.keep, 0.0..=0.5)
                        .suffix(" s")
                        .text("Keep at each edge"),
                );
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

    #[test]
    fn stretches_land_on_frames() {
        let r = regions(&[(1.004, 2.0), (3.0, 3.001)], Rate::FPS_60);
        assert_eq!(r.len(), 1, "the second rounds to nothing");
        assert_eq!(r[0].start, Rate::FPS_60.snap(Time::from_ratio(1, 1).unwrap()));
    }
}
