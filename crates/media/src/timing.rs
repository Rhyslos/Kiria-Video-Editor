//! Frame timing: is a recording's frame rate constant, and which constant
//! rate is it closest to?
//!
//! Game recorders often write variable frame rate: when the game stutters, a
//! frame is held longer or skipped. The editor works on a constant rate, so
//! such a file is shown at the nearest standard rate, with every frame placed
//! by its timestamp. Sound is placed by its own timestamps too, so the two
//! can't drift apart however long the file is.

use project::Rate;

/// What the packet times showed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// The constant rate the file is shown at.
    pub rate: Rate,
    pub variable: bool,
}

/// Standard rates a measured one snaps to when within 1 %.
const STANDARD: [Rate; 15] = [
    Rate::NTSC_24,
    Rate::FPS_24,
    Rate::FPS_25,
    Rate::NTSC_30,
    Rate::FPS_30,
    Rate::whole(48),
    Rate::FPS_50,
    Rate::NTSC_60,
    Rate::FPS_60,
    Rate::whole(90),
    Rate::whole(100),
    Rate::FPS_120,
    Rate::FPS_144,
    Rate::whole(165),
    Rate::whole(240),
];

/// A frame duration counts as uneven when it is this far from the usual one.
const UNEVEN: f64 = 0.10;
/// More than this share of uneven frame durations makes a file variable.
const UNEVEN_SHARE: f64 = 0.01;

/// Works out the timing from windows of packet times (seconds, in file order
/// within each window). `None` when there are too few frames to tell.
pub fn analyse(windows: &[Vec<f64>]) -> Option<Timing> {
    let windows: Vec<Vec<f64>> = windows
        .iter()
        .map(|w| {
            let mut times = w.clone();
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            times
        })
        .collect();
    let gaps: Vec<f64> = windows
        .iter()
        .flat_map(|t| t.windows(2).map(|w| w[1] - w[0]))
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.len() < 8 {
        return None;
    }
    let mut sorted = gaps.clone();
    sorted.sort_by(f64::total_cmp);
    let usual = sorted[sorted.len() / 2];
    let uneven = gaps
        .iter()
        .filter(|g| ((**g - usual) / usual).abs() > UNEVEN)
        .count();
    let variable = uneven as f64 / gaps.len() as f64 > UNEVEN_SHARE;
    // Rate = frame slots per second over each whole window: a skipped frame
    // still counts as a slot, and timestamps rounded to whole milliseconds
    // only matter at the window's two ends.
    let (mut slots, mut span) = (0.0, 0.0);
    for times in &windows {
        if let (Some(first), Some(last)) = (times.first(), times.last()) {
            slots += times
                .windows(2)
                .map(|w| ((w[1] - w[0]) / usual).round().max(1.0))
                .sum::<f64>();
            span += last - first;
        }
    }
    let fps = if span > 0.0 { slots / span } else { 1.0 / usual };
    Some(Timing {
        rate: nearest(fps),
        variable,
    })
}

/// The standard rate within 1 % of `fps`, else `fps` rounded to a whole number.
pub fn nearest(fps: f64) -> Rate {
    STANDARD
        .iter()
        .copied()
        .min_by(|a, b| (a.as_f64() - fps).abs().total_cmp(&(b.as_f64() - fps).abs()))
        .filter(|r| (r.as_f64() - fps).abs() / fps <= 0.01)
        .unwrap_or_else(|| Rate::whole(fps.round().clamp(1.0, 1000.0) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn even(fps: f64, frames: usize) -> Vec<f64> {
        (0..frames).map(|i| i as f64 / fps).collect()
    }

    #[test]
    fn a_steady_recording_is_constant() {
        let t = analyse(&[even(60.0, 300), even(60.0, 300)]).unwrap();
        assert_eq!(
            t,
            Timing {
                rate: Rate::FPS_60,
                variable: false
            }
        );
        let t = analyse(&[even(60_000.0 / 1001.0, 300)]).unwrap();
        assert_eq!(t.rate, Rate::NTSC_60);
    }

    #[test]
    fn dropped_and_held_frames_make_it_variable() {
        let mut times = even(60.0, 600);
        // Every 20th frame skipped: the gap doubles there.
        times.retain(|t| ((t * 60.0).round() as i64) % 20 != 7);
        let t = analyse(&[times]).unwrap();
        assert!(t.variable);
        assert_eq!(t.rate, Rate::FPS_60, "the usual gap still says 60");
    }

    #[test]
    fn millisecond_timestamps_still_give_the_right_rate() {
        let times: Vec<f64> = (0..300)
            .map(|i| (i as f64 * 1000.0 / 60.0).round() / 1000.0)
            .collect();
        assert_eq!(
            analyse(&[times]).unwrap(),
            Timing {
                rate: Rate::FPS_60,
                variable: false
            }
        );
    }

    #[test]
    fn b_frame_order_and_odd_rates() {
        let mut times = even(144.0, 200);
        times.swap(3, 4); // packets arrive out of display order
        assert_eq!(
            analyse(&[times]).unwrap(),
            Timing {
                rate: Rate::FPS_144,
                variable: false
            }
        );
        assert_eq!(nearest(72.3), Rate::whole(72));
        assert!(analyse(&[vec![0.0, 0.1]]).is_none(), "too few frames");
    }
}
