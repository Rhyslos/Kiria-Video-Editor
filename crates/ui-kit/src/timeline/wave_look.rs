//! How a clip's waveform is drawn on the timeline.
//!
//! Two layers per pixel column: the loudest peak under it, lighter, and
//! the clip's usual level there (the average of the peaks under it), bright,
//! so speech and hits read as shapes rather than a flat block. Heights
//! follow the clip's and its track's volume. Full scale (0 dBFS) sits at
//! [`FULL_SCALE`] of the way from the middle to the edge, marked by a faint
//! line each side; louder than that, the column goes on into the room
//! above it in red (up to +6 dB over, the edge), so where a volume change
//! makes the sound clip is plain to see while it is dragged.

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};

/// Where full scale sits, as a share of the way from the middle to the edge.
pub const FULL_SCALE: f32 = 0.8;
/// How far over full scale reaches the edge: twice as loud, +6 dB.
const OVER_TO_EDGE: f32 = 2.0;
/// Below full scale, heights are the level to this power: a little more
/// room for quiet sound than straight amplitude, with peaks still standing out.
const CURVE: f32 = 0.7;
/// Peaks averaged for a column's usual level at most: far zoomed out, an
/// even sample of them.
const BODY_SAMPLES: usize = 16;
/// Clipping's colour.
pub const OVER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);

/// A level (1 is full scale) as a share of the way from the middle to the
/// edge, and whether it is over full scale.
pub fn height(level: f32) -> (f32, bool) {
    let level = level.max(0.0);
    if level <= 1.0 {
        return (level.powf(CURVE) * FULL_SCALE, false);
    }
    let over = ((level - 1.0) / (OVER_TO_EDGE - 1.0)).min(1.0);
    (FULL_SCALE + over * (1.0 - FULL_SCALE), true)
}

/// The usual level of `peaks[from..to]`: their average, from at most
/// [`BODY_SAMPLES`] of them spread evenly.
pub fn body(peaks: &[f32], from: usize, to: usize) -> f32 {
    let to = to.min(peaks.len());
    if from >= to {
        return 0.0;
    }
    let count = to - from;
    let taken = count.min(BODY_SAMPLES);
    let sum: f32 = (0..taken).map(|i| peaks[from + i * count / taken]).sum();
    sum / taken as f32
}

/// One pixel column: its loudest peak and its usual level, both already
/// times the volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub x: f32,
    pub peak: f32,
    pub body: f32,
}

/// Draws `columns` in `area` around its middle line: the faint full-scale
/// lines, the peaks, the usual level and, where they go over full scale,
/// the part past it in red. `colour` is the bright layer's.
pub fn draw(p: &egui::Painter, area: Rect, columns: &[Column], colour: Color32) {
    let (mid, half) = (area.center().y, area.height() / 2.0);
    if half < 2.0 {
        return;
    }
    let full = FULL_SCALE * half;
    if half >= 10.0 {
        let line = Stroke::new(1.0, Color32::from_white_alpha(28));
        for y in [mid - full, mid + full] {
            p.line_segment([pos2(area.left(), y), pos2(area.right(), y)], line);
        }
    }
    let peak_colour = colour.gamma_multiply(0.45);
    let mut mesh = egui::Mesh::default();
    let bar = |mesh: &mut egui::Mesh, x: f32, h: f32, c: Color32| {
        let h = h.max(0.5);
        mesh.add_colored_rect(Rect::from_min_max(pos2(x, mid - h), pos2(x + 1.0, mid + h)), c);
    };
    for c in columns {
        let (peak, peak_over) = height(c.peak);
        let (body, _) = height(c.body.min(c.peak));
        bar(&mut mesh, c.x, peak.min(FULL_SCALE) * half, peak_colour);
        bar(&mut mesh, c.x, body.min(FULL_SCALE) * half, colour);
        if peak_over {
            // Past full scale, both sides, in red.
            for (from, to) in [(mid - peak * half, mid - full), (mid + full, mid + peak * half)] {
                mesh.add_colored_rect(Rect::from_min_max(pos2(c.x, from), pos2(c.x + 1.0, to)), OVER);
            }
        }
    }
    p.add(Shape::mesh(mesh));
    // A clipped column's tip: a bright cap at the edge, so it reads at a glance.
    for c in columns.iter().filter(|c| c.peak >= OVER_TO_EDGE) {
        for y in [area.top(), area.bottom() - 1.0] {
            p.line_segment(
                [Pos2::new(c.x, y), Pos2::new(c.x + 1.0, y)],
                Stroke::new(2.0, OVER),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_sits_inside_the_clip_and_louder_goes_on_in_red() {
        assert_eq!(height(0.0), (0.0, false));
        assert_eq!(height(1.0), (FULL_SCALE, false));
        // Half as loud is more than half as tall: quiet sound keeps room.
        let (half, _) = height(0.5);
        assert!(half > 0.5 * FULL_SCALE && half < FULL_SCALE, "{half}");
        // +3 dB over: half way into the room above, and over.
        let (over, clipped) = height(std::f32::consts::SQRT_2);
        assert!(clipped && over > FULL_SCALE && over < 1.0, "{over}");
        // +6 dB and louder: the edge, never past it.
        assert_eq!(height(2.0), (1.0, true));
        assert_eq!(height(9.0), (1.0, true));
        // Turned down, it shrinks with the volume: not a flat block.
        assert!(height(0.25).0 < height(0.5).0);
    }

    #[test]
    fn the_usual_level_is_the_average_of_an_even_sample() {
        let peaks = [0.0, 1.0, 0.0, 1.0];
        assert_eq!(body(&peaks, 0, 4), 0.5);
        assert_eq!(body(&peaks, 1, 2), 1.0);
        assert_eq!(body(&peaks, 3, 2), 0.0, "nothing between");
        assert_eq!(body(&peaks, 0, 99), 0.5, "held to the peaks there are");
        // Far zoomed out: at most a few looked at, still the average.
        let many: Vec<f32> = (0..10_000).map(|i| if i % 2 == 0 { 0.2 } else { 0.6 }).collect();
        assert!((body(&many, 0, many.len()) - 0.4).abs() < 0.05);
    }
}
