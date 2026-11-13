//! A whole file's waveform in a rectangle: the Library's audio rows and
//! the large waveform in its detail panel. One bar per pixel column, as
//! loud as the loudest peak under it, the same curve the timeline uses (the
//! square root of the peak, so quiet sound still shows).
//!
//! Also `Levels`, which every waveform drawn (here and on the timeline)
//! takes its columns' loudest peaks from: a column over many peaks costs a
//! few steps, not one per peak, so zoomed out over hours of sound a frame
//! costs what its width does, not what the recording's length does.

use egui::{Color32, Context, Id, Painter, Rect, Shape, pos2};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// The loudest of each two peaks, of each two of those, and so on: worked
/// out once per set of peaks, so the loudest between any two is found in a
/// few steps.
#[derive(Debug)]
pub struct Levels {
    /// `above[0][i]` is the loudest of peaks `2i` and `2i + 1`, `above[1][i]`
    /// of `above[0][2i]` and `above[0][2i + 1]`, and so on.
    above: Vec<Vec<f32>>,
}

impl Levels {
    pub fn of(peaks: &[f32]) -> Levels {
        let mut above: Vec<Vec<f32>> = Vec::new();
        loop {
            let below: &[f32] = above.last().map_or(peaks, Vec::as_slice);
            if below.len() <= 1 {
                break;
            }
            let next = below
                .chunks(2)
                .map(|pair| pair.iter().fold(0f32, |m, v| m.max(*v)))
                .collect();
            above.push(next);
        }
        Levels { above }
    }

    /// The loudest of `peaks[from..to]` (0 for none), `peaks` being what
    /// these levels were made of.
    pub fn loudest(&self, peaks: &[f32], from: usize, to: usize) -> f32 {
        let (mut lo, mut hi) = (from.min(peaks.len()), to.min(peaks.len()));
        let mut m = 0f32;
        let mut level: &[f32] = peaks;
        let mut up = 0;
        // Bottom up: the ends that don't make a whole pair are taken at
        // this level, the rest from the level above.
        while lo < hi {
            if !lo.is_multiple_of(2) {
                m = m.max(level[lo]);
                lo += 1;
            }
            if !hi.is_multiple_of(2) {
                hi -= 1;
                m = m.max(level[hi]);
            }
            lo /= 2;
            hi /= 2;
            let Some(next) = self.above.get(up) else {
                break;
            };
            level = next;
            up += 1;
        }
        m
    }
}

/// Fewer peaks than this are folded as they are: quicker than the levels.
const FEW: usize = 4_096;

/// Levels for peaks, each kept with the peaks they were made of (so the
/// address they are known by can't be another's) and the pass they were
/// last used in. They are made on a thread of their own (a few
/// milliseconds for hours of sound), filled in once ready.
#[derive(Clone, Default)]
struct Kept(HashMap<usize, Entry>);

/// The peaks, their levels once made, the pass last drawn in.
type Entry = (Arc<[f32]>, Arc<OnceLock<Arc<Levels>>>, u64);

/// Passes a set of levels is kept unused before it is let go.
const KEPT_FOR: u64 = 600;

/// The levels of `peaks`, made in the background the first time they are
/// drawn and kept while they are; `None` until they are ready (the peaks
/// are then folded as they are) and for a few peaks.
pub fn levels_for(ctx: &Context, peaks: &Arc<[f32]>) -> Option<Arc<Levels>> {
    if peaks.len() < FEW {
        return None;
    }
    let pass = ctx.cumulative_pass_nr();
    let key = Arc::as_ptr(peaks).cast::<f32>() as usize;
    let ready = ctx.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<Kept>(Id::unique("waveform levels"));
        if pass.is_multiple_of(60) {
            kept.0
                .retain(|_, (_, _, used)| used.saturating_add(KEPT_FOR) >= pass);
        }
        let entry = kept.0.entry(key).or_insert_with(|| {
            let cell = Arc::new(OnceLock::new());
            let (made, of, ctx) = (Arc::clone(&cell), Arc::clone(peaks), ctx.clone());
            let spawned = std::thread::Builder::new()
                .name("waveform levels".into())
                .spawn(move || {
                    let _ = made.set(Arc::new(Levels::of(&of)));
                    ctx.request_repaint();
                });
            if let Err(err) = spawned {
                tracing::debug!("waveform levels not made: {err}");
            }
            (Arc::clone(peaks), cell, pass)
        });
        entry.2 = pass;
        Arc::clone(&entry.1)
    });
    ready.get().cloned()
}

/// The loudest of `peaks[from..to]` (0 for none), through `levels` when
/// there are any.
pub fn loudest(peaks: &[f32], levels: Option<&Levels>, from: usize, to: usize) -> f32 {
    match levels {
        Some(levels) => levels.loudest(peaks, from, to),
        None => peaks[from.min(peaks.len())..to.min(peaks.len())]
            .iter()
            .fold(0f32, |m, v| m.max(*v)),
    }
}

/// Draws `peaks` (`per_second` of them per second, 0 to 1) of a file
/// `length` seconds long across `rect`, centred on its middle line. Peaks
/// still being read leave the rest of the rectangle empty.
pub fn paint(
    painter: &Painter,
    rect: Rect,
    peaks: &Arc<[f32]>,
    per_second: f32,
    length: f64,
    colour: Color32,
) {
    if rect.width() < 1.0 || rect.height() < 2.0 || peaks.is_empty() || length <= 0.0 || per_second <= 0.0 {
        return;
    }
    let levels = levels_for(painter.ctx(), peaks);
    let mut mesh = egui::Mesh::default();
    let (mid, half) = (rect.center().y, rect.height() / 2.0);
    let columns = rect.width().floor().max(1.0) as usize;
    for column in 0..columns {
        let Some(peak) = column_peak(peaks, levels.as_deref(), (per_second, length), column, columns) else {
            break;
        };
        let x = rect.left() + column as f32;
        let h = (peak.sqrt() * half).max(0.5);
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x, mid - h), pos2(x + 1.0, mid + h)),
            colour,
        );
    }
    painter.add(Shape::mesh(mesh));
}

/// The loudest peak under column `column` of `columns` spread over the
/// file, or `None` past the peaks read so far.
pub fn column_peak(
    peaks: &[f32],
    levels: Option<&Levels>,
    (per_second, length): (f32, f64),
    column: usize,
    columns: usize,
) -> Option<f32> {
    let columns = columns.max(1) as f64;
    let per_second = f64::from(per_second);
    let from = (column as f64 / columns * length * per_second).floor() as usize;
    let to = (((column + 1) as f64 / columns * length * per_second).ceil() as usize).max(from + 1);
    if from >= peaks.len() {
        return None;
    }
    let to = to.min(peaks.len());
    Some(loudest(peaks, levels, from, to).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_column_takes_the_loudest_peak_under_it() {
        // Ten seconds at ten peaks a second, quiet but for one loud peak.
        let mut peaks = vec![0.1f32; 100];
        peaks[57] = 0.9;
        // Ten columns: a second each.
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 5, 10), Some(0.9));
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 4, 10), Some(0.1));
        // Many more columns than peaks: each still finds one.
        assert!(column_peak(&peaks, None, (10.0, 10.0), 999, 1000).is_some());
    }

    #[test]
    fn peaks_still_being_read_stop_early() {
        let peaks = vec![0.5f32; 30];
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 2, 10), Some(0.5));
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 3, 10), None);
    }

    #[test]
    fn long_peaks_get_levels_made_in_the_background_and_kept() {
        let ctx = Context::default();
        let short: Arc<[f32]> = vec![0.5f32; 100].into();
        assert!(levels_for(&ctx, &short).is_none(), "a few are folded as they are");
        let long: Arc<[f32]> = (0..100_000u32).map(|i| (i % 97) as f32 / 97.0).collect();
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let levels = loop {
            if let Some(levels) = levels_for(&ctx, &long) {
                break levels;
            }
            assert!(std::time::Instant::now() < give_up, "made in time");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(levels.loudest(&long, 0, 100_000), 96.0 / 97.0);
        let again = levels_for(&ctx, &long).unwrap();
        assert!(Arc::ptr_eq(&levels, &again), "kept, not made again");
    }

    #[test]
    fn the_levels_find_the_loudest_between_any_two_peaks() {
        // Not a power of two long, with loud peaks here and there.
        let peaks: Vec<f32> = (0..1_000u32)
            .map(|i| ((i * 7_919) % 1_009) as f32 / 1_009.0)
            .collect();
        let levels = Levels::of(&peaks);
        for from in (0..1_000).step_by(37) {
            for to in [
                from,
                from + 1,
                from + 2,
                from + 63,
                from + 64,
                from + 65,
                from + 500,
                1_000,
                2_000,
            ] {
                let want = peaks[from.min(1_000)..to.clamp(from, 1_000)]
                    .iter()
                    .fold(0f32, |m, v| m.max(*v));
                assert_eq!(levels.loudest(&peaks, from, to), want, "{from}..{to}");
            }
        }
        assert_eq!(Levels::of(&[]).loudest(&[], 0, 5), 0.0);
        assert_eq!(
            loudest(&peaks, Some(&levels), 10, 900),
            loudest(&peaks, None, 10, 900)
        );
        assert_eq!(Levels::of(&[0.3]).loudest(&[0.3], 0, 1), 0.3);
    }
}
