//! Mixing the timeline's sound: every audible clip at a moment, summed.
//!
//! Tracks that are muted, or not soloed while another track is, are silent.
//! Each track's volume applies. Decoders stay open per file and stream and
//! only seek when the next clip doesn't continue where they are.

use media::AudioDecoder;
use media::audio::CHANNELS;
use project::{MediaId, Project, Sequence};

/// How far a decoder may be from where it is needed before it seeks (seconds).
const SEEK_SLACK: f64 = 0.005;
/// How many files and streams keep an open decoder.
const OPEN_DECODERS: usize = 16;

pub struct Mixer {
    rate: u32,
    decoders: Vec<((MediaId, u32), AudioDecoder)>,
    scratch: Vec<f32>,
    /// Files that failed to open, so the log isn't flooded.
    failed: Vec<(MediaId, u32)>,
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
                let source = clip.source_in.as_secs_f64() + (a - cs);
                let key = (media.id, clip.stream);
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

    fn decoder(&mut self, key: (MediaId, u32), path: &std::path::Path) -> Option<&mut AudioDecoder> {
        if let Some(i) = self.decoders.iter().position(|(k, _)| *k == key) {
            let entry = self.decoders.remove(i);
            self.decoders.insert(0, entry);
        } else {
            if self.failed.contains(&key) {
                return None;
            }
            match AudioDecoder::open(path, key.1, self.rate) {
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
}
