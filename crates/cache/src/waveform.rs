//! Waveforms: how loud a sound stream is, a hundred times a second.
//!
//! Reading a stream once gives its peaks; the timeline draws any zoom level
//! from them. A 20-minute stream becomes 120,000 numbers (half a megabyte),
//! so they stay in memory. Reading runs on a worker: decoding all the sound
//! of a long recording takes a while, so every stream of a file is read in
//! the same pass ([`peaks_of_streams`]: the file is read once, not once per
//! stream), at the stream's own rate (a peak needs no resampling), and the
//! peaks are kept on disk by the recording's fingerprint ([`Store`]), so a
//! file is read once, not once per session.

use media::AudioStreams;
use media::audio::{CHANNELS, OWN_RATE};
use project::Fingerprint;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Peaks per second of sound.
pub const PER_SECOND: u32 = 100;

/// Sample frames decoded between two hand-overs of new peaks.
const STEP: usize = 48_000;

/// The loudest sample (0 to 1) in each 1/100 s of a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    pub per_second: u32,
    pub values: Arc<[f32]>,
}

/// Reads stream `stream` of the file and returns its peaks.
pub fn peaks(path: &Path, stream: u32) -> diag::Result<Peaks> {
    peaks_as_read(path, stream, |_| true)
}

/// Like [`peaks`], handing `more` every new run of peaks as soon as it is
/// read, in order from the start of the stream (a second of sound or so at
/// a time), so a waveform can be drawn and progress told while the rest is
/// still being read. `more` returns whether to go on: `false` stops the
/// reading, which then returns an error.
pub fn peaks_as_read(path: &Path, stream: u32, mut more: impl FnMut(&[f32]) -> bool) -> diag::Result<Peaks> {
    let mut all = peaks_of_streams(path, &[stream], |_, values| more(values))?;
    all.pop().ok_or_else(stopped)
}

/// The peaks of several streams of one file (in the order of `streams`),
/// read in one pass over the file. `more` is handed each stream's new
/// peaks as they are read (its place in `streams`, and the peaks), and
/// returns whether to go on: `false` stops the reading, which then returns
/// an error.
pub fn peaks_of_streams(
    path: &Path,
    streams: &[u32],
    mut more: impl FnMut(usize, &[f32]) -> bool,
) -> diag::Result<Vec<Peaks>> {
    let mut reader = AudioStreams::open(path, streams, OWN_RATE)?;
    let mut buckets: Vec<Buckets> = reader.rates().into_iter().map(Buckets::new).collect();
    let mut out = vec![Vec::new(); streams.len()];
    loop {
        let going = reader.read(STEP, &mut out);
        for (i, (samples, bucket)) in out.iter_mut().zip(&mut buckets).enumerate() {
            let told = bucket.values.len();
            bucket.add(samples);
            if !going {
                bucket.finish();
            }
            samples.clear();
            if bucket.values.len() > told && !more(i, &bucket.values[told..]) {
                return Err(stopped());
            }
        }
        if !going {
            break;
        }
    }
    Ok(buckets
        .into_iter()
        .map(|b| Peaks {
            per_second: PER_SECOND,
            values: b.values.into(),
        })
        .collect())
}

/// Turns one stream's samples into peaks as they come.
struct Buckets {
    rate: u64,
    values: Vec<f32>,
    /// The loudest sample of the peak being filled, and whether any is in it.
    current: f32,
    started: bool,
    /// Samples counted so far, and where the peak being filled ends.
    sample: u64,
    ends_at: u64,
}

impl Buckets {
    fn new(rate: u32) -> Buckets {
        let mut b = Buckets {
            rate: u64::from(rate.max(1)),
            values: Vec::new(),
            current: 0.0,
            started: false,
            sample: 0,
            ends_at: 0,
        };
        b.ends_at = b.end_of(0);
        b
    }

    /// Peak `n` covers samples from n·rate/100 up to (n+1)·rate/100, rounded
    /// down: whole samples, none counted twice, at any rate.
    fn end_of(&self, n: u64) -> u64 {
        (n + 1) * self.rate / u64::from(PER_SECOND)
    }

    /// Takes interleaved stereo samples.
    fn add(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(CHANNELS) {
            self.current = self.current.max(frame.iter().fold(0f32, |m, s| m.max(s.abs())));
            self.started = true;
            self.sample += 1;
            if self.sample == self.ends_at {
                self.values.push(self.current.min(1.0));
                self.current = 0.0;
                self.started = false;
                self.ends_at = self.end_of(self.values.len() as u64);
            }
        }
    }

    /// The stream has ended: a peak partly filled counts too.
    fn finish(&mut self) {
        if self.started {
            self.values.push(self.current.min(1.0));
            self.started = false;
        }
    }
}

/// Peaks kept on disk, one small file per recording and sound stream,
/// named by the recording's fingerprint (as proxies are), so a waveform
/// read once is there the next time any project uses that recording. Each
/// peak is kept as a 16-bit number (finer than a pixel at any height).
/// Unlike proxies they are small (a three-hour stream is about 2 MB), so
/// the folder only keeps the most recently used within its budget
/// ([`Store::trim`]).
#[derive(Debug, Clone)]
pub struct Store {
    folder: PathBuf,
}

/// What a peaks file starts with: its kind and format version.
const MAGIC: &[u8; 8] = b"PEAKS\0\0\x01";

impl Store {
    pub fn new(folder: PathBuf) -> Store {
        Store { folder }
    }

    fn file_for(&self, fingerprint: &Fingerprint, stream: u32) -> PathBuf {
        self.folder.join(format!(
            "{}-{}-{stream}.peaks",
            fingerprint.size, fingerprint.sample_hash
        ))
    }

    /// The peaks kept for this stream, if any (and readable).
    pub fn load(&self, fingerprint: &Fingerprint, stream: u32) -> Option<Peaks> {
        let path = self.file_for(fingerprint, stream);
        let bytes = std::fs::read(&path).ok()?;
        let peaks = decode(&bytes);
        if peaks.is_some() {
            // Its last use: what `trim` keeps the newest of.
            let _ = std::fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        } else {
            tracing::info!(file = %path.display(), "a kept waveform could not be read; reading the sound again");
        }
        peaks
    }

    /// Keeps `peaks` for this stream. Failing only costs reading it again.
    pub fn save(&self, fingerprint: &Fingerprint, stream: u32, peaks: &Peaks) {
        let written = std::fs::create_dir_all(&self.folder)
            .and_then(|()| project::disk::write_atomic(&self.file_for(fingerprint, stream), &encode(peaks)));
        if let Err(err) = written {
            tracing::info!("a waveform could not be kept on disk: {err}");
        }
    }

    /// Removes every waveform kept (Settings › Cache): they are read from
    /// the recordings again when next needed. Returns how many went.
    pub fn clear(&self) -> usize {
        self.trim(0)
    }

    /// Removes the waveforms used longest ago until those left take at
    /// most `budget` bytes. Returns how many went.
    pub fn trim(&self, budget: u64) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return 0;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "peaks"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), e.path()))
            })
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        let mut kept = 0u64;
        let mut removed = 0;
        for (_, len, path) in files {
            kept += len;
            if kept > budget && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

fn encode(peaks: &Peaks) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + peaks.values.len() * 2);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&peaks.per_second.to_le_bytes());
    bytes.extend_from_slice(&(peaks.values.len() as u32).to_le_bytes());
    for v in peaks.values.iter() {
        bytes.extend_from_slice(&((v.clamp(0.0, 1.0) * 65_535.0).round() as u16).to_le_bytes());
    }
    bytes
}

fn decode(bytes: &[u8]) -> Option<Peaks> {
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let number = |at: usize| Some(u32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?));
    let (per_second, count) = (number(0)?, number(4)? as usize);
    let values = rest.get(8..)?;
    if per_second != PER_SECOND || values.len() != count * 2 {
        return None;
    }
    Some(Peaks {
        per_second,
        values: values
            .chunks_exact(2)
            .map(|b| f32::from(u16::from_le_bytes([b[0], b[1]])) / 65_535.0)
            .collect(),
    })
}

fn stopped() -> diag::AppError {
    diag::AppError::new("Reading the sound was stopped")
        .why("The waveform was no longer needed, or the app is closing.")
        .fix("Nothing to do: it is read again when it is needed.")
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

    #[test]
    fn peaks_are_handed_over_as_they_are_read() {
        let mut so_far = Vec::new();
        let mut runs = 0;
        let p = peaks_as_read(&fixture("voice.mp3"), 0, |more| {
            assert!(!more.is_empty());
            so_far.extend_from_slice(more);
            runs += 1;
            true
        })
        .unwrap();
        assert!(runs >= 1);
        assert_eq!(so_far, &p.values[..], "every peak exactly once, in order");
        assert_eq!(p, peaks(&fixture("voice.mp3"), 0).unwrap());
    }

    #[test]
    fn every_stream_of_a_file_is_read_in_one_pass_as_it_is_alone() {
        let path = fixture("game-3-audio.mkv");
        let mut runs = [0, 0, 0];
        let all = peaks_of_streams(&path, &[1, 2, 3], |i, more| {
            assert!(!more.is_empty());
            runs[i] += 1;
            true
        })
        .unwrap();
        assert!(runs.iter().all(|r| *r >= 1));
        for (i, stream) in [1, 2, 3].into_iter().enumerate() {
            assert_eq!(all[i], peaks(&path, stream).unwrap(), "stream {stream}");
        }
    }

    #[test]
    fn peaks_are_kept_on_disk_by_fingerprint_and_trimmed_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("waveforms"));
        let print = |n: u64| Fingerprint {
            size: n,
            sample_hash: format!("{:x}", n * 7),
        };
        let peaks = Peaks {
            per_second: PER_SECOND,
            values: vec![0.0, 0.25, 1.0, 0.5].into(),
        };
        assert_eq!(store.load(&print(1), 0), None, "nothing kept yet");
        store.save(&print(1), 0, &peaks);
        let back = store.load(&print(1), 0).unwrap();
        assert!(
            back.values
                .iter()
                .zip(peaks.values.iter())
                .all(|(a, b)| (a - b).abs() < 1e-4),
            "{back:?}"
        );
        assert_eq!(store.load(&print(1), 1), None, "another stream");
        std::fs::write(store.file_for(&print(2), 0), b"not peaks").unwrap();
        assert_eq!(store.load(&print(2), 0), None, "a damaged file is read again");
        // Older first: the one saved first goes when the budget is tight.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .append(true)
            .open(store.file_for(&print(2), 0))
            .unwrap()
            .set_modified(old)
            .unwrap();
        store.save(&print(3), 0, &peaks);
        assert_eq!(store.trim(50), 1, "two of 24 bytes fit");
        assert!(store.load(&print(3), 0).is_some() && !store.file_for(&print(2), 0).exists());
        assert_eq!(store.clear(), 2, "clearing removes them all");
        assert_eq!(store.load(&print(3), 0), None);
    }

    #[test]
    fn reading_stops_when_asked() {
        let err = peaks_as_read(&fixture("voice.mp3"), 0, |_| false).unwrap_err();
        assert!(err.what().contains("stopped"), "{}", err.what());
    }
}
