//! Mixing the timeline's sound: every audible clip at a moment, summed.
//!
//! Tracks that are muted, or not soloed while another track is, are silent.
//! Each clip's own sound (volume, pan, fades) applies, then its track's
//! volume, then the master's. Decoders stay open per file and stream and
//! only seek on a real jump: when the next stretch isn't the same clip
//! carrying on from where the last one stopped, nor a clip starting where
//! the decoder is (to the sample). Rounding never makes it seek: the
//! decoder counts samples exactly (`media::audio`), and a clip that carries
//! on is known by itself, not by comparing times.

use media::AudioDecoder;
use media::audio::CHANNELS;
use project::{Clip, ClipId, MediaId, MediaRef, Project, Sequence, SequenceId, Sound, Speed, Time};
use std::f64::consts::FRAC_PI_2;

/// How far ahead of a ramped clip's decoder its next sample may be and
/// still be read on to rather than jumped to (seconds).
const SEEK_SLACK: f64 = 0.005;
/// How many files and streams keep an open decoder.
const OPEN_DECODERS: usize = 16;

pub struct Mixer {
    rate: u32,
    /// Open decoders by file, stream and the rate they read at (a clip at
    /// another speed reads its file at another rate).
    decoders: Vec<Open>,
    scratch: Vec<f32>,
    /// Files that failed to open, so the log isn't flooded.
    failed: Vec<(MediaId, u32, u32)>,
    /// What ramped clips read last, to carry on from (see `Mixer::ramped`),
    /// most recent first.
    carries: Vec<Carry>,
    /// The file's samples a ramped clip reads, kept between calls.
    ramp_read: Vec<f32>,
    /// Mixers of nested timelines by timeline and the rate they mix at (a
    /// nested clip at another speed mixes its timeline faster or slower,
    /// tape-style), most recent first.
    nested: Vec<((SequenceId, u32), Mixer)>,
    /// How many timelines deep this mixer is: 0 for the timeline played.
    depth: usize,
}

/// A decoder kept open, most recently used first.
struct Open {
    key: (MediaId, u32, u32),
    decoder: AudioDecoder,
    /// The clip it last read for, if it read for one without a ramp.
    follows: Option<Follows>,
}

/// What a decoder last read for: a clip as it was then, and the sample on
/// the timeline (at the mixer's rate) its next stretch starts at. The same
/// clip, unchanged, asking for that sample carries on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Follows {
    clip: ClipId,
    start: Time,
    source_in: Time,
    speed: Speed,
    next: i64,
}

/// Where a ramped clip's reading stopped: its decoder's next sample (in
/// samples from the file's start) and the sample before it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Carry {
    clip: ClipId,
    key: (MediaId, u32, u32),
    next: i64,
    last: [f32; CHANNELS],
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
            carries: Vec::new(),
            ramp_read: Vec::new(),
            nested: Vec::new(),
            depth: 0,
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
                let first = ((a - start) * rate).round() as usize;
                let count = (((b - start) * rate).round() as usize)
                    .min(frames)
                    .saturating_sub(first);
                if count == 0 {
                    continue;
                }
                let mut scratch = std::mem::take(&mut self.scratch);
                scratch.resize(count * CHANNELS, 0.0);
                let from = start + first as f64 / rate;
                let got = if let Some(inner) = clip.nested() {
                    self.nested_sound(project, clip, inner, a, b, &mut scratch)
                } else if let Some(media) = project.media.get(&clip.media) {
                    self.file_sound(clip, media, a, from, &mut scratch)
                } else {
                    0
                };
                if let Some(shape) = Shape::of(clip) {
                    // Seconds into the clip of the first sample read.
                    shape.apply(&mut scratch[..got * CHANNELS], from - cs, rate);
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

    /// Fills `out` with a file clip's sound from `a` seconds on the
    /// timeline (`from` is the same moment brought to a whole sample).
    /// Returns the sample frames written.
    fn file_sound(&mut self, clip: &Clip, media: &MediaRef, a: f64, from: f64, out: &mut [f32]) -> usize {
        let rate = f64::from(self.rate);
        let cs = clip.start.as_secs_f64();
        if clip.ramp.is_empty() {
            // At half speed the file is read at twice the rate, so
            // each second of the timeline plays half a second of it
            // (lower, like tape).
            let speed = clip.speed.as_f64();
            let source = clip.source_in.as_secs_f64() + (a - cs) * speed;
            let read_rate = ((rate / speed).round() as u32).max(1);
            let key = (media.id, clip.stream, read_rate);
            let wanted = (source.max(0.0) * f64::from(read_rate)).round() as i64;
            let follows = Follows {
                clip: clip.id,
                start: clip.start,
                source_in: clip.source_in,
                speed: clip.speed,
                next: (a * rate).round() as i64,
            };
            let Some(open) = self.open(key, &media.path) else {
                return 0;
            };
            // The same clip carrying on, or one that starts where the
            // decoder is (a clip cut in two): read on. Anything else is a
            // jump. (At another speed the file's place, worked out from the
            // timeline's time, and the samples read part by a fraction of
            // a sample a stretch: carrying on is known by the clip.)
            let carries_on =
                open.follows == Some(follows) || (open.decoder.next_sample() - wanted).abs() <= 1;
            if !carries_on {
                open.decoder.seek_to_sample(wanted);
            }
            let got = open.decoder.read(out);
            // Past the file's end the clip carries on, silent.
            open.follows = Some(Follows {
                next: follows.next + (out.len() / CHANNELS) as i64,
                ..follows
            });
            got
        } else {
            self.ramped(clip, &media.path, from, out)
        }
    }

    /// Fills `out` with a nested timeline's sound for its clip from `a` to
    /// `b` seconds on this timeline: that timeline mixed from where the clip
    /// is in it, by a mixer of its own, at a rate that makes it play at the
    /// clip's speed there (tape-style, as a file's sound does; a ramp is
    /// followed at the speed it has over each stretch mixed). Returns the
    /// sample frames written.
    fn nested_sound(
        &mut self,
        project: &Project,
        clip: &Clip,
        id: SequenceId,
        a: f64,
        b: f64,
        out: &mut [f32],
    ) -> usize {
        let Some(inner) = project.sequence(id) else {
            return 0;
        };
        if self.depth >= crate::timeline::DEEPEST {
            return 0;
        }
        let source = clip.source_at_f64(a);
        let speed = if clip.ramp.is_empty() || b <= a {
            clip.speed.as_f64()
        } else {
            (clip.source_at_f64(b) - source) / (b - a)
        };
        let read_rate = ((f64::from(self.rate) / speed.max(0.0001)).round() as u32).clamp(1, 1_000_000);
        let key = (id, read_rate);
        let mut mixer = match self.nested.iter().position(|(k, _)| *k == key) {
            Some(i) => self.nested.remove(i).1,
            None => {
                let mut mixer = Mixer::new(read_rate);
                mixer.depth = self.depth + 1;
                mixer
            }
        };
        mixer.mix(project, inner, source, out);
        self.nested.insert(0, (key, mixer));
        self.nested.truncate(OPEN_DECODERS);
        out.len() / CHANNELS
    }

    /// Fills `out` with a ramped clip's sound from `from` seconds on the
    /// timeline: tape-style, the pitch following the speed as it changes.
    /// The file is read at the mixer's own rate and each sample taken from
    /// where the ramp says the clip is in its file then, between two read
    /// samples by their distance. Reading carries on from one call to the
    /// next (the last sample read is kept for the step between them), so
    /// the decoder only seeks when the clip doesn't continue. Returns the
    /// sample frames written.
    fn ramped(&mut self, clip: &Clip, path: &std::path::Path, from: f64, out: &mut [f32]) -> usize {
        let count = out.len() / CHANNELS;
        let rate = f64::from(self.rate);
        let key = (clip.media, clip.stream, self.rate);
        // Where in the file (in samples) each sample written comes from.
        let place = |i: usize| clip.source_at_f64(from + i as f64 / rate).max(0.0) * rate;
        let (first, last) = (place(0), place(count.saturating_sub(1)));
        let low = first.floor() as i64;
        let high = last.floor() as i64 + 1;
        let carried = self
            .carries
            .iter()
            .position(|c| c.clip == clip.id && c.key == key)
            .map(|i| self.carries.remove(i));
        let mut read = std::mem::take(&mut self.ramp_read);
        read.clear();
        let Some(open) = self.open(key, path) else {
            self.ramp_read = read;
            return 0;
        };
        open.follows = None;
        let decoder = &mut open.decoder;
        let next = decoder.next_sample();
        // Carry on where the decoder is when that is this clip's last
        // sample read and close enough; seek otherwise.
        let base = match carried {
            Some(c) if c.next == next && low >= next - 1 && low <= next + (SEEK_SLACK * rate) as i64 => {
                read.extend_from_slice(&c.last);
                next - 1
            }
            _ => {
                decoder.seek_to_sample(low);
                low
            }
        };
        let already = (read.len() / CHANNELS) as i64;
        let wanted = (high + 1 - (base + already)).max(0) as usize;
        let start = read.len();
        read.resize(start + wanted * CHANNELS, 0.0);
        let got = decoder.read(&mut read[start..]);
        read.truncate(start + got * CHANNELS);
        let have = read.len() / CHANNELS;
        if have == 0 {
            self.ramp_read = read;
            return 0;
        }
        for (i, frame) in out.chunks_exact_mut(CHANNELS).enumerate() {
            let x = (place(i) - base as f64).max(0.0);
            let k = (x.floor() as usize).min(have - 1);
            let share = (x - k as f64).clamp(0.0, 1.0) as f32;
            let k2 = (k + 1).min(have - 1);
            for (channel, sample) in frame.iter_mut().enumerate() {
                let (a, b) = (read[k * CHANNELS + channel], read[k2 * CHANNELS + channel]);
                *sample = a + (b - a) * share;
            }
        }
        let mut last = [0.0; CHANNELS];
        last.copy_from_slice(&read[(have - 1) * CHANNELS..have * CHANNELS]);
        self.carries.insert(
            0,
            Carry {
                clip: clip.id,
                key,
                next: base + have as i64,
                last,
            },
        );
        self.carries.truncate(OPEN_DECODERS);
        self.ramp_read = read;
        count
    }

    fn open(&mut self, key: (MediaId, u32, u32), path: &std::path::Path) -> Option<&mut Open> {
        if let Some(i) = self.decoders.iter().position(|o| o.key == key) {
            let entry = self.decoders.remove(i);
            self.decoders.insert(0, entry);
        } else {
            if self.failed.contains(&key) {
                return None;
            }
            match AudioDecoder::open(path, key.1, key.2) {
                Ok(decoder) => {
                    self.decoders.insert(
                        0,
                        Open {
                            key,
                            decoder,
                            follows: None,
                        },
                    );
                    self.decoders.truncate(OPEN_DECODERS);
                }
                Err(err) => {
                    tracing::warn!("{}", err.report());
                    self.failed.push(key);
                    return None;
                }
            }
        }
        self.decoders.first_mut()
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

    /// The fixture's first sound track alone, its clips given `ramp`.
    fn first_track_ramped(ramp: project::Ramp) -> Project {
        let p = first_track_with(Sound::PLAIN, None);
        let clip = p.active().unwrap().video_tracks[0].clips[0].id;
        Command::SetRamp {
            sequence: p.active_sequence,
            clip,
            ramp,
        }
        .apply(&p)
        .unwrap()
    }

    /// How many times a channel crosses zero going up: its pitch, roughly.
    fn rises(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count()
    }

    fn key(at: project::Time, speed: project::Speed) -> project::SpeedKey {
        project::SpeedKey {
            at,
            speed,
            ease: project::Ease::Linear,
        }
    }

    #[test]
    fn a_ramp_at_one_speed_sounds_like_that_speed_and_carries_on_between_calls() {
        let half = project::Speed::new(1, 2).unwrap();
        let ramped = first_track_ramped(project::Ramp::new([key(project::Time::ZERO, half)]));
        let slowed = first_track_with(Sound::PLAIN, Some(half));
        let mut whole = vec![0f32; 24_000 * CHANNELS];
        Mixer::new(48_000).mix(&ramped, ramped.active().unwrap(), 0.25, &mut whole);
        let mut plain = vec![0f32; 24_000 * CHANNELS];
        Mixer::new(48_000).mix(&slowed, slowed.active().unwrap(), 0.25, &mut plain);
        let (a, b) = (channel(&whole, 0), channel(&plain, 0));
        assert!(loudness(&a) > 0.01, "it sounds");
        assert!((loudness(&a) / loudness(&b) - 1.0).abs() < 0.05);
        let (ra, rb) = (rises(&a) as f64, rises(&b) as f64);
        assert!((ra / rb - 1.0).abs() < 0.03, "the same pitch: {ra} and {rb}");
        // Mixed a little at a time, as playback does: the same sound.
        let mut mixer = Mixer::new(48_000);
        let mut pieces = Vec::new();
        for i in 0..50 {
            let mut block = vec![0f32; 480 * CHANNELS];
            mixer.mix(
                &ramped,
                ramped.active().unwrap(),
                0.25 + i as f64 * 0.01,
                &mut block,
            );
            pieces.extend(block);
        }
        let largest = pieces
            .iter()
            .zip(&whole)
            .map(|(x, y)| (x - y).abs())
            .fold(0f32, f32::max);
        assert!(largest < 1e-3, "pieces differ by up to {largest}");
    }

    #[test]
    fn a_ramp_down_to_half_speed_drops_the_pitch_an_octave() {
        let ramp = project::Ramp::new([
            key(project::Time::from_ratio(1, 2).unwrap(), project::Speed::NORMAL),
            key(project::Time::seconds(1), project::Speed::new(1, 2).unwrap()),
        ]);
        let ramped = first_track_ramped(ramp);
        let mut buf = vec![0f32; 96_000 * CHANNELS];
        Mixer::new(48_000).mix(&ramped, ramped.active().unwrap(), 0.0, &mut buf);
        let left = channel(&buf, 0);
        let (before, after) = (
            rises(&left[0..19_200]) as f64,
            rises(&left[57_600..76_800]) as f64,
        );
        assert!(before > 20.0, "a tone: {before}");
        assert!((before / after - 2.0).abs() < 0.1, "{before} then {after}");
    }

    #[test]
    fn a_nested_timeline_sounds_as_its_clips_did() {
        let p = project();
        let seq = p.active_sequence;
        let picture = p.active().unwrap().video_tracks[0].clips[0].id;
        let mut direct = vec![0.0f32; 4_800 * CHANNELS];
        Mixer::new(48_000).mix(&p, p.active().unwrap(), 0.5, &mut direct);
        let nested = Command::NestClips {
            sequence: seq,
            clips: vec![picture],
            name: "Inner".into(),
        }
        .apply(&p)
        .unwrap();
        let outer = nested.active().unwrap();
        assert!(
            outer
                .audio_tracks
                .iter()
                .all(|t| t.clips.iter().all(|c| c.nested().is_some()))
        );
        let mut heard = vec![0.0f32; 4_800 * CHANNELS];
        Mixer::new(48_000).mix(&nested, outer, 0.5, &mut heard);
        assert!(loudness(&direct) > 0.001);
        let most = direct
            .iter()
            .zip(&heard)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(most < 1e-4, "the same samples, differing by up to {most}");
        // At half speed the nested timeline plays half as far in the same time.
        let slow = Command::SetSpeed {
            sequence: seq,
            clip: outer.video_tracks[0].clips[0].id,
            speed: project::Speed::new(1, 2).unwrap(),
        }
        .apply(&nested)
        .unwrap();
        let mut slowed = vec![0.0f32; 4_800 * CHANNELS];
        Mixer::new(48_000).mix(&slow, slow.active().unwrap(), 1.0, &mut slowed);
        assert!(loudness(&slowed) > 0.001, "still heard");
    }

    fn imported(name: &str) -> Project {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name);
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture).unwrap();
        Command::Import { sequence: seq, media }.apply(&p).unwrap()
    }

    /// Each sound clip mixed a block at a time, as export (800 samples, a
    /// frame at 60 per second) and playback (1 024) mix, is exactly its file
    /// read straight through from where the clip starts in it: the mixer
    /// never jumps while a clip carries on, whatever the file's timestamps
    /// say. On the recording whose Opus sound is timed in rounded
    /// milliseconds; on one whose sound starts before its picture and one
    /// whose sound starts after it; from the file's start and from the
    /// middle; with the clip at the timeline's start and later.
    #[test]
    fn mixed_a_block_at_a_time_a_clip_is_its_file_read_straight() {
        let third = Time::from_ratio(1, 3).unwrap();
        let quarter = Time::from_ratio(1, 4).unwrap();
        for file in ["game-3-audio.mkv", "sound-offset.mkv"] {
            let p = imported(file);
            let tracks = p.active().unwrap().audio_tracks.len();
            assert!(tracks >= 2, "{file}");
            for track in 0..tracks {
                for (source_in, start) in [(Time::ZERO, Time::ZERO), (third, Time::ZERO), (third, quarter)] {
                    for block in [800usize, 1_024] {
                        let mut q = p.clone();
                        let seq = &mut q.sequences[0];
                        seq.audio_tracks[track].solo = true;
                        let clip = &mut seq.audio_tracks[track].clips[0];
                        clip.source_in = source_in;
                        clip.start = start;
                        clip.duration = Time::from_ratio(1, 2).unwrap();
                        let (media, stream) = (q.media[&clip.media].path.clone(), clip.stream);

                        let mut mixer = Mixer::new(48_000);
                        let mut mixed = vec![0f32; 48_000 * CHANNELS];
                        for (n, chunk) in mixed.chunks_mut(block * CHANNELS).enumerate() {
                            mixer.mix(&q, q.active().unwrap(), (n * block) as f64 / 48_000.0, chunk);
                        }
                        let jumps: u64 = mixer.decoders.iter().map(|o| o.decoder.seeks()).sum();
                        let expected = u64::from(source_in != Time::ZERO);
                        assert_eq!(
                            jumps, expected,
                            "{file} stream {stream}: jumps only to the clip's start"
                        );
                        let mut straight = AudioDecoder::open(&media, stream, 48_000).unwrap();
                        if source_in != Time::ZERO {
                            straight.seek(source_in.as_secs_f64());
                        }
                        let mut read = vec![0f32; 24_000 * CHANNELS];
                        straight.read(&mut read);
                        let first = (start.as_secs_f64() * 48_000.0).round() as usize * CHANNELS;
                        let heard = &mixed[first..first + read.len()];
                        let what = format!(
                            "{file} stream {stream} from {source_in:?} at {start:?} in blocks of {block}"
                        );
                        assert!(loudness(heard) > 0.01, "{what}: it sounds");
                        let off = heard
                            .iter()
                            .zip(&read)
                            .map(|(a, b)| (a - b).abs())
                            .fold(0f32, f32::max);
                        assert!(off < 1e-6, "{what}: off by {off}");
                        assert!(
                            mixed[..first].iter().all(|s| *s == 0.0),
                            "{what}: silent before the clip"
                        );
                        assert!(
                            mixed[first + read.len()..].iter().all(|s| *s == 0.0),
                            "{what}: silent after the clip"
                        );
                    }
                }
            }
        }
    }

    /// At a speed whose reading rate is rounded (seven tenths: 68 571 a
    /// second for 68 571.4), the file's place worked out from the timeline
    /// and the samples read part by a fraction of a sample a block; a clip
    /// carrying on still never jumps, and sounds as mixed in one go.
    #[test]
    fn at_another_speed_a_clip_carries_on_without_jumping() {
        let p = first_track_with(Sound::PLAIN, Some(project::Speed::new(7, 10).unwrap()));
        let seq = p.active().unwrap();
        let mut whole = vec![0f32; 96_000 * CHANNELS];
        Mixer::new(48_000).mix(&p, seq, 0.0, &mut whole);
        let mut mixer = Mixer::new(48_000);
        let mut pieces = vec![0f32; 96_000 * CHANNELS];
        for (n, chunk) in pieces.chunks_mut(800 * CHANNELS).enumerate() {
            mixer.mix(&p, seq, (n * 800) as f64 / 48_000.0, chunk);
        }
        assert!(loudness(&whole) > 0.01);
        assert!(pieces == whole, "the same samples");
        let jumps: u64 = mixer.decoders.iter().map(|o| o.decoder.seeks()).sum();
        assert_eq!(jumps, 0);
    }
}
