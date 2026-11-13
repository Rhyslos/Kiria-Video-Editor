//! A file's loudness and true peak, for the detail panel: each of its
//! sound streams (game, microphone, chat…) read once, start to end, on a
//! worker and measured as the master meter measures (`engine::loudness`:
//! integrated loudness gated as the loudness standard says, true peak from
//! four-times oversampling). Every sound stream of a file is measured in
//! one pass over it, once someone looks at it, and the readings are kept
//! in the cache folder's `loudness/` by the file's fingerprint, so a file
//! is measured once, not once a session. A stream of one channel (a microphone) is
//! measured as one channel, as the loudness standard says: its peak is its
//! own samples', not those of the two channels it is spread over to play.
//!
//! What to do about it (`Advice`), stream by stream: the change of level
//! that brings it to the loudness aimed for (Settings › Audio, the master
//! column's target) without its true peak going over -1 dBTP, which "Add
//! to timeline" puts on that stream's clips when it adds them.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, MediaRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The rate sound is read at to be measured.
const RATE: u32 = 48_000;

/// The highest true peak a file may reach after its level is changed.
pub const PEAK_CEILING: f32 = -1.0;

/// What was measured of a sound stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Gated loudness of the whole stream, LUFS (minus infinity for silence).
    pub integrated: f32,
    /// Its highest true peak, dBTP.
    pub true_peak: f32,
}

/// What is known of one file's streams.
enum File {
    Measuring(JobHandle<Vec<(u32, Reading)>>),
    Done(Result<Vec<(u32, Reading)>, String>),
}

#[derive(Default)]
pub struct Measurements {
    /// By file.
    files: HashMap<MediaId, File>,
    /// Where readings are kept between sessions, by the file's fingerprint
    /// (the cache folder's `loudness/`); none in tests.
    kept: Option<PathBuf>,
}

impl Measurements {
    /// Readings kept in `folder` (made when the first is written).
    pub fn keeping_in(folder: PathBuf) -> Measurements {
        Measurements {
            files: HashMap::new(),
            kept: Some(folder),
        }
    }

    /// What is known of the loudness of `media`'s sound stream `stream`:
    /// `None` while the file is measured (started now if nobody asked
    /// before: every sound stream of it in one pass, or read back from
    /// what an earlier session kept), else the reading or why there is none.
    pub fn get(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> Option<Result<Reading, String>> {
        let kept = self.kept.clone();
        let file = self.files.entry(media.id).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let streams: Vec<(u32, u16)> = media.info.audio.iter().map(|a| (a.index, a.channels)).collect();
            let fingerprint = media.fingerprint.clone();
            File::Measuring(jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "measure loudness",
                move |job| {
                    let kept = kept.map(|folder| folder.join(kept_name(&fingerprint)));
                    if let Some(readings) = kept.as_deref().and_then(|file| load(file, &streams)) {
                        ctx.request_repaint();
                        return Ok(readings);
                    }
                    let result = measure_streams(&path, &streams, || job.is_cancelled());
                    if let (Ok(readings), Some(file)) = (&result, &kept) {
                        save(file, &streams, readings);
                    }
                    ctx.request_repaint();
                    result.map(|r| streams.iter().map(|s| s.0).zip(r).collect())
                },
            ))
        });
        if let File::Measuring(handle) = file
            && let Some(result) = handle.try_result()
        {
            *file = File::Done(result.map_err(|err| err.why_text().unwrap_or(err.what()).to_owned()));
        }
        match file {
            File::Measuring(_) => None,
            File::Done(Ok(readings)) => Some(
                readings
                    .iter()
                    .find(|(s, _)| *s == stream)
                    .map(|(_, r)| *r)
                    .ok_or_else(|| "It has no such sound stream.".to_owned()),
            ),
            File::Done(Err(why)) => Some(Err(why.clone())),
        }
    }

    /// Whether `media` is being measured now.
    pub fn measuring(&self, media: MediaId) -> bool {
        matches!(self.files.get(&media), Some(File::Measuring(_)))
    }

    /// Measures the file again next time (it was found somewhere else).
    pub fn forget(&mut self, media: MediaId) {
        if let Some(File::Measuring(handle)) = self.files.remove(&media) {
            handle.cancel();
        }
    }

    /// The app is closing: measurements under way stop.
    pub fn stop(&self) {
        for file in self.files.values() {
            if let File::Measuring(handle) = file {
                handle.cancel();
            }
        }
    }
}

/// The name a file's readings are kept under: its fingerprint, as its
/// proxy's and waveforms' are.
fn kept_name(fingerprint: &project::Fingerprint) -> String {
    format!("{}-{}.txt", fingerprint.size, fingerprint.sample_hash)
}

/// The first line of a kept file: what it is and its format's version.
const KEPT_HEADER: &str = "loudness 1";

/// The readings kept in `file` for `streams`, when it holds every one.
fn load(file: &Path, streams: &[(u32, u16)]) -> Option<Vec<(u32, Reading)>> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut lines = text.lines();
    if lines.next()? != KEPT_HEADER {
        return None;
    }
    let mut kept = HashMap::new();
    for line in lines {
        let mut parts = line.split_whitespace();
        let stream: u32 = parts.next()?.parse().ok()?;
        let integrated: f32 = parts.next()?.parse().ok()?;
        let true_peak: f32 = parts.next()?.parse().ok()?;
        kept.insert(
            stream,
            Reading {
                integrated,
                true_peak,
            },
        );
    }
    streams.iter().map(|(s, _)| Some((*s, *kept.get(s)?))).collect()
}

/// Keeps the readings of `streams` in `file`, whole or not at all; a
/// failure only means measuring again next session.
fn save(file: &Path, streams: &[(u32, u16)], readings: &[Reading]) {
    let mut text = format!("{KEPT_HEADER}\n");
    for ((stream, _), r) in streams.iter().zip(readings) {
        text.push_str(&format!("{stream} {} {}\n", r.integrated, r.true_peak));
    }
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(|e| e.to_string())
        .and_then(|()| project::disk::write_atomic(file, text.as_bytes()).map_err(|e| e.to_string()));
    if let Err(why) = written {
        tracing::info!("a file's loudness was not kept: {why}");
    }
}

/// Reads every one of `streams` (number, channels) of the file from start
/// to end, in one pass, and measures each. Slow (the whole file is read):
/// run it on a worker. `cancelled` is asked between blocks.
pub fn measure_streams(
    path: &Path,
    streams: &[(u32, u16)],
    cancelled: impl Fn() -> bool,
) -> diag::Result<Vec<Reading>> {
    let numbers: Vec<u32> = streams.iter().map(|s| s.0).collect();
    let mut reader = media::AudioStreams::open(path, &numbers, RATE)?;
    let mut meters: Vec<(bool, engine::loudness::Loudness)> = streams
        .iter()
        .map(|&(_, channels)| {
            let mono = channels == 1;
            let meter = if mono {
                engine::loudness::Loudness::mono(RATE)
            } else {
                engine::loudness::Loudness::new(RATE)
            };
            (mono, meter)
        })
        .collect();
    let mut out = vec![Vec::new(); streams.len()];
    let mut one = Vec::new();
    loop {
        if cancelled() {
            return Err(diag::AppError::new("Measuring the loudness stopped").why("It was no longer needed."));
        }
        // A tenth of a second at a time, in stereo.
        let going = reader.read((RATE / 10) as usize, &mut out);
        for ((mono, meter), samples) in meters.iter_mut().zip(&mut out) {
            if *mono {
                // The decoder spreads one channel over two at -3 dB each
                // (the same power, as it is played); the channel itself is
                // either of them brought back up.
                one.clear();
                one.extend(
                    samples
                        .chunks_exact(2)
                        .map(|pair| pair[0] * std::f32::consts::SQRT_2),
                );
                meter.feed(&one);
            } else {
                meter.feed(samples);
            }
            samples.clear();
        }
        if !going {
            break;
        }
    }
    Ok(meters
        .iter()
        .map(|(_, meter)| {
            let measured = meter.measured();
            Reading {
                integrated: measured.integrated,
                true_peak: measured.true_peak_max,
            }
        })
        .collect())
}

/// Reads sound stream `stream` (of `channels` channels) of the file from
/// start to end and measures it (see [`measure_streams`]).
#[cfg(test)]
pub fn measure(path: &Path, stream: (u32, u16), cancelled: impl Fn() -> bool) -> diag::Result<Reading> {
    measure_streams(path, &[stream], cancelled)?
        .pop()
        .ok_or_else(|| diag::AppError::new("The loudness can't be measured").why("No sound stream was read."))
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

    /// What `measurements` says of every sound stream of `media`, waited for.
    fn every_stream(measurements: &mut Measurements, jobs: &JobSystem, media: &MediaRef) -> Vec<Reading> {
        let ctx = egui::Context::default();
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(60);
        media
            .info
            .audio
            .iter()
            .map(|a| {
                loop {
                    if let Some(result) = measurements.get(jobs, &ctx, media, a.index) {
                        break result.unwrap();
                    }
                    assert!(std::time::Instant::now() < give_up, "measured in time");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            })
            .collect()
    }

    #[test]
    fn every_stream_is_measured_in_one_pass_and_kept_for_the_next_session() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let media = media::describe(&fixture).unwrap();
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("loudness");
        // Each stream as it reads alone.
        let alone: Vec<Reading> = media
            .info
            .audio
            .iter()
            .map(|a| measure(&fixture, (a.index, a.channels), || false).unwrap())
            .collect();
        let first = every_stream(&mut Measurements::keeping_in(folder.clone()), &jobs, &media);
        assert_eq!(first, alone, "read together, each as it reads alone");
        assert!(folder.join(kept_name(&media.fingerprint)).is_file(), "kept");
        // The next session reads them back: even a file that is gone.
        let mut gone = media.clone();
        gone.path = dir.path().join("moved away.mkv");
        let again = every_stream(&mut Measurements::keeping_in(folder.clone()), &jobs, &gone);
        assert_eq!(again, first);
        // Another file (another fingerprint) is measured afresh.
        let mut other = gone.clone();
        other.fingerprint.sample_hash.push('x');
        let mut fresh = Measurements::keeping_in(folder);
        let ctx = egui::Context::default();
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let failed = loop {
            if let Some(result) = fresh.get(&jobs, &ctx, &other, other.info.audio[0].index) {
                break result;
            }
            assert!(std::time::Instant::now() < give_up);
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(failed.is_err(), "not read back for another file");
    }

    #[test]
    fn a_sound_file_is_measured() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/voice.mp3");
        let probed = media::probe(&fixture).unwrap();
        let stream = (probed.info.audio[0].index, probed.info.audio[0].channels);
        let reading = measure(&fixture, stream, || false).unwrap();
        assert!(
            reading.true_peak.is_finite() && reading.true_peak < 3.0,
            "{reading:?}"
        );
        assert!(
            measure(&fixture, stream, || true).is_err(),
            "stopped when no longer needed"
        );
        assert!(measure(Path::new("/nowhere/at/all.wav"), (0, 2), || false).is_err());
    }

    /// `seconds` of a sine at `freq` Hz `gain` times full scale, clipped,
    /// as a 16-bit WAV of one channel at 48 kHz.
    fn clipped_mono_wav(path: &Path, freq: f64, gain: f64, seconds: u32) {
        let frames = RATE * seconds;
        let mut bytes = Vec::new();
        let data = frames * 2;
        bytes.extend(b"RIFF");
        bytes.extend((36 + data).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // whole numbers
        bytes.extend(1u16.to_le_bytes()); // one channel
        bytes.extend(RATE.to_le_bytes());
        bytes.extend((RATE * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data.to_le_bytes());
        for i in 0..frames {
            let v = gain * (std::f64::consts::TAU * freq * f64::from(i) / f64::from(RATE)).sin();
            let s = (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
            bytes.extend(s.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// A clipped microphone recording of one channel: its true peak is at
    /// least its samples' (full scale), not 3 dB under it as when it was
    /// spread over two channels first, and both figures are what the
    /// common command-line tool's loudness filter (true peak on) says of
    /// the same files, within 0.1 dB: 1 kHz twice full scale -0.81 LUFS,
    /// +0.17 dBTP (a true peak of 1.020); 997 Hz 1.5 times -1.44 LUFS,
    /// +0.12 dBTP (1.014); 3 kHz twice full scale +2.00 LUFS, +0.30 dBTP
    /// (1.035), where the four-times oversampled peak (16 taps a phase)
    /// reads about 0.2 dB under the tool's longer filter (the recommendation
    /// allows a 4x meter to read up to 0.5 dB low that near the top), so
    /// its peak is held to 0.3 dB.
    #[test]
    fn a_clipped_mono_recording_reads_as_the_standard_measures_it() {
        let dir = tempfile::tempdir().unwrap();
        for (freq, gain, lufs, dbtp, within) in [
            (1_000.0, 2.0, -0.81, 0.17, 0.1),
            (997.0, 1.5, -1.44, 0.12, 0.1),
            (3_000.0, 2.0, 2.00, 0.30, 0.3),
        ] {
            let path = dir.path().join(format!("clipped-{freq}.wav"));
            clipped_mono_wav(&path, freq, gain, 5);
            let probed = media::probe(&path).unwrap();
            let sound = &probed.info.audio[0];
            assert_eq!(sound.channels, 1);
            let reading = measure(&path, (sound.index, sound.channels), || false).unwrap();
            assert!(
                (reading.integrated - lufs).abs() <= 0.1,
                "{freq} Hz: {reading:?} against {lufs} LUFS"
            );
            assert!(
                (reading.true_peak - dbtp).abs() <= within,
                "{freq} Hz: {reading:?} against {dbtp} dBTP"
            );
            assert!(reading.true_peak >= -0.01, "never under the samples' own peak");
        }
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
