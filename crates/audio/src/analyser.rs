//! The analyser behind the EQ: the spectrum of the track the Audio page
//! shows, drawn from work done off the real-time thread.
//!
//! The mixer's [`Tap`] copies that track's sound (after its EQ, both
//! channels mixed to one) into a lock-free ring and never waits: when the
//! worker is behind, a block is skipped. The worker calls
//! [`Analyser::step`] every frame or so; it takes 2048-sample windows (a
//! new one every 1024), turns each into frequencies and publishes the
//! level in the 64 bands of [`crate::dsp::bands`] to a [`Spectrum`] the
//! interface reads without locks. Levels fall back slowly (about 20 dB a
//! second) so the drawing doesn't flicker.

use crate::dsp::bands;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Samples per analysis window.
pub const WINDOW: usize = 2048;
/// A new window every this many samples.
const HOP: usize = WINDOW / 2;
/// Samples the ring holds: about 85 ms at 48 kHz, plenty for a worker
/// that looks every frame.
const RING: usize = 4 * WINDOW;

/// The spectrum the interface draws: dBFS per band (64, by frequency).
#[derive(Debug)]
pub struct Spectrum {
    bands: [AtomicU32; bands::COUNT],
}

impl Default for Spectrum {
    fn default() -> Spectrum {
        Spectrum {
            bands: std::array::from_fn(|_| AtomicU32::new((-120f32).to_bits())),
        }
    }
}

impl Spectrum {
    /// The levels now, dBFS per band (band `i` is around `bands::centre(i)`).
    pub fn levels(&self) -> [f32; bands::COUNT] {
        std::array::from_fn(|i| f32::from_bits(self.bands[i].load(Ordering::Relaxed)))
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
    /// Sends a block (both channels mixed); skipped whole when there is no room.
    pub fn send(&mut self, l: &[f32], r: &[f32]) {
        let n = l.len().min(r.len());
        if let Ok(mut chunk) = self.to.write_chunk(n) {
            let (first, second) = chunk.as_mut_slices();
            for (i, o) in first.iter_mut().chain(second.iter_mut()).enumerate() {
                *o = 0.5 * (l[i] + r[i]);
            }
            chunk.commit_all();
        }
    }
}

/// The worker's end.
pub struct Analyser {
    from: rtrb::Consumer<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    held: Vec<f32>,
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
    let (to, from) = rtrb::RingBuffer::new(RING);
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
            held: Vec::with_capacity(WINDOW),
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
            let want = WINDOW - self.held.len();
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
            if self.held.len() == WINDOW {
                self.analyse();
                self.held.drain(..HOP);
                any = true;
            }
        }
    }

    fn analyse(&mut self) {
        for ((f, x), w) in self.frame.iter_mut().zip(&self.held).zip(&self.window) {
            *f = x * w;
        }
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
    fn a_full_ring_skips_blocks_instead_of_waiting() {
        let (mut tap, mut analyser) = analyser(48_000);
        let block = vec![0.1f32; 256];
        for _ in 0..RING / 256 + 10 {
            tap.send(&block, &block);
        }
        assert!(analyser.step());
    }
}
