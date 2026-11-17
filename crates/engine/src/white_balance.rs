//! Auto white balance for the Color page: how warm or cool, green or
//! magenta the light was in a clip, from a handful of frames across it,
//! as the Temperature and Tint that take it back to neutral.
//!
//! Each frame is read small and turned into light. Clipped, near-black and
//! strongly coloured pixels are left out (they say nothing about the light,
//! and a game's own colours are mostly the strongly coloured ones). Of
//! what is left, two estimates of the light's colour are taken and
//! combined: the mean of everything (grey world) and the mean of the
//! brightest near-neutral part (white patch). A strong cast makes greys
//! look coloured too, so this is done three times, each on the picture as
//! the last estimate would correct it, with a stricter idea of what counts
//! as neutral (skin is left out by the last). See
//! `docs/research/auto-white-balance.md` for how they compare.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use media::{Decoding, VideoDecoder};
use project::Time;

/// How many frames across the clip are looked at.
pub const FRAMES: usize = 8;

/// A pixel brighter than this in any channel is clipped (light, 0 to 1).
const CLIPPED: f32 = 0.92;
/// Darker than this, a pixel is mostly noise.
const NEAR_BLACK: f32 = 0.01;
/// More saturated than this ((most - least) / most), a pixel is a colour
/// of its own rather than the light's: loose at first, while a cast still
/// colours the greys, strict at the end.
const STRONG_COLOUR: [f32; 3] = [0.5, 0.35, 0.25];
/// How much of the brightest near-neutral part makes the white patch.
const BRIGHTEST: f32 = 0.05;

/// The light's colour as seen, red, green and blue (any scale).
pub type Light = [f32; 3];

/// Light from a display value (BT.709's transfer, as the compositor reads it).
fn to_light(value: u8) -> f32 {
    (f32::from(value) / 255.0).powf(2.4)
}

/// Every pixel of `frames` (RGBA, display values), as light.
pub fn light_of(frames: &[Vec<u8>]) -> Vec<Light> {
    frames
        .iter()
        .flat_map(|rgba| rgba.chunks_exact(4))
        .map(|p| [to_light(p[0]), to_light(p[1]), to_light(p[2])])
        .collect()
}

/// The pixels that say something about the light, as `light` would
/// correct them: neither clipped (as recorded) nor near black, nor more
/// coloured than `strong`.
pub fn usable(pixels: &[Light], light: Light, strong: f32) -> Vec<Light> {
    pixels
        .iter()
        .filter(|p| p[0].max(p[1]).max(p[2]) < CLIPPED)
        .map(|p| [p[0] / light[0], p[1] / light[1], p[2] / light[2]])
        .filter(|p| {
            let most = p[0].max(p[1]).max(p[2]);
            let least = p[0].min(p[1]).min(p[2]);
            most > NEAR_BLACK && (most - least) / most < strong
        })
        .collect()
}

fn mean(pixels: &[Light]) -> Option<Light> {
    if pixels.is_empty() {
        return None;
    }
    let n = pixels.len() as f32;
    let sum = pixels
        .iter()
        .fold([0.0f32; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
    Some([sum[0] / n, sum[1] / n, sum[2] / n])
}

/// Grey world: the mean of every usable pixel.
pub fn grey_world(pixels: &[Light]) -> Option<Light> {
    mean(pixels)
}

/// White patch: the mean of the brightest few of the usable pixels.
pub fn white_patch(pixels: &[Light]) -> Option<Light> {
    let mut sorted: Vec<Light> = pixels.to_vec();
    sorted.sort_by(|a, b| (b[0] + b[1] + b[2]).total_cmp(&(a[0] + a[1] + a[2])));
    let keep = ((sorted.len() as f32 * BRIGHTEST).ceil() as usize)
        .max(1)
        .min(sorted.len());
    mean(&sorted[..keep])
}

/// The two combined, each counting half (by geometric mean, so neither's
/// brightness matters).
pub fn combined(pixels: &[Light]) -> Option<Light> {
    let (a, b) = (grey_world(pixels)?, white_patch(pixels)?);
    let norm = |l: Light| {
        let s = (l[0] + l[1] + l[2]).max(1e-6);
        [l[0] / s, l[1] / s, l[2] / s]
    };
    let (a, b) = (norm(a), norm(b));
    Some([(a[0] * b[0]).sqrt(), (a[1] * b[1]).sqrt(), (a[2] * b[2]).sqrt()])
}

/// The Temperature and Tint (each -1 to 1, as the grade keeps them in
/// whole numbers over a million) that take light of colour `light` back
/// to neutral: the Basic tool's sliders make red and blue `2^(t/2)` and
/// `2^(-t/2)`, green `2^(-tint/2)`.
pub fn sliders(light: Light) -> (f32, f32) {
    let [r, g, b] = light.map(|v| v.max(1e-6));
    let temperature = (b / r).log2();
    let tint = -2.0 * ((r * b).sqrt() / g).log2();
    (temperature.clamp(-1.0, 1.0), tint.clamp(-1.0, 1.0))
}

/// The light of a clip's frames, if enough of them can be used: at least
/// a fiftieth of the pixels at every step.
pub fn estimate(frames: &[Vec<u8>]) -> Option<(f32, f32)> {
    let all = light_of(frames);
    let mut light: Light = [1.0; 3];
    for strong in STRONG_COLOUR {
        let pixels = usable(&all, light, strong);
        if pixels.len() * 50 < all.len().max(1) {
            return None;
        }
        let more = combined(&pixels)?;
        let mean = (more[0] + more[1] + more[2]) / 3.0;
        light = [0, 1, 2].map(|k| light[k] * more[k] / mean.max(1e-6));
    }
    Some(sliders(light))
}

/// What to look at: a stretch of a file's picture.
#[derive(Debug, Clone)]
pub struct Look {
    pub path: PathBuf,
    pub stream: u32,
    pub rotation: u16,
    pub from: Time,
    pub to: Time,
}

/// A white balance being worked out on a thread of its own: its answer,
/// once it comes (`None` when the clip has too little that is neutral).
#[derive(Debug)]
pub struct Balancing {
    answer: Receiver<diag::Result<Option<(f32, f32)>>>,
}

impl Balancing {
    pub fn start(look: Look) -> diag::Result<Balancing> {
        let (send, answer) = channel();
        std::thread::Builder::new()
            .name("white balance".into())
            .spawn(move || {
                let _ = send.send(frames_of(&look).map(|frames| estimate(&frames)));
            })
            .map_err(|err| {
                diag::AppError::new("Auto white balance couldn't start")
                    .why(format!("A thread for it couldn't be made: {err}."))
            })?;
        Ok(Balancing { answer })
    }

    /// The answer, once it has come.
    pub fn answer(&self) -> Option<diag::Result<Option<(f32, f32)>>> {
        self.answer.try_recv().ok()
    }
}

/// [`FRAMES`] frames spread across the stretch, small.
fn frames_of(look: &Look) -> diag::Result<Vec<Vec<u8>>> {
    let mut decoder = VideoDecoder::open(&look.path, look.stream, look.rotation, Decoding::Software)?;
    let (from, to) = (
        look.from.as_secs_f64(),
        look.to.as_secs_f64().max(look.from.as_secs_f64()),
    );
    (0..FRAMES)
        .map(|i| {
            let at = from + (to - from) * (i as f64 + 0.5) / FRAMES as f64;
            decoder.frame_at(at, 160, 160).map(|frame| frame.rgba)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room: greys, skin, a wood shelf, a blue poster, a lamp clipped
    /// white, shadows; lit by light of colour `cast` (light multiplied).
    fn room(cast: [f32; 3]) -> Vec<u8> {
        let surfaces: [[f32; 3]; 8] = [
            [0.18, 0.18, 0.18],
            [0.45, 0.45, 0.45],
            [0.62, 0.42, 0.33],
            [0.35, 0.22, 0.12],
            [0.10, 0.18, 0.55],
            [1.2, 1.2, 1.2],
            [0.004, 0.004, 0.004],
            [0.30, 0.31, 0.29],
        ];
        let mut rgba = Vec::new();
        for i in 0..2000 {
            let s = surfaces[i % surfaces.len()];
            for k in 0..3 {
                let light = (s[k] * cast[k]).clamp(0.0, 1.0);
                rgba.push((light.powf(1.0 / 2.4) * 255.0).round() as u8);
            }
            rgba.push(255);
        }
        rgba
    }

    /// A game's own look: strong purples and teals, almost nothing neutral.
    fn neon() -> Vec<u8> {
        (0..2000)
            .flat_map(|i| match i % 3 {
                0 => [200, 30, 220, 255],
                1 => [20, 200, 190, 255],
                _ => [240, 60, 120, 255],
            })
            .collect()
    }

    #[test]
    fn a_warm_room_comes_back_to_neutral() {
        let warm = [1.25, 1.0, 0.7];
        let (t, tint) = estimate(&[room(warm)]).expect("enough neutral");
        // The right answer: blue over red of the light.
        let right = (0.7f32 / 1.25).log2();
        assert!((t - right).abs() < 0.12, "temperature {t}, right {right}");
        // Green over red and blue together.
        let right_tint = -2.0 * ((1.25f32 * 0.7).sqrt() / 1.0).log2();
        assert!((tint - right_tint).abs() < 0.1, "tint {tint}, right {right_tint}");
        let (t, tint) = estimate(&[room([1.0, 1.0, 1.0])]).expect("enough neutral");
        assert!(
            t.abs() < 0.05 && tint.abs() < 0.05,
            "a neutral room is left: {t} {tint}"
        );
    }

    #[test]
    fn a_game_with_colours_of_its_own_is_left_alone() {
        assert_eq!(estimate(&[neon()]), None);
    }

    #[test]
    fn combined_is_steadier_than_either_alone_on_a_room_with_a_coloured_wall() {
        // Half the room a pale green wall: grey world leans green, the
        // white patch doesn't; together they lean less.
        let warm = [1.2, 1.0, 0.8];
        let mut frame = room(warm);
        for chunk in frame.chunks_exact_mut(8) {
            chunk[..4].copy_from_slice(&[150, 185, 150, 255]);
        }
        let pixels = usable(&light_of(&[frame]), [1.0; 3], 0.5);
        let tint_of = |l: Option<Light>| sliders(l.expect("an estimate")).1.abs();
        let (grey, patch, both) = (
            tint_of(grey_world(&pixels)),
            tint_of(white_patch(&pixels)),
            tint_of(combined(&pixels)),
        );
        assert!(both <= grey + 1e-6, "combined {both} against grey world {grey}");
        assert!(patch <= grey + 1e-6, "white patch {patch}");
    }
}
