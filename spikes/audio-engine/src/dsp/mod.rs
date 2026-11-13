//! The effects. Shared by every design: the designs differ only in how a
//! track's effects are held and called, and how settings reach them.

pub mod biquad;
pub mod duck;
pub mod dynamics;
pub mod fader;
pub mod filters;
pub mod gate;
pub mod limiter;
pub mod meter;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Hands one track's sound to a worker that draws the analyser (only the
/// track shown on the page has one). Never waits: when the worker is
/// behind, the block is skipped.
pub struct Tap {
    pub to: rtrb::Producer<f32>,
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tap")
    }
}

impl Tap {
    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        if let Ok(mut chunk) = self.to.write_chunk(l.len()) {
            let (a, b) = chunk.as_mut_slices();
            let (la, lb) = l.split_at(a.len());
            let (ra, rb) = r.split_at(a.len());
            for ((o, x), y) in a.iter_mut().zip(la).zip(ra) {
                *o = 0.5 * (x + y);
            }
            for ((o, x), y) in b.iter_mut().zip(lb).zip(rb) {
                *o = 0.5 * (x + y);
            }
            chunk.commit_all();
        }
    }
}

/// The analyser's worker: 2048-point spectra of what the tap sends,
/// published as 64 bands of atomics.
pub fn analyser(
    mut from: rtrb::Consumer<f32>,
    bands: Arc<[AtomicU32; 64]>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use realfft::RealFftPlanner;
    const N: usize = 2048;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let mut frame = vec![0f32; N];
    let mut spec = fft.make_output_vec();
    let mut fill = 0;
    while !stop.load(Ordering::Relaxed) {
        let n = from.slots();
        if n == 0 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        if let Ok(chunk) = from.read_chunk(n.min(N - fill)) {
            let (a, b) = chunk.as_slices();
            frame[fill..fill + a.len()].copy_from_slice(a);
            frame[fill + a.len()..fill + a.len() + b.len()].copy_from_slice(b);
            fill += a.len() + b.len();
            chunk.commit_all();
        }
        if fill == N {
            fill = 0;
            let _ = fft.process(&mut frame, &mut spec);
            for (i, band) in bands.iter().enumerate() {
                let m = spec[i * 16..(i + 1) * 16]
                    .iter()
                    .map(|c| c.norm_sqr())
                    .sum::<f32>();
                band.store((10.0 * (m + 1e-12).log10()).to_bits(), Ordering::Relaxed);
            }
        }
    }
}
