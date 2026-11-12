//! Features of the video, one value per decoded frame: how different the
//! colours are from the frame before (histogram distance, for cuts) and how
//! much the picture changed pixel by pixel (difference, for cuts and
//! motion). Worked out on the tiny pictures from `media`.

use crate::media::Thumb;

const Y_BINS: usize = 32;
const C_BINS: usize = 16;

/// One decoded frame, compared with the one decoded before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub time: f64,
    /// Mean absolute brightness difference, 0 to 255.
    pub difference: f32,
    /// Half the summed absolute difference of the colour histograms, 0
    /// (same colours) to 1 (no colour in common).
    pub histogram: f32,
}

/// Turns tiny pictures into [`Change`]s as they arrive.
#[derive(Default)]
pub struct Tracker {
    previous: Option<(Vec<u8>, Vec<f32>)>,
    pub changes: Vec<Change>,
}

fn histogram(t: &Thumb) -> Vec<f32> {
    let mut h = vec![0.0f32; Y_BINS + 2 * C_BINS];
    let wy = 1.0 / t.y.len() as f32;
    let wc = 1.0 / t.u.len() as f32;
    for &y in &t.y {
        h[y as usize * Y_BINS / 256] += wy;
    }
    for &u in &t.u {
        h[Y_BINS + u as usize * C_BINS / 256] += wc;
    }
    for &v in &t.v {
        h[Y_BINS + C_BINS + v as usize * C_BINS / 256] += wc;
    }
    // Three histograms, each summing to 1: scale so the distance is 0..1.
    h.iter_mut().for_each(|x| *x /= 3.0);
    h
}

impl Tracker {
    pub fn push(&mut self, thumb: &Thumb) {
        let hist = histogram(thumb);
        if let Some((y, h)) = &self.previous {
            let difference = y
                .iter()
                .zip(&thumb.y)
                .map(|(&a, &b)| (i32::from(a) - i32::from(b)).unsigned_abs())
                .sum::<u32>() as f32
                / y.len() as f32;
            let histogram = h.iter().zip(&hist).map(|(a, b)| (a - b).abs()).sum::<f32>() / 2.0;
            self.changes.push(Change {
                time: thumb.time,
                difference,
                histogram,
            });
        }
        self.previous = Some((thumb.y.clone(), hist));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{THUMB_H, THUMB_W};

    fn flat(time: f64, y: u8, u: u8) -> Thumb {
        Thumb {
            time,
            y: vec![y; THUMB_W * THUMB_H],
            u: vec![u; THUMB_W * THUMB_H / 4],
            v: vec![128; THUMB_W * THUMB_H / 4],
        }
    }

    #[test]
    fn same_picture_changes_nothing_and_new_colours_change_everything() {
        let mut t = Tracker::default();
        t.push(&flat(0.0, 50, 100));
        t.push(&flat(0.1, 50, 100));
        t.push(&flat(0.2, 200, 30));
        assert_eq!(t.changes.len(), 2);
        assert_eq!(t.changes[0].difference, 0.0);
        assert_eq!(t.changes[0].histogram, 0.0);
        assert_eq!(t.changes[1].difference, 150.0);
        // Brightness and one colour channel moved; the other stayed.
        assert!((t.changes[1].histogram - 2.0 / 3.0).abs() < 1e-5);
    }
}
