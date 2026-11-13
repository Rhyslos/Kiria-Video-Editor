//! Each heard track's sound after its own effects, as the mixer hears it,
//! measured in bands frame by frame.
//!
//! The tracks go through `crates/audio`'s own [`Strip`] (high pass,
//! clean-up, EQ, compressor, Loudness, ducking, volume, pan and lines), in
//! step and in the mixer's order: dialogue tracks first, summed into the
//! ducking key, then the others ducked under it. So a fix shows in the
//! next check exactly as it will be heard. The master is left out: it
//! changes every track alike, so it never changes which covers which.
//!
//! The strips start a little before the stretch asked for, so envelopes
//! (compressor, ducking, Loudness) have settled, and every track comes out
//! late by the clean-up's fixed delay, which is taken off.

use super::bands::{BandMeter, COUNT, WINDOW};
use crate::mix;
use audio::dsp::cleanup::{LATENCY, NoiseProfile};
use audio::dsp::duck::Key;
use audio::settings::{TrackSettings, lines_from_project, noise_from_project};
use audio::strip::Strip;
use audio::{BLOCK, TrackReadout};
use media::audio::CHANNELS;
use project::{Project, Sequence, Track, TrackId};
use std::collections::HashMap;

/// Samples between frames: a frame stands for this much of the timeline.
pub const HOP: usize = WINDOW / 2;
/// Frames mixed at a time per track: four of the mixer's blocks.
const PIECE: usize = 4 * BLOCK;
/// How long the strips run before the stretch asked for, in samples at
/// 48 kHz (scaled to the rate): a second, for envelopes to settle.
const SETTLE: u64 = 48_000;

/// A track's sound before anything the track itself does: its clips.
pub trait DrySound: Send {
    /// Fills `out` (interleaved stereo) with `track`'s clips from timeline
    /// sample `at` on.
    fn fill(&mut self, project: &Project, track: &Track, at: u64, out: &mut [f32]);
}

/// The clips' sound decoded from their files (decoding, clip volume, pan,
/// fades, speed, ramps, nested timelines), as the feeder mixes it.
#[derive(Debug)]
pub struct Clips {
    rate: u32,
    /// A mixer per track, so tracks never take each other's decoders.
    mixers: HashMap<TrackId, mix::Mixer>,
}

impl Clips {
    pub fn new(rate: u32) -> Clips {
        Clips {
            rate,
            mixers: HashMap::new(),
        }
    }
}

impl DrySound for Clips {
    fn fill(&mut self, project: &Project, track: &Track, at: u64, out: &mut [f32]) {
        let rate = self.rate;
        let mixer = self
            .mixers
            .entry(track.id)
            .or_insert_with(|| mix::Mixer::new(rate));
        mixer.mix_track(project, track, at as f64 / f64::from(rate), out);
    }
}

/// A heard track: its strip and where its sound is.
struct Heard {
    track: Track,
    key: bool,
    strip: Strip,
    readout: TrackReadout,
    l: Vec<f32>,
    r: Vec<f32>,
    /// What came out of the strip and is not measured yet: from timeline
    /// sample `Listening::kept_from` on.
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

/// The tracks heard in a timeline, in the mixer's order (dialogue first).
pub struct Listening {
    rate: u32,
    tracks: Vec<Heard>,
    key: Key,
    key_sum: [Vec<f32>; 2],
    key_db: Vec<f32>,
    piece: Vec<f32>,
    meter: BandMeter,
    /// The next sample fed in, on the timeline (before the strips' delay).
    fed: u64,
    /// The timeline sample the first kept sample is (may be before 0: the
    /// start is then padded with silence).
    kept_from: i64,
}

impl std::fmt::Debug for Listening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Listening({} tracks at {} Hz)", self.tracks.len(), self.rate)
    }
}

/// Whether mute and solo let a track be heard.
pub fn heard(sequence: &Sequence, track: &Track) -> bool {
    let soloing = sequence.audio_tracks.iter().any(|t| t.solo);
    !track.muted && (!soloing || track.solo)
}

impl Listening {
    /// Every heard audio track of `sequence`, ready to measure frames whose
    /// middles start at timeline sample `from`.
    pub fn new(sequence: &Sequence, rate: u32, from: u64) -> Listening {
        let mut tracks: Vec<Heard> = sequence
            .audio_tracks
            .iter()
            .filter(|t| heard(sequence, t))
            .map(|t| {
                let settings = TrackSettings::from_project(t);
                let noise = noise_from_project(&t.sound)
                    .unwrap_or(NoiseProfile::SILENT)
                    .bins(rate);
                Heard {
                    track: t.clone(),
                    key: settings.key,
                    strip: Strip::new(rate, settings, lines_from_project(&t.sound, rate), noise),
                    readout: TrackReadout::default(),
                    l: vec![0.0; PIECE],
                    r: vec![0.0; PIECE],
                    out_l: Vec::new(),
                    out_r: Vec::new(),
                }
            })
            .collect();
        // Dialogue first, as the mixer runs them.
        tracks.sort_by_key(|t| !t.key);
        let settle = SETTLE * u64::from(rate) / 48_000;
        // A frame's window is centred on its stretch of the timeline.
        let kept_from = from as i64 - (HOP / 2) as i64;
        let fed = (kept_from.max(0) as u64).saturating_sub(settle + LATENCY as u64);
        let mut listening = Listening {
            rate,
            tracks,
            key: Key::new(rate as f32),
            key_sum: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            key_db: vec![0.0; BLOCK],
            piece: vec![0.0; PIECE * CHANNELS],
            meter: BandMeter::new(rate),
            fed,
            kept_from,
        };
        if kept_from < 0 {
            let pad = kept_from.unsigned_abs() as usize;
            for t in &mut listening.tracks {
                t.out_l.resize(pad, 0.0);
                t.out_r.resize(pad, 0.0);
            }
        }
        listening
    }

    /// The tracks measured, in the order frames list them.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().map(|t| &t.track)
    }

    /// The next frame: each track's power per band (in [`Listening::tracks`]
    /// order), mixing more as needed. Then moves on by [`HOP`].
    pub fn next_frame(&mut self, project: &Project, dry: &mut dyn DrySound) -> Vec<[f32; COUNT]> {
        while self.tracks.first().is_some_and(|t| t.out_l.len() < WINDOW) {
            self.mix_piece(project, dry);
        }
        let mut frame = Vec::with_capacity(self.tracks.len());
        for t in &mut self.tracks {
            frame.push(self.meter.measure(&t.out_l[..WINDOW], &t.out_r[..WINDOW]));
            t.out_l.drain(..HOP);
            t.out_r.drain(..HOP);
        }
        self.kept_from += HOP as i64;
        frame
    }

    /// One piece through every strip, in step.
    fn mix_piece(&mut self, project: &Project, dry: &mut dyn DrySound) {
        let at = self.fed;
        for t in &mut self.tracks {
            dry.fill(project, &t.track, at, &mut self.piece);
            for ((frame, l), r) in self.piece.chunks_exact(CHANNELS).zip(&mut t.l).zip(&mut t.r) {
                *l = frame[0];
                *r = frame[1];
            }
        }
        for b in 0..PIECE / BLOCK {
            let span = b * BLOCK..(b + 1) * BLOCK;
            let pos = at + (b * BLOCK) as u64;
            for s in &mut self.key_sum {
                s.fill(0.0);
            }
            for key_pass in [true, false] {
                if !key_pass {
                    let [kl, kr] = &self.key_sum;
                    self.key.listen(kl, kr, &mut self.key_db);
                }
                for t in self.tracks.iter_mut().filter(|t| t.key == key_pass) {
                    let (l, r) = (&mut t.l[span.clone()], &mut t.r[span.clone()]);
                    t.strip.process(pos, &self.key_db, true, None, l, r, &t.readout);
                    if key_pass {
                        for (sum, part) in self.key_sum.iter_mut().zip([&*l, &*r]) {
                            for (s, p) in sum.iter_mut().zip(part) {
                                *s += p;
                            }
                        }
                    }
                }
            }
        }
        // What came out is `LATENCY` late: sample i of the piece is timeline
        // sample `at + i - LATENCY`.
        let first_out = at as i64 - LATENCY as i64;
        let wanted = self.kept_from + self.tracks.first().map_or(0, |t| t.out_l.len()) as i64;
        let skip = (wanted - first_out).clamp(0, PIECE as i64) as usize;
        for t in &mut self.tracks {
            t.out_l.extend_from_slice(&t.l[skip..]);
            t.out_r.extend_from_slice(&t.r[skip..]);
        }
        self.fed += PIECE as u64;
    }
}
