//! The inspector's Audio section: the sound of the clip shown (itself on
//! an audio track, its linked sound clips for a picture), with volume, pan
//! and fades written and read in plain words ("−3.0 dB", "L 20", "0.50 s ·
//! 30 frames"). A change is shown at once (`Inspector::shown`, and on the
//! timeline through `Inspector::sounds_being_set`) and becomes one
//! `SetSound` for all those clips when the hand is off it.

use project::{ClipId, Command, Project, Rate, Sound, Time};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number, Touched, Words};

use super::Inspector;

impl Inspector {
    /// The Audio section: the sound of `clip` (itself on an audio track,
    /// its linked sound clips for a picture). A change is shown (and seen
    /// on the timeline) at once and becomes one undo step for all of those
    /// clips when the hand is off it. Returns that step when it is done.
    pub(super) fn audio_section(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        seq: &project::Sequence,
        clip: &project::Clip,
    ) -> Option<Command> {
        let targets = sound_clips(seq, clip);
        let first = targets.first()?;
        let rate = seq.settings.frame_rate;
        let pending = self.pending_sound.get();
        let from_timeline = self.volume_from_timeline;
        let now = |c: &project::Clip| {
            let sound = pending
                .and_then(|(clips, sounds)| sounds.get(clips.iter().position(|id| *id == c.id)?))
                .map_or(c.sound, |sound| *sound);
            match from_timeline {
                Some((id, gain_mdb)) if id == c.id => Sound { gain_mdb, ..sound },
                _ => sound,
            }
        };
        let before = now(first);
        let length = first.duration;
        let mut values = SoundValues::of(&before);
        let mut touched = Touched::default();
        let clicks = inspector::section(
            ui,
            theme,
            "Audio",
            &sound_summary(&before),
            before != Sound::PLAIN,
            |ui| {
                let volume_words = Words {
                    write: &|db| volume_text(db),
                    read: &|text| read_volume(text),
                };
                touched = touched.and(inspector::slider_in_words(
                    ui,
                    theme,
                    "Volume",
                    &mut values.volume,
                    VOLUME,
                    volume_words,
                ));
                let pan_words = Words {
                    write: &|pan| pan_text(pan),
                    read: &|text| read_pan(text),
                };
                touched = touched.and(inspector::slider_in_words(
                    ui,
                    theme,
                    "Pan",
                    &mut values.pan,
                    PAN,
                    pan_words,
                ));
                let fade_words = Words {
                    write: &|secs| fade_text(secs, rate),
                    read: &|text| read_seconds(text),
                };
                let whole = length.as_secs_f64();
                let fade_in_room = (whole - values.fade_out).max(0.0);
                touched = touched.and(inspector::number_in_words(
                    ui,
                    theme,
                    "Fade in",
                    &mut values.fade_in,
                    fade_number(fade_in_room),
                    fade_words,
                ));
                let fade_out_room = (whole - values.fade_in).max(0.0);
                touched = touched.and(inspector::number_in_words(
                    ui,
                    theme,
                    "Fade out",
                    &mut values.fade_out,
                    fade_number(fade_out_room),
                    fade_words,
                ));
                if targets.len() > 1 {
                    ui.label(
                        egui::RichText::new(format!(
                            "Sets all {} sound streams of this clip together.",
                            targets.len()
                        ))
                        .color(theme.muted)
                        .size(12.0),
                    );
                }
            },
        );
        if clicks.reset {
            values = SoundValues::of(&Sound::PLAIN);
            touched.changed = true;
        }
        if touched.changed {
            let was = SoundValues::of(&before);
            let sounds = targets
                .iter()
                .map(|c| values.sound(&was, now(c), rate, c.duration))
                .collect();
            self.pending_sound
                .set(targets.iter().map(|c| c.id).collect(), sounds);
        }
        let (clips, sounds) = self
            .pending_sound
            .finish_if_released(touched.busy, |clips| sounds_in(seq, clips))?;
        Some(Command::SetSound {
            sequence: seq.id,
            sounds: clips.into_iter().zip(sounds).collect(),
        })
    }
}

/// The sound each of `clips` has on the open timeline; `None` when one is gone.
pub(super) fn stored_sounds(project: &Project, clips: &[ClipId]) -> Option<Vec<Sound>> {
    sounds_in(project.active()?, clips)
}

/// The sound each of `clips` has on `seq`; `None` when one is gone.
fn sounds_in(seq: &project::Sequence, clips: &[ClipId]) -> Option<Vec<Sound>> {
    clips
        .iter()
        .map(|id| {
            seq.audio_tracks
                .iter()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.id == *id)
                .map(|clip| clip.sound)
        })
        .collect()
}

/// The sound clips the Audio section sets for `clip`: itself when it is on
/// an audio track, else the sound clips linked to it (none for a picture
/// without sound).
pub(super) fn sound_clips<'a>(seq: &'a project::Sequence, clip: &project::Clip) -> Vec<&'a project::Clip> {
    let on_audio = |c: &project::Clip| c.id == clip.id || (clip.link.is_some() && c.link == clip.link);
    let own = seq
        .audio_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip.id);
    match own {
        Some(own) => vec![own],
        None => seq
            .audio_tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| on_audio(c))
            .collect(),
    }
}

/// A clip's sound as the inspector shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SoundValues {
    /// Decibels; the bottom of the range is silent.
    volume: f64,
    /// -100 (all left) to 100 (all right).
    pan: f64,
    /// Seconds.
    fade_in: f64,
    fade_out: f64,
}

const VOLUME: Number = Number {
    range: (Sound::SILENT as f64 / 1000.0, Sound::LOUDEST as f64 / 1000.0),
    per_point: 0.1,
    decimals: 1,
    suffix: "",
    usual: 0.0,
};
const PAN: Number = Number {
    range: (-100.0, 100.0),
    per_point: 1.0,
    decimals: 0,
    suffix: "",
    usual: 0.0,
};

/// A fade's number: up to `room` seconds, a hundredth at a time.
fn fade_number(room: f64) -> Number {
    Number {
        range: (0.0, room),
        per_point: 0.01,
        decimals: 2,
        suffix: "",
        usual: 0.0,
    }
}

impl SoundValues {
    fn of(sound: &Sound) -> SoundValues {
        SoundValues {
            volume: f64::from(sound.gain_mdb) / 1000.0,
            pan: f64::from(sound.pan) / f64::from(Sound::SIDE) * 100.0,
            fade_in: sound.fade_in.as_secs_f64(),
            fade_out: sound.fade_out.as_secs_f64(),
        }
    }

    /// `stored` with the numbers that differ from `was` set to these: a
    /// changed fade on a whole frame of `rate`, and everything within its
    /// range on a clip `length` long. What wasn't touched keeps `stored`'s
    /// exact value, so linked sound clips set together keep what differs
    /// between them in what wasn't changed.
    fn sound(&self, was: &SoundValues, stored: Sound, rate: Rate, length: Time) -> Sound {
        let on_frame = |secs: f64| rate.frame_start((secs.max(0.0) * rate.as_f64()).round() as i64);
        let mut sound = stored;
        if self.volume != was.volume {
            sound.gain_mdb = (self.volume * 1000.0).round() as i32;
        }
        if self.pan != was.pan {
            sound.pan = (self.pan / 100.0 * f64::from(Sound::SIDE)).round() as i32;
        }
        if self.fade_in != was.fade_in {
            sound.fade_in = on_frame(self.fade_in).min(length - sound.fade_out.min(length));
        }
        if self.fade_out != was.fade_out {
            sound.fade_out = on_frame(self.fade_out).min(length - sound.fade_in.min(length));
        }
        sound.within_limits().fitted(length)
    }
}

/// "−3.0 dB", "+2.5 dB", "0.0 dB", or "Off" at the bottom of the range.
fn volume_text(db: f64) -> String {
    // Written as the timeline's level line writes it.
    ui_kit::timeline::level::words(db)
}

/// What was typed as a volume: "-3", "-3 dB", "+2.5dB", or "off".
fn read_volume(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("off") || text.eq_ignore_ascii_case("silent") {
        return Some(f64::from(Sound::SILENT) / 1000.0);
    }
    let number = text.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c.is_whitespace());
    number.replace('−', "-").trim().parse().ok()
}

/// "C" in the middle, "L 20" or "R 20" to a side.
fn pan_text(pan: f64) -> String {
    let amount = pan.abs().round();
    if amount == 0.0 {
        "C".to_owned()
    } else if pan < 0.0 {
        format!("L {amount}")
    } else {
        format!("R {amount}")
    }
}

/// What was typed as a pan: "C", "L 20", "R20", or a number (negative to
/// the left).
fn read_pan(text: &str) -> Option<f64> {
    let text = text.trim().replace('−', "-");
    let (sign, rest) = match text.chars().next()? {
        'l' | 'L' => (-1.0, &text[1..]),
        'r' | 'R' => (1.0, &text[1..]),
        'c' | 'C' => return Some(0.0),
        _ => (1.0, text.as_str()),
    };
    let rest = rest.trim();
    if rest.is_empty() {
        // "L" alone is all the way.
        return Some(sign * 100.0);
    }
    rest.parse::<f64>().ok().map(|n| sign * n)
}

/// "0.50 s · 30 frames": a fade's length, and in frames of the timeline.
fn fade_text(seconds: f64, rate: Rate) -> String {
    let frames = (seconds * rate.as_f64()).round() as i64;
    if frames == 0 {
        return "None".to_owned();
    }
    let unit = if frames == 1 { "frame" } else { "frames" };
    format!("{seconds:.2} s · {frames} {unit}")
}

/// What was typed as a length in seconds: "0.5", "0.5 s", "none".
fn read_seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
    let number = text.split('·').next()?.trim().trim_end_matches('s').trim();
    number.parse().ok()
}

/// The Audio header's line: "−3.0 dB · pan L 20 · fade in 0.5 s".
fn sound_summary(sound: &Sound) -> String {
    if sound.is_silent() {
        return "Silent".to_owned();
    }
    let values = SoundValues::of(sound);
    let mut parts = Vec::new();
    if sound.gain_mdb != 0 {
        parts.push(volume_text(values.volume));
    }
    if sound.pan != 0 {
        parts.push(format!("pan {}", pan_text(values.pan)));
    }
    for (name, length) in [("fade in", values.fade_in), ("fade out", values.fade_out)] {
        if length > 0.0 {
            parts.push(format!(
                "{name} {}",
                ui_kit::time_text::length(length).replace(".0 s", " s")
            ));
        }
    }
    if parts.is_empty() {
        "As recorded".to_owned()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_number_sets_that_setting_alone_on_each_stream() {
        let rate = Rate::FPS_60;
        let length = Time::seconds(10);
        let mic = Sound {
            gain_mdb: -9_000,
            pan: 300_000,
            fade_out: Time::seconds(2),
            ..Sound::PLAIN
        };
        let was = SoundValues::of(&Sound::PLAIN);
        let mut now = was;
        now.fade_in = 0.509;
        let set = now.sound(&was, mic, rate, length);
        assert_eq!(set.fade_in, rate.frame_start(31), "on a whole frame");
        assert_eq!(
            (set.gain_mdb, set.pan, set.fade_out),
            (mic.gain_mdb, mic.pan, mic.fade_out)
        );
        // Too long for what the fade out leaves: cut to fit.
        now.fade_in = 9.0;
        assert_eq!(now.sound(&was, mic, rate, length).fade_in, Time::seconds(8));
        let mut louder = was;
        louder.volume = 2.54;
        louder.pan = -100.0;
        let set = louder.sound(&was, mic, rate, length);
        assert_eq!((set.gain_mdb, set.pan), (2_540, -Sound::SIDE));
        // Shown and read back, nothing changes.
        assert_eq!(
            SoundValues::of(&mic).sound(&SoundValues::of(&mic), mic, rate, length),
            mic
        );
    }

    #[test]
    fn sound_is_written_and_read_in_plain_words() {
        assert_eq!(volume_text(-3.0), "−3.0 dB");
        assert_eq!(volume_text(2.54), "+2.5 dB");
        assert_eq!(volume_text(0.0), "0.0 dB");
        assert_eq!(volume_text(-60.0), "Off");
        assert_eq!(read_volume("-3 dB"), Some(-3.0));
        assert_eq!(read_volume("−4.5"), Some(-4.5));
        assert_eq!(read_volume("off"), Some(-60.0));
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-20.0), "L 20");
        assert_eq!(pan_text(100.0), "R 100");
        assert_eq!(read_pan("L 20"), Some(-20.0));
        assert_eq!(read_pan("r35"), Some(35.0));
        assert_eq!(read_pan("C"), Some(0.0));
        assert_eq!(read_pan("-40"), Some(-40.0));
        assert_eq!(read_pan("L"), Some(-100.0));
        assert_eq!(fade_text(0.5, Rate::FPS_60), "0.50 s · 30 frames");
        assert_eq!(fade_text(0.0, Rate::FPS_60), "None");
        assert_eq!(read_seconds("0.75 s · 45 frames"), Some(0.75));
        assert_eq!(read_seconds("1.2"), Some(1.2));
        assert_eq!(read_seconds("none"), Some(0.0));
        assert_eq!(sound_summary(&Sound::PLAIN), "As recorded");
        let set = Sound {
            gain_mdb: -3_000,
            pan: -200_000,
            fade_in: Time::from_ratio(1, 2).unwrap(),
            fade_out: Time::seconds(1),
        };
        assert_eq!(
            sound_summary(&set),
            "−3.0 dB · pan L 20 · fade in 0.5 s · fade out 1 s"
        );
        assert_eq!(
            sound_summary(&Sound {
                gain_mdb: Sound::SILENT,
                ..set
            }),
            "Silent"
        );
    }
}
