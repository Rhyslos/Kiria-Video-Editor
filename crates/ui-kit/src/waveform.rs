//! A whole file's waveform in a rectangle: the Library's audio rows and
//! the large waveform in its detail panel. One bar per pixel column, as
//! loud as the loudest peak under it, the same curve the timeline uses (the
//! square root of the peak, so quiet sound still shows).

use egui::{Color32, Painter, Rect, Shape, pos2};

/// Draws `peaks` (`per_second` of them per second, 0 to 1) of a file
/// `length` seconds long across `rect`, centred on its middle line. Peaks
/// still being read leave the rest of the rectangle empty.
pub fn paint(painter: &Painter, rect: Rect, peaks: &[f32], per_second: f32, length: f64, colour: Color32) {
    if rect.width() < 1.0 || rect.height() < 2.0 || peaks.is_empty() || length <= 0.0 || per_second <= 0.0 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    let (mid, half) = (rect.center().y, rect.height() / 2.0);
    let columns = rect.width().floor().max(1.0) as usize;
    for column in 0..columns {
        let Some(peak) = column_peak(peaks, per_second, length, column, columns) else {
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
    per_second: f32,
    length: f64,
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
    Some(peaks[from..to].iter().fold(0f32, |m, v| m.max(*v)).min(1.0))
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
        assert_eq!(column_peak(&peaks, 10.0, 10.0, 5, 10), Some(0.9));
        assert_eq!(column_peak(&peaks, 10.0, 10.0, 4, 10), Some(0.1));
        // Many more columns than peaks: each still finds one.
        assert!(column_peak(&peaks, 10.0, 10.0, 999, 1000).is_some());
    }

    #[test]
    fn peaks_still_being_read_stop_early() {
        let peaks = vec![0.5f32; 30];
        assert_eq!(column_peak(&peaks, 10.0, 10.0, 2, 10), Some(0.5));
        assert_eq!(column_peak(&peaks, 10.0, 10.0, 3, 10), None);
    }
}
