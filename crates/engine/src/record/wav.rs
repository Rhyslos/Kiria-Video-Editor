//! Writing a recording as a WAV file: 32-bit floating-point samples, as the
//! sound card gives them, so nothing is rounded or clipped on the way.
//!
//! The header's two sizes are written as zero first and set when the file
//! is finished, so a recording cut off by a crash still opens (FFmpeg reads
//! such a file to its end).

use diag::{AppError, Explain};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The header's length: RIFF, the format chunk (18 bytes for floating
/// point, with its extra size), and the data chunk's start.
const HEADER_BYTES: u32 = 12 + 8 + 18 + 8;

/// A WAV file being written.
#[derive(Debug)]
pub struct WavWriter {
    out: BufWriter<File>,
    path: PathBuf,
    channels: u16,
    frames: u64,
}

impl WavWriter {
    /// Starts a file at `path` (made, or replaced) of `channels` at `rate`.
    pub fn create(path: &Path, rate: u32, channels: u16) -> diag::Result<WavWriter> {
        let file = File::create(path).or_explain(|| AppError::new("The recording's file can't be made"))?;
        let mut out = BufWriter::with_capacity(1 << 16, file);
        let block = channels * 4;
        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&0u32.to_le_bytes()); // set at the end
        header.extend_from_slice(b"WAVE");
        header.extend_from_slice(b"fmt ");
        header.extend_from_slice(&18u32.to_le_bytes());
        header.extend_from_slice(&3u16.to_le_bytes()); // IEEE floating point
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&rate.to_le_bytes());
        header.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        header.extend_from_slice(&block.to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // no extra format bytes
        header.extend_from_slice(b"data");
        header.extend_from_slice(&0u32.to_le_bytes()); // set at the end
        out.write_all(&header)
            .or_explain(|| AppError::new("The recording's file can't be written"))?;
        Ok(WavWriter {
            out,
            path: path.to_path_buf(),
            channels,
            frames: 0,
        })
    }

    /// Adds samples, interleaved by channel (whole frames).
    pub fn write(&mut self, samples: &[f32]) -> diag::Result<()> {
        for sample in samples {
            self.out
                .write_all(&sample.to_le_bytes())
                .or_explain(|| AppError::new("The recording's file can't be written"))?;
        }
        self.frames += (samples.len() / usize::from(self.channels.max(1))) as u64;
        Ok(())
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Sets the header's sizes and closes the file; returns its path.
    pub fn finish(mut self) -> diag::Result<PathBuf> {
        let data = self.frames * u64::from(self.channels) * 4;
        // A WAV file's sizes are 32 bits: past 4 GB (over three hours of
        // mono at 96 kHz) they stay at their largest, which FFmpeg reads
        // as "to the end".
        let data32 = u32::try_from(data).unwrap_or(u32::MAX);
        let riff = u32::try_from(data + u64::from(HEADER_BYTES) - 8).unwrap_or(u32::MAX);
        let explain = || AppError::new("The recording's file can't be finished");
        self.out.flush().or_explain(explain)?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(4)).or_explain(explain)?;
        file.write_all(&riff.to_le_bytes()).or_explain(explain)?;
        file.seek(SeekFrom::Start(u64::from(HEADER_BYTES) - 4))
            .or_explain(explain)?;
        file.write_all(&data32.to_le_bytes()).or_explain(explain)?;
        file.sync_all().or_explain(explain)?;
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_file_has_its_sizes_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let mut wav = WavWriter::create(&path, 48_000, 1).unwrap();
        let samples: Vec<f32> = (0..4800).map(|i| (i as f32 / 4800.0) - 0.5).collect();
        wav.write(&samples[..1000]).unwrap();
        wav.write(&samples[1000..]).unwrap();
        assert_eq!(wav.frames(), 4800);
        let path = wav.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3, "floating point");
        assert_eq!(
            &bytes[HEADER_BYTES as usize - 8..HEADER_BYTES as usize - 4],
            b"data"
        );
        assert_eq!(
            u32::from_le_bytes(
                bytes[HEADER_BYTES as usize - 4..HEADER_BYTES as usize]
                    .try_into()
                    .unwrap()
            ),
            4800 * 4
        );
        let first = f32::from_le_bytes(
            bytes[HEADER_BYTES as usize..HEADER_BYTES as usize + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(first, -0.5);
    }

    /// FFmpeg reads it as one stream of sound of the length written, as the
    /// import's check does.
    #[test]
    fn the_import_check_reads_a_written_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let mut wav = WavWriter::create(&path, 48_000, 1).unwrap();
        let tone: Vec<f32> = (0..48_000 * 2)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.25)
            .collect();
        wav.write(&tone).unwrap();
        let path = wav.finish().unwrap();
        let media = media::check(&path).unwrap();
        assert_eq!(media.kind, project::MediaKind::Audio);
        assert!(media.info.video.is_none());
        assert_eq!(media.info.audio.len(), 1);
        assert_eq!(media.info.audio[0].channels, 1);
        assert_eq!(media.info.audio[0].sample_rate, 48_000);
        let seconds = media.duration.as_secs_f64();
        assert!((seconds - 2.0).abs() < 0.01, "{seconds}");
    }
}
