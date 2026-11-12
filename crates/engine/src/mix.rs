//! Mixing the timeline's sound: every audible clip at a moment, summed.
//!
//! Tracks that are muted, or not soloed while another track is, are silent.
//! Each clip's own sound (volume, pan, fades) applies, then its track's
//! volume, then the master's. Decoders stay open per file and stream and
//! only seek when the next clip doesn't continue where they are.

use media::AudioDecoder;
use media::audio::CHANNELS;
use project::{Clip, MediaId, Project, Sequence, Sound};
use std::f64::consts::FRAC_PI_2;

/// How far a decoder may be from where it is needed before it seeks (seconds).
const SEEK_SLACK: f64 = 0.005;
/// How many files and streams keep an open decoder.
const OPEN_DECODERS: usize = 16;

pub struct Mixer {
    rate: u32,
    /// Open decoders by file, stream and the rate they read at (a clip at
    /// another speed reads its file at another rate).
    decoders: Vec<((MediaId, u32, u32), AudioDecoder)>,
    scratch: Vec<f32>,
    /// Files that failed to open, so the log isn't flooded.
    failed: Vec<(MediaId, u32, u32)>,
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mixer({} Hz, {} open)", self.rate, self.decoders.len())
    }
}

impl Mixer {
    /// A mixer producing interleaved stereo at `rate` samples per second.
    pub fn new(rate: u32) -> Mixer {
        Mixer {
            rate,
            decoders: Vec::new(),
            scratch: Vec::new(),
            failed: Vec::new(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Fills `out` (interleaved stereo) with the timeline's sound from
    /// `start` seconds on. Gaps and silent tracks give zeros.
    pub fn mix(&mut self, project: &Project, sequence: &Sequence, start: f64, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / CHANNELS;
        let rate = f64::from(self.rate);
        let end = start + frames as f64 / rate;
        let soloing = sequence.audio_tracks.iter().any(|t| t.solo);
        for track in &sequence.audio_tracks {
            if track.muted || (soloing && !track.solo) {
                continue;
            }
            let gain = 10f32.powf(track.gain_db / 20.0);
            for clip in &track.clips {
                if clip.sound.is_silent() {
                    continue;
                }
                let (cs, ce) = (clip.start.as_secs_f64(), clip.end().as_secs_f64());
                let (a, b) = (cs.max(start), ce.min(end));
                if a >= b {
                    continue;
                }
                let Some(media) = project.media.get(&clip.media) else {
                    continue;
                };
                let first = ((a - start) * rate).round() as usize;
                let count = (((b - start) * rate).round() as usize)
                    .min(frames)
                    .saturating_sub(first);
                if count == 0 {
                    continue;
                }
                // At half speed the file is read at twice the rate, so each
                // second of the timeline plays half a second of it (lower,
                // like tape).
                let speed = clip.speed.as_f64();
                let source = clip.source_in.as_secs_f64() + (a - cs) * speed;
                let read_rate = ((rate / speed).round() as u32).max(1);
                let key = (media.id, clip.stream, read_rate);
                let mut scratch = std::mem::take(&mut self.scratch);
                scratch.resize(count * CHANNELS, 0.0);
                let got = match self.decoder(key, &media.path) {
                    Some(decoder) => {
                        if (decoder.position() - source).abs() > SEEK_SLACK {
                            decoder.seek(source);
                        }
                        decoder.read(&mut scratch)
                    }
                    None => 0,
                };
                if let Some(shape) = Shape::of(clip) {
                    // Seconds into the clip of the first sample read.
                    let from = start + first as f64 / rate - cs;
                    shape.apply(&mut scratch[..got * CHANNELS], from, rate);
                }
                for (o, s) in out[first * CHANNELS..(first + got) * CHANNELS]
                    .iter_mut()
                    .zip(&scratch)
                {
                    *o += s * gain;
                }
                self.scratch = scratch;
            }
        }
        if sequence.master_gain_db != 0.0 {
            let master = 10f32.powf(sequence.master_gain_db / 20.0);
            for s in out.iter_mut() {
                *s *= master;
            }
        }
    }

    fn decoder(&mut self, key: (MediaId, u32, u32), path: &std::path::Path) -> Option<&mut AudioDecoder> {
        if let Some(i) = self.decoders.iter().position(|(k, _)| *k == key) {
            let entry = self.decoders.remove(i);
            self.decoders.insert(0, entry);
        } else {
            if self.failed.contains(&key) {
                return None;
            }
            match AudioDecoder::open(path, key.1, key.2) {
                Ok(d) => {
                    self.decoders.insert(0, (key, d));
                    self.decoders.truncate(OPEN_DECODERS);
                }
                Err(err) => {
                    tracing::warn!("{}", err.report());
                    self.failed.push(key);
                    return None;
                }
            }
        }
        self.decoders.first_mut().map(|(_, d)| d)
    }
}

/// A decibel value in thousandths as a factor on samples. The bottom of the
/// range is silence.
pub fn volume(gain_mdb: i32) -> f32 {
    if gain_mdb <= Sound::SILENT {
        0.0
    } else {
        10f32.powf(gain_mdb as f32 / 20_000.0)
    }
}

/// The factors on the left and right channel for a pan in millionths
/// (-1 000 000 all left, 1 000 000 all right). Equal power: the two
/// factors' squares always add up to the same, so a sound moved across
/// keeps its loudness; scaled so that the centre leaves both channels as
/// they are (all the way to one side is that side 3 dB louder).
pub fn pan_gains(pan: i32) -> (f32, f32) {
    let share = f64::from(pan.clamp(-Sound::SIDE, Sound::SIDE)) / f64::from(Sound::SIDE);
    let angle = (share + 1.0) * std::f64::consts::FRAC_PI_4;
    let wide = std::f64::consts::SQRT_2;
    ((angle.cos() * wide) as f32, (angle.sin() * wide) as f32)
}

/// How far up a fade is at `share` of its length (0 silent, 1 full): a
/// quarter of a sine, which sounds even (at the middle it is 3 dB down, half
/// the power) where a straight line seems to rush at the end.
pub fn fade(share: f64) -> f32 {
    (share.clamp(0.0, 1.0) * FRAC_PI_2).sin() as f32
}

/// What a clip's own sound does to its samples.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    left: f32,
    right: f32,
    /// Seconds on the timeline: fades last as long whatever the clip's speed.
    fade_in: f64,
    fade_out: f64,
    length: f64,
}

impl Shape {
    /// `None` when the clip sounds as recorded: nothing to do.
    fn of(clip: &Clip) -> Option<Shape> {
        let sound = clip.sound;
        if sound == Sound::PLAIN {
            return None;
        }
        let gain = volume(sound.gain_mdb);
        let (left, right) = pan_gains(sound.pan);
        Some(Shape {
            left: left * gain,
            right: right * gain,
            fade_in: sound.fade_in.as_secs_f64(),
            fade_out: sound.fade_out.as_secs_f64(),
            length: clip.duration.as_secs_f64(),
        })
    }

    /// The fades' factor `at` seconds into the clip.
    fn faded(&self, at: f64) -> f32 {
        let mut level = 1.0;
        if self.fade_in > 0.0 && at < self.fade_in {
            level *= fade(at / self.fade_in);
        }
        let left = self.length - at;
        if self.fade_out > 0.0 && left < self.fade_out {
            level *= fade(left / self.fade_out);
        }
        level
    }

    /// Shapes interleaved stereo `samples` whose first frame is `from`
    /// seconds into the clip, at `rate` frames a second.
    fn apply(&self, samples: &mut [f32], from: f64, rate: f64) {
        let fading_from = self.length - self.fade_out;
        for (i, frame) in samples.chunks_exact_mut(CHANNELS).enumerate() {
            let at = from + i as f64 / rate;
            let level = if at >= self.fade_in && at <= fading_from {
                1.0
            } else {
                self.faded(at)
            };
            frame[0] *= self.left * level;
            frame[1] *= self.right * level;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Project, Time};
    use std::path::Path;

    fn project() -> Project {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture).unwrap();
        Command::Import { sequence: seq, media }.apply(&p).unwrap()
    }

    fn loudness(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    #[test]
    fn clips_sound_and_gaps_are_silent() {
        let p = project();
        let seq = p.active().unwrap();
        let mut mixer = Mixer::new(48_000);
        let mut buf = vec![0f32; 4_800 * CHANNELS];
        mixer.mix(&p, seq, 0.5, &mut buf);
        let all_three = loudness(&buf);
        assert!(all_three > 0.05, "{all_three}");
        mixer.mix(&p, seq, 5.0, &mut buf);
        assert_eq!(loudness(&buf), 0.0, "after the clip");
        // Half in, half out: only the first half sounds.
        mixer.mix(&p, seq, 1.95, &mut buf);
        assert!(loudness(&buf[..2_000 * CHANNELS]) > 0.01);
        assert_eq!(loudness(&buf[2_600 * CHANNELS..]), 0.0);
    }

    #[test]
    fn mute_solo_and_volume() {
        let p = project();
        let mut mixer = Mixer::new(48_000);
        let mut buf = vec![0f32; 4_800 * CHANNELS];
        let level = |p: &Project, mixer: &mut Mixer, buf: &mut Vec<f32>| {
            mixer.mix(p, p.active().unwrap(), 0.5, buf);
            loudness(buf)
        };
        let full = level(&p, &mut mixer, &mut buf);

        let mut muted = p.clone();
        for t in muted.sequences[0].audio_tracks.iter_mut() {
            t.muted = true;
        }
        assert_eq!(level(&muted, &mut mixer, &mut buf), 0.0);

        let mut solo = p.clone();
        solo.sequences[0].audio_tracks[1].solo = true;
        let one = level(&solo, &mut mixer, &mut buf);
        assert!(one > 0.0 && one < full, "{one} < {full}");

        let mut quiet = solo.clone();
        quiet.sequences[0].audio_tracks[1].gain_db = -20.0;
        let q = level(&quiet, &mut mixer, &mut buf);
        assert!((q / one - 0.1).abs() < 0.01, "-20 dB is a tenth: {}", q / one);

        let mut master = solo.clone();
        master.sequences[0].master_gain_db = -6.0;
        let m = level(&master, &mut mixer, &mut buf);
        assert!(
            (m / one - 0.501).abs() < 0.01,
            "-6 dB on the whole mix: {}",
            m / one
        );
        let _ = Time::ZERO;
    }

    #[test]
    fn decibels_pan_and_fade_curve() {
        assert_eq!(volume(0), 1.0);
        assert!((volume(-6_000) - 0.501).abs() < 0.001);
        assert!((volume(12_000) - 3.981).abs() < 0.001);
        assert_eq!(volume(Sound::SILENT), 0.0, "the bottom is silence");
        assert_eq!(pan_gains(0), (1.0, 1.0), "the centre changes nothing");
        let (l, r) = pan_gains(-Sound::SIDE);
        assert!((l - std::f32::consts::SQRT_2).abs() < 1e-6 && r.abs() < 1e-6);
        for pan in [-700_000, -200_000, 300_000, 1_000_000] {
            let (l, r) = pan_gains(pan);
            assert!((l * l + r * r - 2.0).abs() < 1e-5, "equal power at {pan}");
        }
        let (l, r) = pan_gains(500_000);
        assert!(r > l, "to the right");
        assert_eq!(fade(0.0), 0.0);
        assert!(
            (fade(0.5) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "3 dB down halfway"
        );
        assert_eq!(fade(1.0), 1.0);
    }

    #[test]
    fn fades_follow_the_clips_start_and_end_to_the_sample() {
        let shape = Shape {
            left: 1.0,
            right: 1.0,
            fade_in: 1.0,
            fade_out: 0.5,
            length: 2.0,
        };
        let rate = 48_000.0;
        let mut ones = vec![1f32; 96_000 * CHANNELS];
        shape.apply(&mut ones, 0.0, rate);
        let at = |frame: usize| ones[frame * CHANNELS];
        assert_eq!(at(0), 0.0, "silent at the very start");
        assert!((at(24_000) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(at(60_000), 1.0, "full between the fades");
        assert!(
            (at(84_000) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "fade out halfway"
        );
        assert!(at(95_999) < 0.001, "almost silent at the end");
        // Read from the middle of the fade in, the curve goes on from there.
        let mut later = vec![1f32; 100 * CHANNELS];
        shape.apply(&mut later, 0.5, rate);
        assert!((later[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    /// The fixture with only its first sound track heard, and that clip's sound set.
    fn first_track_with(sound: Sound, speed: Option<project::Speed>) -> Project {
        let mut p = project();
        let seq = p.active_sequence;
        if let Some(speed) = speed {
            let clip = p.active().unwrap().video_tracks[0].clips[0].id;
            p = Command::SetSpeed {
                sequence: seq,
                clip,
                speed,
            }
            .apply(&p)
            .unwrap();
        }
        let clip = p.active().unwrap().audio_tracks[0].clips[0].id;
        p = Command::SetSound {
            sequence: seq,
            sounds: vec![(clip, sound)],
        }
        .apply(&p)
        .unwrap();
        p.sequences[0].audio_tracks[0].solo = true;
        p
    }

    fn channel(buf: &[f32], which: usize) -> Vec<f32> {
        buf.iter().skip(which).step_by(CHANNELS).copied().collect()
    }

    #[test]
    fn a_clips_volume_and_pan_are_heard() {
        let plain = first_track_with(Sound::PLAIN, None);
        let mut mixer = Mixer::new(48_000);
        let mut buf = vec![0f32; 4_800 * CHANNELS];
        mixer.mix(&plain, plain.active().unwrap(), 0.5, &mut buf);
        let (l, r) = (loudness(&channel(&buf, 0)), loudness(&channel(&buf, 1)));
        assert!(l > 0.01 && r > 0.01, "{l} {r}");

        let quiet = first_track_with(
            Sound {
                gain_mdb: -6_000,
                ..Sound::PLAIN
            },
            None,
        );
        mixer.mix(&quiet, quiet.active().unwrap(), 0.5, &mut buf);
        let q = loudness(&channel(&buf, 0));
        assert!((q / l - 0.501).abs() < 0.005, "-6 dB: {}", q / l);

        let left = first_track_with(
            Sound {
                pan: -Sound::SIDE,
                ..Sound::PLAIN
            },
            None,
        );
        mixer.mix(&left, left.active().unwrap(), 0.5, &mut buf);
        let (pl, pr) = (loudness(&channel(&buf, 0)), loudness(&channel(&buf, 1)));
        assert!(pr < 1e-6, "nothing on the right: {pr}");
        assert!(
            (pl / l - std::f32::consts::SQRT_2).abs() < 0.01,
            "+3 dB on the left: {}",
            pl / l
        );

        let silent = first_track_with(
            Sound {
                gain_mdb: Sound::SILENT,
                ..Sound::PLAIN
            },
            None,
        );
        mixer.mix(&silent, silent.active().unwrap(), 0.5, &mut buf);
        assert_eq!(loudness(&buf), 0.0);
    }

    /// Sample by sample, the faded mix over the plain one: the fade's own curve.
    fn fade_heard(speed: Option<project::Speed>) -> Vec<(usize, f32)> {
        let half_second = project::Time::from_ratio(1, 2).unwrap();
        let faded = first_track_with(
            Sound {
                fade_in: half_second,
                ..Sound::PLAIN
            },
            speed,
        );
        let plain = first_track_with(Sound::PLAIN, speed);
        let mut mixer = Mixer::new(48_000);
        let (mut a, mut b) = (vec![0f32; 48_000 * CHANNELS], vec![0f32; 48_000 * CHANNELS]);
        mixer.mix(&faded, faded.active().unwrap(), 0.0, &mut a);
        let mut mixer = Mixer::new(48_000);
        mixer.mix(&plain, plain.active().unwrap(), 0.0, &mut b);
        let (a, b) = (channel(&a, 0), channel(&b, 0));
        [0usize, 6_000, 12_000, 18_000, 24_000, 30_000]
            .iter()
            .map(|&i| {
                // The nearest sample loud enough to divide by.
                let j = (i..i + 200).find(|&j| b[j].abs() > 0.01).unwrap();
                (j, a[j] / b[j])
            })
            .collect()
    }

    #[test]
    fn a_fade_in_rises_on_a_sine_from_the_clips_start_whatever_its_speed() {
        for speed in [None, Some(project::Speed::new(1, 2).unwrap())] {
            for (frame, ratio) in fade_heard(speed) {
                let expected = fade(frame as f64 / 24_000.0);
                assert!(
                    (ratio - expected).abs() < 1e-4,
                    "at sample {frame} ({speed:?}): {ratio}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn a_slowed_clip_sounds_for_longer() {
        let p = project();
        let seq = p.active_sequence;
        let clip = p.active().unwrap().video_tracks[0].clips[0].id;
        let mut mixer = Mixer::new(48_000);
        let mut buf = vec![0f32; 4_800 * CHANNELS];
        mixer.mix(&p, p.active().unwrap(), 3.0, &mut buf);
        assert_eq!(loudness(&buf), 0.0, "past the clip at normal speed");
        let slow = Command::SetSpeed {
            sequence: seq,
            clip,
            speed: project::Speed::new(1, 2).unwrap(),
        }
        .apply(&p)
        .unwrap();
        mixer.mix(&slow, slow.active().unwrap(), 3.0, &mut buf);
        assert!(loudness(&buf) > 0.01, "half speed: still sounding");
    }
}
