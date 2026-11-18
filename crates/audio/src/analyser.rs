//! The analyser behind the EQ and the Audio page's band: the spectrum and
//! the stereo picture of one track or of the mix, drawn from work done off
//! the real-time thread.
//!
//! The mixer's [`Tap`] copies that sound (a track after its EQ, or the
//! mix after the master; both channels) into a lock-free ring and never
//! waits: when the worker is behind, a block is skipped. The worker calls
//! [`Analyser::step`] every frame or so; it takes 2048-frame windows (a
//! new one every 1024), turns each (the channels mixed to one) into
//! frequencies and publishes the level in the 64 bands of
//! [`crate::dsp::bands`] to a [`Spectrum`] the interface reads without
//! locks. Levels fall back slowly (about 20 dB a second) so the drawing
//! doesn't flicker. Each window also gives the [`Stereo`] picture: how
//! alike the channels are, which side is louder, and a few hundred of its
//! frames as points for a picture of left against right.

use crate::dsp::bands;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Frames per analysis window.
pub const WINDOW: usize = 2048;
/// The frames of a window kept as points for the stereo picture.
pub const POINTS: usize = 256;
/// A new window every this many samples.
const HOP: usize = WINDOW / 2;
/// Frames the ring holds (two samples each): about 170 ms at 48 kHz,
/// plenty for a worker that looks every frame.
const RING: usize = 4 * WINDOW;

/// The spectrum the interface draws: dBFS per band (64, by frequency),
/// and the stereo picture of the same sound.
#[derive(Debug)]
pub struct Spectrum {
    bands: [AtomicU32; bands::COUNT],
    /// Correlation, balance and how many windows there have been.
    stereo: [AtomicU32; 3],
    /// Left and right of the newest window's points, one after the other.
    points: [AtomicU32; 2 * POINTS],
}

/// The stereo picture of the newest window.
#[derive(Debug, Clone, PartialEq)]
pub struct Stereo {
    /// How alike the channels are: 1 the same (mono), 0 unrelated, -1
    /// opposite (they cancel when mixed to one). NaN in silence.
    pub correlation: f32,
    /// Which side is louder: -1 all left, 0 even, 1 all right.
    pub balance: f32,
    /// Windows analysed since made: a new one is a new picture.
    pub windows: u32,
    /// Some of the window's frames, (left, right).
    pub points: Vec<(f32, f32)>,
}

impl Default for Spectrum {
    fn default() -> Spectrum {
        Spectrum {
            bands: std::array::from_fn(|_| AtomicU32::new((-120f32).to_bits())),
            stereo: [
                AtomicU32::new(f32::NAN.to_bits()),
                AtomicU32::new(0f32.to_bits()),
                AtomicU32::new(0),
            ],
            points: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

impl Spectrum {
    /// The levels now, dBFS per band (band `i` is around `bands::centre(i)`).
    pub fn levels(&self) -> [f32; bands::COUNT] {
        std::array::from_fn(|i| f32::from_bits(self.bands[i].load(Ordering::Relaxed)))
    }

    /// The stereo picture now. Read while it is written, a few points may
    /// be from the window before: harmless in a picture.
    pub fn stereo(&self) -> Stereo {
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        Stereo {
            correlation: f(&self.stereo[0]),
            balance: f(&self.stereo[1]),
            windows: self.stereo[2].load(Ordering::Acquire),
            points: (0..POINTS)
                .map(|i| (f(&self.points[i]), f(&self.points[POINTS + i])))
                .collect(),
        }
    }

    fn put_stereo(&self, correlation: f32, balance: f32, left: &[f32], right: &[f32]) {
        for (slot, v) in self.points.iter().zip(left.iter().chain(right)) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
        self.stereo[0].store(correlation.to_bits(), Ordering::Relaxed);
        self.stereo[1].store(balance.to_bits(), Ordering::Relaxed);
        self.stereo[2].fetch_add(1, Ordering::Release);
    }

    fn put(&self, levels: &[f32; bands::COUNT]) {
        for (slot, v) in self.bands.iter().zip(levels) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
    }
}

/// The mixer's end: hands sound to the analyser, never waiting.
pub struct Tap {
    to: rtrb::Producer<f32>,
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tap")
    }
}

impl Tap {
    /// Sends a block of two channels; skipped whole when there is no room.
    pub fn send(&mut self, l: &[f32], r: &[f32]) {
        let n = l.len().min(r.len());
        if let Ok(mut chunk) = self.to.write_chunk(2 * n) {
            let (first, second) = chunk.as_mut_slices();
            for (i, o) in first.iter_mut().chain(second.iter_mut()).enumerate() {
                *o = if i % 2 == 0 { l[i / 2] } else { r[i / 2] };
            }
            chunk.commit_all();
        }
    }

    /// Sends frames already side by side (left, right, left…); skipped
    /// whole when there is no room.
    pub fn send_frames(&mut self, frames: &[f32]) {
        let n = frames.len() / 2 * 2;
        if let Ok(mut chunk) = self.to.write_chunk(n) {
            let (first, second) = chunk.as_mut_slices();
            let split = first.len();
            first.copy_from_slice(&frames[..split]);
            second.copy_from_slice(&frames[split..n]);
            chunk.commit_all();
        }
    }
}

/// The worker's end.
pub struct Analyser {
    from: rtrb::Consumer<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Frames waiting, left and right side by side.
    held: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    frame: Vec<f32>,
    spec: Vec<Complex<f32>>,
    power: Vec<f32>,
    levels: [f32; bands::COUNT],
    spectrum: Arc<Spectrum>,
    rate: f32,
    /// How far a level may fall per window (dB).
    fall_db: f32,
}

impl std::fmt::Debug for Analyser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyser")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

/// A tap and its analyser, for sound at `rate`.
pub fn analyser(rate: u32) -> (Tap, Analyser) {
    let (to, from) = rtrb::RingBuffer::new(2 * RING);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
    let spec = fft.make_output_vec();
    // A Hann window, scaled so a full-scale sine reads 0 dBFS at its band.
    let window: Vec<f32> = (0..WINDOW)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WINDOW as f32).cos())
        .collect();
    let windows_per_second = rate as f32 / HOP as f32;
    (
        Tap { to },
        Analyser {
            from,
            fft,
            window,
            held: Vec::with_capacity(2 * WINDOW),
            left: vec![0.0; POINTS],
            right: vec![0.0; POINTS],
            frame: vec![0.0; WINDOW],
            spec,
            power: vec![0.0; WINDOW / 2 + 1],
            levels: [-120.0; bands::COUNT],
            spectrum: Arc::new(Spectrum::default()),
            rate: rate as f32,
            fall_db: 20.0 / windows_per_second,
        },
    )
}

impl Analyser {
    /// What the interface reads.
    pub fn spectrum(&self) -> Arc<Spectrum> {
        Arc::clone(&self.spectrum)
    }

    /// Takes in what the tap sent and publishes a spectrum for each whole
    /// window. Returns whether there was anything new.
    pub fn step(&mut self) -> bool {
        let mut any = false;
        loop {
            let want = 2 * WINDOW - self.held.len();
            let n = self.from.slots().min(want);
            if n == 0 {
                return any;
            }
            if let Ok(chunk) = self.from.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                self.held.extend_from_slice(a);
                self.held.extend_from_slice(b);
                chunk.commit_all();
            }
            if self.held.len() == 2 * WINDOW {
                self.analyse();
                self.held.drain(..2 * HOP);
                any = true;
            }
        }
    }

    fn analyse(&mut self) {
        for ((f, x), w) in self
            .frame
            .iter_mut()
            .zip(self.held.chunks_exact(2))
            .zip(&self.window)
        {
            *f = 0.5 * (x[0] + x[1]) * w;
        }
        self.stereo();
        // Lengths are the plan's own, so it can't fail.
        let _ = self.fft.process(&mut self.frame, &mut self.spec);
        // A Hann window's gain is a half; a sine's peak splits over two halves.
        let scale = 2.0 / (0.5 * WINDOW as f32);
        for (p, c) in self.power.iter_mut().zip(&self.spec) {
            *p = c.norm_sqr() * scale * scale;
        }
        // Each band shows its loudest frequency, so a tone reads its level
        // however wide the band; a band narrower than a frequency step shows
        // the one nearest its centre.
        let bin_hz = self.rate / WINDOW as f32;
        let mut bands_now = [0f32; bands::COUNT];
        let mut seen = [false; bands::COUNT];
        for (k, p) in self.power.iter().enumerate().skip(1) {
            let at = bands::position(k as f32 * bin_hz);
            if (0.0..bands::COUNT as f32).contains(&at) {
                let b = at as usize;
                bands_now[b] = bands_now[b].max(*p);
                seen[b] = true;
            }
        }
        let last = self.power.len() - 1;
        for (b, now) in bands_now.iter_mut().enumerate() {
            if !seen[b] {
                let k = ((bands::centre(b) / bin_hz).round() as usize).clamp(1, last);
                *now = self.power[k];
            }
        }
        for (level, now) in self.levels.iter_mut().zip(bands_now) {
            let db = 10.0 * (now + 1e-12).log10();
            *level = db.max(*level - self.fall_db).max(-120.0);
        }
        self.spectrum.put(&self.levels);
    }

    /// The window's stereo picture: correlation, balance and points.
    fn stereo(&mut self) {
        let (mut ll, mut rr, mut lr) = (0f64, 0f64, 0f64);
        for x in self.held.chunks_exact(2) {
            let (l, r) = (f64::from(x[0]), f64::from(x[1]));
            ll += l * l;
            rr += r * r;
            lr += l * r;
        }
        // Under about -90 dBFS on both: silence, nothing to say.
        let quiet = (ll + rr) / (2 * WINDOW) as f64 <= 1e-9;
        let correlation = if quiet {
            f32::NAN
        } else if ll <= 1e-12 || rr <= 1e-12 {
            0.0
        } else {
            (lr / (ll * rr).sqrt()) as f32
        };
        let balance = if quiet {
            0.0
        } else {
            ((rr - ll) / (rr + ll)) as f32
        };
        let step = WINDOW / POINTS;
        for (i, x) in self.held.chunks_exact(2).step_by(step).take(POINTS).enumerate() {
            self.left[i] = x[0];
            self.right[i] = x[1];
        }
        self.spectrum
            .put_stereo(correlation, balance, &self.left, &self.right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signal::sine;

    #[test]
    fn a_tone_shows_in_its_band_at_its_level() {
        let (mut tap, mut analyser) = analyser(48_000);
        let spectrum = analyser.spectrum();
        let tone = sine(48_000.0, 1_000.0, 0.5, 8_192);
        for block in tone.chunks(256) {
            tap.send(block, block);
            analyser.step();
        }
        let levels = spectrum.levels();
        let band = bands::band_of(1_000.0);
        assert!((levels[band] + 6.0).abs() < 1.5, "{}", levels[band]);
        assert!(levels[bands::band_of(100.0)] < -40.0);
        assert!(levels[bands::band_of(10_000.0)] < -40.0);
    }

    #[test]
    fn the_stereo_picture_tells_mono_wide_and_opposite_apart() {
        let picture = |l: &[f32], r: &[f32]| {
            let (mut tap, mut analyser) = analyser(48_000);
            let spectrum = analyser.spectrum();
            for (a, b) in l.chunks(256).zip(r.chunks(256)) {
                tap.send(a, b);
                analyser.step();
            }
            spectrum.stereo()
        };
        let tone = sine(48_000.0, 440.0, 0.5, 4_096);
        let other = sine(48_000.0, 1_330.0, 0.5, 4_096);
        let opposite: Vec<f32> = tone.iter().map(|s| -s).collect();
        let quiet: Vec<f32> = tone.iter().map(|s| s * 0.25).collect();
        let mono = picture(&tone, &tone);
        assert!((mono.correlation - 1.0).abs() < 1e-3 && mono.balance.abs() < 1e-3);
        assert!(mono.windows > 0 && mono.points.iter().all(|(l, r)| l == r));
        assert!(picture(&tone, &other).correlation.abs() < 0.1, "unrelated");
        assert!((picture(&tone, &opposite).correlation + 1.0).abs() < 1e-3);
        assert!(picture(&tone, &quiet).balance < -0.8, "left louder");
        let silence = vec![0.0; 4_096];
        assert!(picture(&silence, &silence).correlation.is_nan());
    }

    #[test]
    fn frames_side_by_side_read_as_two_blocks_would() {
        let tone = sine(48_000.0, 1_000.0, 0.5, 8_192);
        let quiet: Vec<f32> = tone.iter().map(|s| s * 0.5).collect();
        let (mut tap, mut analyser) = analyser(48_000);
        let (mut tap2, mut analyser2) = analyser(48_000);
        for (a, b) in tone.chunks(256).zip(quiet.chunks(256)) {
            tap.send(a, b);
            let frames: Vec<f32> = a.iter().zip(b).flat_map(|(l, r)| [*l, *r]).collect();
            tap2.send_frames(&frames);
            analyser.step();
            analyser2.step();
        }
        assert_eq!(analyser.spectrum().levels(), analyser2.spectrum().levels());
        assert_eq!(analyser.spectrum().stereo(), analyser2.spectrum().stereo());
    }

    #[test]
    fn a_full_ring_skips_blocks_instead_of_waiting() {
        let (mut tap, mut analyser) = analyser(48_000);
        let block = vec![0.1f32; 256];
        for _ in 0..RING / 256 + 10 {
            tap.send(&block, &block);
        }
        assert!(analyser.step());
    }
}
