//! Features of one audio stream, worked out while it is decoded: the
//! loudness of every 10 ms, and the spectral flux (how much new sound
//! appeared) of every 10 ms. Nothing else of the sound is kept, so a
//! recording of any length takes a few megabytes.

use crate::media::RATE;

/// One step of the feature series: 10 ms.
pub const HOP: usize = (RATE / 100) as usize;
/// The analysis window of the flux: 32 ms.
const WINDOW: usize = 512;

/// The features of one stream, one value per [`HOP`].
#[derive(Clone, Debug, Default)]
pub struct Features {
    /// Time of the first value, seconds from the file's start.
    pub start: f64,
    /// Mean square of the samples (power), linear.
    pub power: Vec<f32>,
    /// Spectral flux: the summed rise of every frequency's magnitude since
    /// the step before, linear.
    pub flux: Vec<f32>,
}

impl Features {
    pub fn step() -> f64 {
        HOP as f64 / f64::from(RATE)
    }
}

/// Turns samples into [`Features`] as they arrive.
pub struct Extractor {
    pub features: Features,
    /// The last [`WINDOW`] samples.
    ring: Vec<f32>,
    filled: usize,
    pending: usize,
    pending_power: f64,
    window: Vec<f32>,
    previous: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Default for Extractor {
    fn default() -> Self {
        Extractor::new()
    }
}

impl Extractor {
    pub fn new() -> Extractor {
        let window = (0..WINDOW)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / WINDOW as f64).cos()) as f32)
            .collect();
        Extractor {
            features: Features::default(),
            ring: vec![0.0; WINDOW],
            filled: 0,
            pending: 0,
            pending_power: 0.0,
            window,
            previous: vec![0.0; WINDOW / 2 + 1],
            re: vec![0.0; WINDOW],
            im: vec![0.0; WINDOW],
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.ring[self.filled % WINDOW] = s;
            self.filled += 1;
            self.pending += 1;
            self.pending_power += f64::from(s) * f64::from(s);
            if self.pending == HOP {
                self.hop();
            }
        }
    }

    fn hop(&mut self) {
        self.features.power.push((self.pending_power / HOP as f64) as f32);
        self.pending = 0;
        self.pending_power = 0.0;
        for i in 0..WINDOW {
            // Oldest sample first.
            let s = self.ring[(self.filled + i) % WINDOW];
            self.re[i] = s * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        // 62 Hz to 7.8 kHz: leaves out hum and the empty top.
        let mut flux = 0.0f32;
        for k in 2..=WINDOW / 2 - 6 {
            let m = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt();
            flux += (m - self.previous[k]).max(0.0);
            self.previous[k] = m;
        }
        self.features.flux.push(flux / WINDOW as f32);
    }
}

/// In-place radix-2 FFT; the length must be a power of two.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f64).sin_cos();
                let (c, s) = (c as f32, s as f32);
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_sine() {
        let n = 64;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 5.0 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mags: Vec<f32> = (0..n / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        let peak = mags
            .iter()
            .enumerate()
            .fold(0, |b, (k, &m)| if m > mags[b] { k } else { b });
        assert_eq!(peak, 5);
        assert!((mags[5] - n as f32 / 2.0).abs() < 1e-3);
    }

    #[test]
    fn power_and_flux_follow_a_burst() {
        let mut x = Extractor::new();
        let quiet = vec![0.0f32; HOP * 50];
        let loud: Vec<f32> = (0..HOP * 50).map(|i| (i as f32 * 0.3).sin() * 0.5).collect();
        x.push(&quiet);
        x.push(&loud);
        let f = &x.features;
        assert_eq!(f.power.len(), 100);
        assert!(f.power[10] < 1e-9);
        assert!((f.power[80] - 0.125).abs() < 0.01);
        // The flux jumps where the sound starts and settles after.
        let onset = f.flux[50..55].iter().cloned().fold(0.0, f32::max);
        assert!(onset > 10.0 * f.flux[90].max(1e-6));
    }
}
