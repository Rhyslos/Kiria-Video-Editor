//! How the Export page says things: sizes, times, progress, how an export
//! went, and how long one should take on this machine.

use engine::export::{Exported, Progress};
use media::encode::Place;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub fn megabytes_text(megabytes: f64) -> String {
    if megabytes >= 1000.0 {
        format!("{:.1} GB", megabytes / 1000.0)
    } else if megabytes >= 10.0 {
        format!("{megabytes:.0} MB")
    } else {
        format!("{megabytes:.1} MB")
    }
}

pub fn bytes_text(bytes: u64) -> String {
    megabytes_text(bytes as f64 / 1_000_000.0)
}

/// A number without a needless ".00" ("60", "59.94").
pub fn short(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A loudness aimed for: "-14", "-23", "-16.5".
pub fn target_text(lufs: f32) -> String {
    short(f64::from(lufs))
}

pub fn seconds_text(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds >= 3600 {
        format!("{} h {} min", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds} s")
    }
}

/// How an encoder is named for people.
pub fn encoder_text(name: &str) -> String {
    match engine::export::encoder_place(name) {
        Place::Processor => "encoded on the processor".to_owned(),
        Place::GraphicsCard | Place::System => format!("encoded on the graphics card ({name})"),
    }
}

/// How far an export is, in a line for the queue.
pub fn progress_text(progress: &Progress) -> String {
    let percent = (progress.share() * 100.0).floor();
    if progress.paused {
        return format!("Paused at {percent:.0}%");
    }
    if progress.measuring {
        return format!("Measuring the loudness first · {percent:.0}%");
    }
    match progress.time_left() {
        Some(left) => format!("{percent:.0}% · about {} left", seconds_text(left)),
        None if progress.frames_done == 0 => "Starting…".to_owned(),
        None => format!("{percent:.0}%"),
    }
}

/// How an export went, in a line for the queue: the size and how long it took.
pub fn done_line(done: &Exported, bytes: Option<u64>) -> String {
    let mut parts = vec!["Done".to_owned()];
    if let Some(bytes) = bytes {
        parts.push(bytes_text(bytes));
    }
    parts.push(format!("in {}", seconds_text(done.took)));
    parts.join(" · ")
}

/// How an export went, in full, for the tooltip of its row.
pub fn done_text(done: &Exported) -> String {
    let took = done.took.as_secs_f64().max(0.001);
    let beside = done.sound_beside.as_ref().map_or(String::new(), |wav| {
        format!(
            " The sound is also in {}.",
            wav.file_name()
                .map_or_else(|| wav.display().to_string(), |n| n.to_string_lossy().into_owned())
        )
    });
    format!(
        "Saved {} in {}, {:.1}× as fast as it plays, {}. {}{beside}",
        seconds_text(done.length),
        seconds_text(done.took),
        done.length.as_secs_f64() / took,
        encoder_text(&done.encoder),
        sound_text(done)
    )
}

/// What the exported sound measured: its integrated loudness and highest
/// true peak, with a word when the peak is over the usual ceiling.
pub fn sound_text(done: &Exported) -> String {
    let (loudness, true_peak) = (done.loudness, done.true_peak);
    if !loudness.is_finite() {
        return "The sound is silent.".to_owned();
    }
    let normalised = match done.normalised {
        Some(n) => format!(
            " (it measured {} LUFS and was turned {} by {} dB{})",
            ui_kit::master::db_text(n.measured),
            if n.gain_db >= 0.0 { "up" } else { "down" },
            ui_kit::master::db_text(n.gain_db.abs()),
            match done.limited_db {
                Some(db) if db < -0.05 =>
                    format!(", its peaks by up to {} dB more", ui_kit::master::db_text(-db)),
                _ => String::new(),
            }
        ),
        None => String::new(),
    };
    let over = if true_peak > ui_kit::master::TRUE_PEAK_CEILING {
        format!(
            " (over {:.0} dBTP: it may distort when played or encoded again)",
            ui_kit::master::TRUE_PEAK_CEILING
        )
    } else {
        String::new()
    };
    format!(
        "Loudness {} LUFS integrated{normalised}, true peak {} dBTP{over}.",
        ui_kit::master::db_text(loudness),
        ui_kit::master::db_text(true_peak)
    )
}

/// How fast exports went on this machine, kept in settings: the newest
/// export of each kind measured it. Pictures in millions of the file's
/// pixels a second; sound alone in seconds of sound a second.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pace {
    pub graphics_card: Option<f64>,
    pub processor: Option<f64>,
    pub sound: Option<f64>,
}

/// Guesses until an export measured this machine: about real time for
/// 1080p 60 on the processor, three times that on a graphics card, sound
/// alone sixty times as fast as it plays.
const GUESS_CARD: f64 = 375.0;
const GUESS_PROCESSOR: f64 = 125.0;
const GUESS_SOUND: f64 = 60.0;

/// How long an export should take on this machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub time: Duration,
    /// From an export measured here, not a guess.
    pub measured: bool,
}

/// What an export to estimate is like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Work {
    /// The pictures' size and how many there are (none for sound alone).
    pub pictures: Option<((u32, u32), u64)>,
    /// Encoded on the graphics card.
    pub on_card: bool,
    /// How long the sound is.
    pub length: Duration,
    /// Measured first for normalising: the sound is mixed twice.
    pub normalise: bool,
}

impl Pace {
    pub fn estimate(&self, work: Work) -> Estimate {
        match work.pictures {
            Some(((w, h), frames)) => {
                let (kept, guess) = if work.on_card {
                    (self.graphics_card, GUESS_CARD)
                } else {
                    (self.processor, GUESS_PROCESSOR)
                };
                let megapixels = f64::from(w) * f64::from(h) * frames as f64 / 1_000_000.0;
                let mut seconds = megapixels / kept.unwrap_or(guess).max(0.001);
                if work.normalise {
                    seconds += work.length.as_secs_f64() / self.sound.unwrap_or(GUESS_SOUND).max(0.001);
                }
                Estimate {
                    time: Duration::from_secs_f64(seconds),
                    measured: kept.is_some(),
                }
            }
            None => {
                let passes = if work.normalise { 2.0 } else { 1.0 };
                Estimate {
                    time: Duration::from_secs_f64(
                        work.length.as_secs_f64() * passes / self.sound.unwrap_or(GUESS_SOUND).max(0.001),
                    ),
                    measured: self.sound.is_some(),
                }
            }
        }
    }

    /// Takes in how fast an export went. Exports of a few seconds say
    /// little and are left out.
    pub fn learn(&mut self, done: &Exported) {
        let took = done.took.as_secs_f64();
        if took < 1.0 {
            return;
        }
        match done.size {
            Some((w, h)) => {
                let pace = f64::from(w) * f64::from(h) * done.frames as f64 / 1_000_000.0 / took;
                if done.on_graphics_card {
                    self.graphics_card = Some(pace);
                } else {
                    self.processor = Some(pace);
                }
            }
            None => {
                let passes = if done.normalised.is_some() { 2.0 } else { 1.0 };
                self.sound = Some(done.length.as_secs_f64() * passes / took);
            }
        }
    }
}

/// "about 1 min 40 s", "a few seconds".
pub fn about_time(time: Duration) -> String {
    if time < Duration::from_secs(5) {
        "a few seconds".to_owned()
    } else {
        format!("about {}", seconds_text(time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn exported() -> Exported {
        Exported {
            path: PathBuf::from("/a/b.mp4"),
            frames: 3600,
            took: Duration::from_secs(20),
            length: Duration::from_secs(60),
            size: Some((1920, 1080)),
            sound_beside: None,
            encoder: "libx264".to_owned(),
            audio_encoder: "aac".to_owned(),
            on_graphics_card: false,
            loudness: -14.04,
            true_peak: -1.52,
            normalised: None,
            limited_db: None,
        }
    }

    #[test]
    fn sizes_and_times_are_said_plainly() {
        assert_eq!(megabytes_text(914.4), "914 MB");
        assert_eq!(megabytes_text(2500.0), "2.5 GB");
        assert_eq!(megabytes_text(3.21), "3.2 MB");
        assert_eq!(short(60.0), "60");
        assert_eq!(short(59.94006), "59.94");
        assert_eq!(target_text(-14.0), "-14");
        assert_eq!(seconds_text(Duration::from_secs(42)), "42 s");
        assert_eq!(seconds_text(Duration::from_secs(135)), "2 min 15 s");
        assert_eq!(seconds_text(Duration::from_secs(3720)), "1 h 2 min");
        assert_eq!(about_time(Duration::from_secs(2)), "a few seconds");
        assert_eq!(about_time(Duration::from_secs(100)), "about 1 min 40 s");
    }

    #[test]
    fn progress_reads_as_a_line() {
        let progress = Progress {
            frames_done: 600,
            frames: 3600,
            elapsed: Duration::from_secs(5),
            encoder: Some("h264_amf".to_owned()),
            measuring: false,
            paused: false,
        };
        assert_eq!(progress_text(&progress), "16% · about 25 s left");
        let starting = Progress {
            frames_done: 0,
            elapsed: Duration::ZERO,
            ..progress.clone()
        };
        assert_eq!(progress_text(&starting), "Starting…");
        let paused = Progress {
            paused: true,
            ..progress.clone()
        };
        assert_eq!(progress_text(&paused), "Paused at 16%");
        let measuring = Progress {
            measuring: true,
            ..progress
        };
        assert_eq!(progress_text(&measuring), "Measuring the loudness first · 16%");
        assert_eq!(
            encoder_text("h264_amf"),
            "encoded on the graphics card (h264_amf)"
        );
    }

    #[test]
    fn how_an_export_went_is_said() {
        let done = exported();
        assert_eq!(done_line(&done, Some(240_000_000)), "Done · 240 MB · in 20 s");
        assert_eq!(
            done_text(&done),
            "Saved 1 min 0 s in 20 s, 3.0× as fast as it plays, encoded on the processor. Loudness -14.0 LUFS integrated, true peak -1.5 dBTP."
        );
        let silent = Exported {
            loudness: f32::NEG_INFINITY,
            true_peak: f32::NEG_INFINITY,
            ..done.clone()
        };
        assert_eq!(sound_text(&silent), "The sound is silent.");
        let hot = Exported {
            loudness: -9.0,
            true_peak: 0.4,
            ..done.clone()
        };
        assert!(sound_text(&hot).contains("over -1 dBTP"));
        let normalised = Exported {
            normalised: Some(engine::export::Normalised {
                measured: -20.0,
                gain_db: 6.0,
            }),
            limited_db: Some(-2.3),
            sound_beside: Some(PathBuf::from("/a/b.wav")),
            ..done
        };
        assert_eq!(
            done_text(&normalised),
            "Saved 1 min 0 s in 20 s, 3.0× as fast as it plays, encoded on the processor. Loudness -14.0 LUFS integrated (it measured -20.0 LUFS and was turned up by 6.0 dB, its peaks by up to 2.3 dB more), true peak -1.5 dBTP. The sound is also in b.wav."
        );
    }

    #[test]
    fn the_time_on_this_machine_is_guessed_then_measured() {
        let mut pace = Pace::default();
        let work = Work {
            pictures: Some(((1920, 1080), 3600)),
            on_card: false,
            length: Duration::from_secs(60),
            normalise: false,
        };
        let guess = pace.estimate(work);
        assert!(!guess.measured);
        // About real time for 1080p 60 on the processor.
        assert!((50..70).contains(&guess.time.as_secs()), "{guess:?}");
        pace.learn(&exported());
        let measured = pace.estimate(work);
        assert!(measured.measured);
        assert_eq!(measured.time.as_secs(), 20, "as fast as the last one went");
        assert!(
            !pace
                .estimate(Work {
                    on_card: true,
                    ..work
                })
                .measured,
            "the card is measured separately"
        );
        // Sound alone, measured twice over when normalising.
        let sound = Work {
            pictures: None,
            normalise: true,
            ..work
        };
        assert_eq!(pace.estimate(sound).time.as_secs(), 2);
        // A very short export says too little to learn from.
        let quick = Exported {
            took: Duration::from_millis(300),
            on_graphics_card: true,
            ..exported()
        };
        pace.learn(&quick);
        assert_eq!(pace.graphics_card, None);
    }
}
