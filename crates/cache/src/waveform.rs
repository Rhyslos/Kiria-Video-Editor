//! Waveforms: how loud a sound stream is, a hundred times a second.
//!
//! Reading a stream once gives its peaks; the timeline draws any zoom level
//! from them. A 20-minute stream becomes 120,000 numbers (half a megabyte),
//! so they stay in memory. Reading runs on a worker: decoding all the sound
//! of a long recording takes seconds.

use media::AudioDecoder;
use media::audio::CHANNELS;
use std::path::Path;
use std::sync::Arc;

/// Peaks per second of sound.
pub const PER_SECOND: u32 = 100;
/// Sound is read at this rate: plenty for loudness, and cheaper to convert.
const READ_RATE: u32 = 16_000;

/// The loudest sample (0 to 1) in each 1/100 s of a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    pub per_second: u32,
    pub values: Arc<[f32]>,
}

/// Reads stream `stream` of the file and returns its peaks.
pub fn peaks(path: &Path, stream: u32) -> diag::Result<Peaks> {
    let mut decoder = AudioDecoder::open(path, stream, READ_RATE)?;
    decoder.seek(0.0);
    let per_bucket = (READ_RATE / PER_SECOND) as usize;
    let mut buf = vec![0f32; per_bucket * CHANNELS * 256];
    let mut values = Vec::new();
    let mut current = 0f32;
    let mut filled = 0usize;
    loop {
        let frames = decoder.read(&mut buf);
        if frames == 0 {
            break;
        }
        for frame in buf[..frames * CHANNELS].chunks_exact(CHANNELS) {
            current = current.max(frame.iter().fold(0f32, |m, s| m.max(s.abs())));
            filled += 1;
            if filled == per_bucket {
                values.push(current.min(1.0));
                current = 0.0;
                filled = 0;
            }
        }
    }
    if filled > 0 {
        values.push(current.min(1.0));
    }
    Ok(Peaks {
        per_second: PER_SECOND,
        values: values.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    #[test]
    fn a_steady_tone_has_steady_peaks() {
        let p = peaks(&fixture("voice.mp3"), 0).unwrap();
        assert_eq!(p.per_second, 100);
        assert!((148..=153).contains(&p.values.len()), "1.5 s: {}", p.values.len());
        let middle = &p.values[20..120];
        let (lo, hi) = middle
            .iter()
            .fold((1f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert!(
            lo > 0.05 && hi - lo < 0.05,
            "a tone is equally loud throughout: {lo}..{hi}"
        );
    }
}
