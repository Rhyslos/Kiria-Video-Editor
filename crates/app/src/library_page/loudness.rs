//! A file's loudness and true peak, for the detail panel: its first sound
//! stream read once, start to end, on a worker and measured as the master
//! meter measures (`engine::loudness`: integrated loudness gated as the
//! loudness standard says, true peak from four-times oversampling). Kept
//! for the session; a file is measured only once someone looks at it.
//!
//! What to do about it (`Advice`): the change of level that brings it to
//! the loudness aimed for (Settings › Audio, the master column's target)
//! without its true peak going over -1 dBTP, which "Add to timeline" can
//! put on the clips it adds.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use media::AudioDecoder;
use project::{MediaId, MediaRef};
use std::collections::HashMap;
use std::path::Path;

/// The rate sound is read at to be measured.
const RATE: u32 = 48_000;

/// The highest true peak a file may reach after its level is changed.
pub const PEAK_CEILING: f32 = -1.0;

/// What was measured of a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Gated loudness of the whole file, LUFS (minus infinity for silence).
    pub integrated: f32,
    /// Its highest true peak, dBTP.
    pub true_peak: f32,
}

enum State {
    Measuring(JobHandle<Reading>),
    Done(Result<Reading, String>),
}

#[derive(Default)]
pub struct Measurements {
    files: HashMap<MediaId, State>,
}

impl Measurements {
    /// What is known of `media`'s loudness: `None` while it is measured
    /// (started now if nobody asked before), else the reading or why there
    /// is none. Files without sound have none.
    pub fn get(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
    ) -> Option<Result<Reading, String>> {
        let stream = media.info.audio.first()?.index;
        let state = self.files.entry(media.id).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            State::Measuring(jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "measure loudness",
                move |job| {
                    let result = measure(&path, stream, || job.is_cancelled());
                    ctx.request_repaint();
                    result
                },
            ))
        });
        if let State::Measuring(handle) = state
            && let Some(result) = handle.try_result()
        {
            *state = State::Done(result.map_err(|err| err.why_text().unwrap_or(err.what()).to_owned()));
        }
        match state {
            State::Measuring(_) => None,
            State::Done(result) => Some(result.clone()),
        }
    }

    /// Whether `media` is being measured now.
    pub fn measuring(&self, media: MediaId) -> bool {
        matches!(self.files.get(&media), Some(State::Measuring(_)))
    }

    /// Measures the file again next time (it was found somewhere else).
    pub fn forget(&mut self, media: MediaId) {
        if let Some(State::Measuring(handle)) = self.files.remove(&media) {
            handle.cancel();
        }
    }

    /// The app is closing: measurements under way stop.
    pub fn stop(&self) {
        for state in self.files.values() {
            if let State::Measuring(handle) = state {
                handle.cancel();
            }
        }
    }
}

/// Reads sound stream `stream` of the file from start to end and measures
/// it. Slow (the whole stream is decoded): run it on a worker. `cancelled`
/// is asked between blocks.
pub fn measure(path: &Path, stream: u32, cancelled: impl Fn() -> bool) -> diag::Result<Reading> {
    let mut decoder = AudioDecoder::open(path, stream, RATE)?;
    decoder.seek(0.0);
    let mut meter = engine::loudness::Loudness::new(RATE);
    // A tenth of a second at a time, in stereo.
    let mut block = vec![0f32; (RATE / 10) as usize * 2];
    loop {
        if cancelled() {
            return Err(diag::AppError::new("Measuring the loudness stopped").why("It was no longer needed."));
        }
        let frames = decoder.read(&mut block);
        if frames == 0 {
            break;
        }
        meter.feed(&block[..frames * 2]);
        if frames * 2 < block.len() {
            break;
        }
    }
    let measured = meter.measured();
    Ok(Reading {
        integrated: measured.integrated,
        true_peak: measured.true_peak_max,
    })
}

/// What the detail panel says about a reading, and the change of level it offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Advice {
    /// The problem in a sentence, if there is one.
    pub problem: Option<String>,
    /// The change in thousandths of a decibel that fixes it, with the words
    /// for its button, if a change helps.
    pub fix: Option<(i32, String)>,
}

/// Advice for `reading` when the loudness aimed for is `target` LUFS (none
/// when no loudness is aimed for).
pub fn advice(reading: Reading, target: Option<f32>) -> Advice {
    if !reading.integrated.is_finite() {
        return Advice {
            problem: Some("It is silent, or too short to measure.".into()),
            fix: None,
        };
    }
    let peak_room = PEAK_CEILING - reading.true_peak;
    // Towards the target, but never pushing the peaks over the ceiling.
    let wanted = target.map_or(peak_room.min(0.0), |t| (t - reading.integrated).min(peak_room));
    let too_loud_peaks = reading.true_peak > PEAK_CEILING;
    let problem = if too_loud_peaks {
        Some(format!(
            "Its peaks reach {:.1} dBTP: they may clip once the video is encoded.",
            reading.true_peak
        ))
    } else {
        target.filter(|t| (t - reading.integrated).abs() >= 1.0).map(|t| {
            let way = if reading.integrated < t {
                "quieter"
            } else {
                "louder"
            };
            format!(
                "It is {:.1} LU {way} than the {t:.0} LUFS aimed for.",
                (t - reading.integrated).abs()
            )
        })
    };
    // Changes under half a decibel are not worth a button.
    let fix = (wanted.abs() >= 0.5 && problem.is_some()).then(|| {
        let mdb = (wanted * 1000.0).round() as i32;
        let after = reading.integrated + wanted;
        let words = if wanted > 0.0 {
            format!("Add it {wanted:.1} dB louder ({after:.0} LUFS)")
        } else {
            format!("Add it {:.1} dB quieter ({after:.0} LUFS)", -wanted)
        };
        (mdb, words)
    });
    Advice { problem, fix }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_file_is_measured() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/voice.mp3");
        let probed = media::probe(&fixture).unwrap();
        let stream = probed.info.audio[0].index;
        let reading = measure(&fixture, stream, || false).unwrap();
        assert!(
            reading.true_peak.is_finite() && reading.true_peak < 3.0,
            "{reading:?}"
        );
        assert!(
            measure(&fixture, stream, || true).is_err(),
            "stopped when no longer needed"
        );
        assert!(measure(Path::new("/nowhere/at/all.wav"), 0, || false).is_err());
    }

    #[test]
    fn quiet_files_are_raised_as_far_as_the_peaks_allow() {
        let quiet = Reading {
            integrated: -20.0,
            true_peak: -9.0,
        };
        let a = advice(quiet, Some(-14.0));
        assert!(a.problem.as_deref().is_some_and(|p| p.contains("6.0 LU quieter")));
        assert_eq!(a.fix.as_ref().map(|f| f.0), Some(6_000));
        // Peaks at -4 leave only 3 dB of room under -1 dBTP.
        let peaky = Reading {
            integrated: -20.0,
            true_peak: -4.0,
        };
        assert_eq!(advice(peaky, Some(-14.0)).fix.map(|f| f.0), Some(3_000));
    }

    #[test]
    fn peaks_over_the_ceiling_are_lowered() {
        let hot = Reading {
            integrated: -14.0,
            true_peak: 0.4,
        };
        let a = advice(hot, Some(-14.0));
        assert!(a.problem.as_deref().is_some_and(|p| p.contains("0.4 dBTP")));
        let (mdb, words) = a.fix.unwrap();
        assert_eq!(mdb, -1_400);
        assert!(words.contains("quieter"), "{words}");
        // With no loudness aimed for, only the peaks matter.
        assert_eq!(advice(hot, None).fix.map(|f| f.0), Some(-1_400));
    }

    #[test]
    fn files_near_the_target_need_nothing() {
        let fine = Reading {
            integrated: -14.4,
            true_peak: -2.0,
        };
        assert_eq!(
            advice(fine, Some(-14.0)),
            Advice {
                problem: None,
                fix: None
            }
        );
        let silent = Reading {
            integrated: f32::NEG_INFINITY,
            true_peak: f32::NEG_INFINITY,
        };
        assert!(advice(silent, Some(-14.0)).fix.is_none());
    }
}
