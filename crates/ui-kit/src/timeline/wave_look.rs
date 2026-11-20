//! How a clip's sound is drawn on the timeline.
//!
//! Every look draws the same two layers per pixel column: the loudest peak
//! under it, faint, and its usual level (the average of the peaks under it,
//! close to how loud it sounds), bright, so explosions, speech and quiet
//! moments read as shapes. They use the clip's whole height.
//!
//! The scale: fitted to the clip ([`WaveScale::fit`], the usual), the
//! file's own range from its quiet floor to its loudest peak fills the
//! height on a decibel scale, so even heavily compressed game sound shows
//! its ups and downs; straight ([`WaveScale::straight`]), full scale fills
//! it. Either way heights follow the clip's and its track's volume, and a
//! column louder than full scale is drawn in red with a bright cap, so a
//! volume that makes the sound clip is plain to see while it is dragged.
//! The scale only changes the drawing, never the sound.

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};
use serde::{Deserialize, Serialize};

/// Clipping's colour.
pub const OVER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
/// With the straight scale, heights are the level to this power: a little
/// more room for quiet sound than straight amplitude.
const CURVE: f32 = 0.7;
/// Peaks averaged for a column's usual level at most: far zoomed out, an
/// even sample of them.
const BODY_SAMPLES: usize = 16;
/// A fitted scale never spans less than this many decibels, so a nearly
/// steady sound isn't blown up into wild shapes.
const LEAST_RANGE_DB: f32 = 18.0;
/// Nor reaches lower than this.
const FLOOR_DB: f32 = -60.0;
/// Where a fitted scale puts a file's usual level, as a share of the height.
const USUAL_AT: f32 = 0.6;
/// Peaks sampled to find a file's range: enough for its usual level.
const RANGE_SAMPLES: usize = 4096;
/// Pixel columns per bar in [`WaveStyle::Bars`].
const BAR_EVERY: usize = 4;

/// How sound is drawn in clips (Settings › Editing › Waveform look).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveStyle {
    /// Mirrored around the middle in the track's colour, on a dark clip
    /// with a coloured edge (the usual).
    #[default]
    Centred,
    /// Standing on the clip's bottom edge, one side only: twice the room.
    BottomUp,
    /// Separate rounded bars a few pixels apart.
    Bars,
    /// A thin outline over a faint fill, on a plain dark clip with a
    /// coloured bar at its left edge.
    Outline,
}

impl WaveStyle {
    pub const ALL: [WaveStyle; 4] = [
        WaveStyle::Centred,
        WaveStyle::BottomUp,
        WaveStyle::Bars,
        WaveStyle::Outline,
    ];

    /// Its name in Settings.
    pub fn words(self) -> &'static str {
        match self {
            WaveStyle::Centred => "Centred",
            WaveStyle::BottomUp => "From the bottom up",
            WaveStyle::Bars => "Rounded bars",
            WaveStyle::Outline => "Outline",
        }
    }

    /// A line about it in Settings.
    pub fn explained(self) -> &'static str {
        match self {
            WaveStyle::Centred => {
                "Mirrored around the middle in the track's colour, on a dark clip with a coloured edge."
            }
            WaveStyle::BottomUp => {
                "Standing on the clip's bottom edge: twice the room, so quiet sound and short tracks still show it."
            }
            WaveStyle::Bars => "Separate rounded bars: calm when zoomed out, finer as you zoom in.",
            WaveStyle::Outline => {
                "A thin outline on a plain dark clip with a coloured bar at its edge: the clips step back, the lines stand out."
            }
        }
    }
}

/// How tall levels are drawn: fitted to the file's own range (decibels from
/// `floor_db` to `top_db` fill the height), or straight (full scale fills it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveScale {
    Fitted { floor_db: f32, top_db: f32 },
    Straight,
}

impl WaveScale {
    /// The scale fitted to `peaks` (a whole file's): its loudest peak at
    /// the top and its usual level (the median of a sample of them) at
    /// [`USUAL_AT`] of the height, on a decibel scale, never spanning less
    /// than [`LEAST_RANGE_DB`] nor reaching below [`FLOOR_DB`]. So loud,
    /// steady sound sits a little over half way and its hits and dips show
    /// above and below. Silence gives the straight scale.
    pub fn fit(peaks: &[f32]) -> WaveScale {
        let step = (peaks.len() / RANGE_SAMPLES).max(1);
        let mut sample: Vec<f32> = peaks.iter().step_by(step).copied().filter(|v| *v > 0.0).collect();
        let top = peaks.iter().fold(0f32, |m, v| m.max(*v));
        if sample.is_empty() || top <= 0.0 {
            return WaveScale::Straight;
        }
        sample.sort_by(f32::total_cmp);
        let usual_db = db(sample[sample.len() / 2]);
        let top_db = db(top).min(0.0);
        let floor_db = (usual_db - (top_db - usual_db) * USUAL_AT / (1.0 - USUAL_AT))
            .max(FLOOR_DB)
            .min(top_db - LEAST_RANGE_DB);
        WaveScale::Fitted { floor_db, top_db }
    }

    /// The straight scale.
    pub fn straight() -> WaveScale {
        WaveScale::Straight
    }

    /// A level (1 is full scale, already times the volume) as a share of
    /// the room (0 to 1), and whether it is over full scale.
    pub fn height(self, level: f32) -> (f32, bool) {
        let level = level.max(0.0);
        let over = level > 1.0;
        let share = match self {
            WaveScale::Straight => level.min(1.0).powf(CURVE),
            WaveScale::Fitted { floor_db, top_db } => {
                if level <= 0.0 {
                    0.0
                } else {
                    ((db(level) - floor_db) / (top_db - floor_db)).clamp(0.0, 1.0)
                }
            }
        };
        (share, over)
    }
}

fn db(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
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

/// The colours of a clip's sound in a look: the faint peaks, the bright
/// usual level, the outline.
#[derive(Debug, Clone, Copy)]
pub struct WaveColours {
    pub peak: Color32,
    pub body: Color32,
}

impl WaveColours {
    /// From the track's colour, dimmed for a muted track.
    pub fn of(colour: Color32, muted: bool) -> WaveColours {
        let (peak, body) = (lighter(colour, 0.1).gamma_multiply(0.75), lighter(colour, 0.45));
        match muted {
            true => WaveColours {
                peak: peak.gamma_multiply(0.45),
                body: body.gamma_multiply(0.45),
            },
            false => WaveColours { peak, body },
        }
    }
}

/// `c` moved `by` (0 to 1) of the way to white.
fn lighter(c: Color32, by: f32) -> Color32 {
    let up = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * by).round() as u8;
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// A clip with sound's own body in `style`: a dark panel tinted with the
/// track's `colour` and a coloured edge, or (Outline) a plain dark one with
/// a coloured bar at its left edge.
pub fn draw_body(p: &egui::Painter, r: Rect, colour: Color32, style: WaveStyle) {
    match style {
        WaveStyle::Outline => {
            p.rect_filled(r, 4.0, Color32::from_rgb(0x24, 0x25, 0x27));
            p.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, Color32::from_rgb(0x36, 0x38, 0x3B)),
                egui::StrokeKind::Inside,
            );
            let bar = Rect::from_min_max(r.min, pos2(r.left() + 3.0, r.bottom()));
            p.rect_filled(
                bar,
                egui::CornerRadius {
                    nw: 4,
                    sw: 4,
                    ne: 0,
                    se: 0,
                },
                colour,
            );
        }
        _ => {
            let dark = Color32::from_rgb(
                (f32::from(colour.r()) * 0.26) as u8,
                (f32::from(colour.g()) * 0.26) as u8,
                (f32::from(colour.b()) * 0.26) as u8,
            );
            p.rect_filled(r, 4.0, dark);
            p.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, colour.gamma_multiply(0.8)),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Draws `columns` in `area` in `style` on `scale`, in `colours`, louder
/// than full scale in red.
pub fn draw(
    p: &egui::Painter,
    area: Rect,
    columns: &[Column],
    style: WaveStyle,
    scale: WaveScale,
    colours: WaveColours,
) {
    if area.height() < 3.0 || columns.is_empty() {
        return;
    }
    match style {
        WaveStyle::Centred => mirrored(p, area, columns, scale, colours),
        WaveStyle::BottomUp => bottom_up(p, area, columns, scale, colours),
        WaveStyle::Bars => bars(p, area, columns, scale, colours),
        WaveStyle::Outline => outline(p, area, columns, scale, colours),
    }
    // A clipped column's tip: a bright cap at the edge, so it reads at a glance.
    let caps: Vec<&Column> = columns.iter().filter(|c| c.peak > 1.0).collect();
    let edges: &[f32] = match style {
        WaveStyle::BottomUp => &[0.0],
        _ => &[0.0, 1.0],
    };
    for c in caps {
        for at in edges {
            let y = if *at == 0.0 {
                area.top()
            } else {
                area.bottom() - 1.0
            };
            p.line_segment(
                [Pos2::new(c.x, y), Pos2::new(c.x + 1.0, y)],
                Stroke::new(2.0, OVER),
            );
        }
    }
}

/// The heights of a column's peak and usual level as shares of the room,
/// and whether it is over full scale.
fn shares(scale: WaveScale, c: &Column) -> (f32, f32, bool) {
    let (peak, over) = scale.height(c.peak);
    let (body, _) = scale.height(c.body.min(c.peak));
    (peak, body, over)
}

fn mirrored(p: &egui::Painter, area: Rect, columns: &[Column], scale: WaveScale, colours: WaveColours) {
    let (mid, half) = (area.center().y, area.height() / 2.0);
    let mut mesh = egui::Mesh::default();
    for c in columns {
        let (peak, body, over) = shares(scale, c);
        let bar = |mesh: &mut egui::Mesh, h: f32, colour: Color32| {
            let h = h.max(0.5);
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(c.x, mid - h), pos2(c.x + 1.0, mid + h)),
                colour,
            );
        };
        bar(&mut mesh, peak * half, if over { OVER } else { colours.peak });
        bar(&mut mesh, body * half, colours.body);
    }
    p.add(Shape::mesh(mesh));
}

fn bottom_up(p: &egui::Painter, area: Rect, columns: &[Column], scale: WaveScale, colours: WaveColours) {
    let (bottom, room) = (area.bottom(), area.height());
    let mut mesh = egui::Mesh::default();
    for c in columns {
        let (peak, body, over) = shares(scale, c);
        let bar = |mesh: &mut egui::Mesh, h: f32, colour: Color32| {
            let h = h.max(0.5);
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(c.x, bottom - h), pos2(c.x + 1.0, bottom)),
                colour,
            );
        };
        bar(&mut mesh, peak * room, if over { OVER } else { colours.peak });
        bar(&mut mesh, body * room, colours.body);
    }
    p.add(Shape::mesh(mesh));
}

fn bars(p: &egui::Painter, area: Rect, columns: &[Column], scale: WaveScale, colours: WaveColours) {
    let (mid, half) = (area.center().y, area.height() / 2.0);
    for group in columns.chunks(BAR_EVERY) {
        let Some(first) = group.first() else { continue };
        // The loudest of the group, so a short hit still shows.
        let loudest = group.iter().fold(
            Column {
                x: first.x,
                peak: 0.0,
                body: 0.0,
            },
            |m, c| Column {
                x: m.x,
                peak: m.peak.max(c.peak),
                body: m.body.max(c.body),
            },
        );
        let (peak, body, over) = shares(scale, &loudest);
        let h = (peak * 0.6 + body * 0.4).max(0.04) * half;
        let colour = if over { OVER } else { colours.body };
        let bar = Rect::from_min_max(pos2(first.x + 0.5, mid - h), pos2(first.x + 2.7, mid + h));
        p.rect_filled(bar, 1.1, colour);
    }
}

fn outline(p: &egui::Painter, area: Rect, columns: &[Column], scale: WaveScale, colours: WaveColours) {
    let (mid, half) = (area.center().y, area.height() / 2.0);
    let mut mesh = egui::Mesh::default();
    let mut top = Vec::with_capacity(columns.len());
    let mut under = Vec::with_capacity(columns.len());
    // The outline follows the peaks evened over a few columns, so single
    // hits don't make it a saw.
    let evened: Vec<f32> = (0..columns.len())
        .map(|i| {
            let near = &columns[i.saturating_sub(2)..(i + 3).min(columns.len())];
            near.iter().map(|c| c.peak).sum::<f32>() / near.len() as f32
        })
        .collect();
    for (c, even) in columns.iter().zip(&evened) {
        let (_, body, over) = shares(scale, c);
        let (peak, _) = scale.height(*even);
        let h = peak.max(0.01) * half;
        let fill = if over {
            OVER.gamma_multiply(0.6)
        } else {
            colours.peak.gamma_multiply(0.6)
        };
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(c.x, mid - h), pos2(c.x + 1.0, mid + h)),
            fill,
        );
        let b = body * half;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(c.x, mid - b), pos2(c.x + 1.0, mid + b)),
            colours.peak.gamma_multiply(0.5),
        );
        top.push(pos2(c.x + 0.5, mid - h));
        under.push(pos2(c.x + 0.5, mid + h));
    }
    p.add(Shape::mesh(mesh));
    let stroke = Stroke::new(1.0, colours.body);
    p.add(Shape::line(top, stroke));
    p.add(Shape::line(under, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_straight_scale_fills_the_room_at_full_scale_and_marks_louder() {
        let s = WaveScale::straight();
        assert_eq!(s.height(0.0), (0.0, false));
        assert_eq!(s.height(1.0), (1.0, false));
        // Half as loud is more than half as tall: quiet sound keeps room.
        let (half, _) = s.height(0.5);
        assert!(half > 0.5 && half < 1.0, "{half}");
        assert_eq!(s.height(2.0), (1.0, true), "louder: the edge, in red");
        assert!(s.height(0.25).0 < s.height(0.5).0, "turned down, it shrinks");
    }

    #[test]
    fn a_fitted_scale_spreads_loud_steady_sound_over_the_height() {
        // Game sound: never quieter than about -18 dB, peaks near full scale.
        let peaks: Vec<f32> = (0..10_000)
            .map(|i| 0.12 + 0.85 * ((i % 97) as f32 / 96.0))
            .collect();
        let fitted = WaveScale::fit(&peaks);
        let straight = WaveScale::straight();
        let (a, b) = (0.3, 0.9);
        let gap_fitted = fitted.height(b).0 - fitted.height(a).0;
        let gap_straight = straight.height(b).0 - straight.height(a).0;
        assert!(
            gap_fitted > gap_straight,
            "the same two levels further apart: {gap_fitted} > {gap_straight}"
        );
        assert!(
            (fitted.height(peaks.iter().fold(0f32, |m, v| m.max(*v))).0 - 1.0).abs() < 1e-4,
            "the loudest fills it"
        );
        // Its usual level a little over half way: room above for hits.
        let usual = fitted.height(0.545).0;
        assert!(usual > 0.5 && usual < 0.8, "{usual}");
        // Speech with long pauses: the usual level at 60 % of the height.
        let speech: Vec<f32> = (0..10_000)
            .map(|i| {
                if i % 3 == 0 {
                    0.01
                } else {
                    0.1 + 0.4 * ((i % 7) as f32 / 6.0)
                }
            })
            .collect();
        let fitted = WaveScale::fit(&speech);
        let mut sorted = speech.clone();
        sorted.sort_by(f32::total_cmp);
        assert!((fitted.height(sorted[sorted.len() / 2]).0 - USUAL_AT).abs() < 0.02);
        // A nearly steady sound isn't blown up: at least 18 dB from floor to top.
        let steady = vec![0.5f32; 1000];
        match WaveScale::fit(&steady) {
            WaveScale::Fitted { floor_db, top_db } => assert!(top_db - floor_db >= LEAST_RANGE_DB),
            WaveScale::Straight => panic!("sound gives a fitted scale"),
        }
        assert_eq!(WaveScale::fit(&[0.0; 50]), WaveScale::Straight, "silence");
        // Over full scale stays marked on a fitted scale too.
        assert!(fitted.height(1.5).1);
    }

    #[test]
    fn the_usual_level_is_the_average_of_an_even_sample() {
        let peaks = [0.0, 1.0, 0.0, 1.0];
        assert_eq!(body(&peaks, 0, 4), 0.5);
        assert_eq!(body(&peaks, 1, 2), 1.0);
        assert_eq!(body(&peaks, 3, 2), 0.0, "nothing between");
        assert_eq!(body(&peaks, 0, 99), 0.5, "held to the peaks there are");
        let many: Vec<f32> = (0..10_000).map(|i| if i % 2 == 0 { 0.2 } else { 0.6 }).collect();
        assert!((body(&many, 0, many.len()) - 0.4).abs() < 0.05);
    }

    #[test]
    fn every_look_has_words_and_a_muted_track_is_dimmer() {
        for style in WaveStyle::ALL {
            assert!(!style.words().is_empty() && !style.explained().is_empty());
        }
        let c = Color32::from_rgb(0x3E, 0x8E, 0x7E);
        let (on, off) = (WaveColours::of(c, false), WaveColours::of(c, true));
        assert!(off.body.r() < on.body.r() && off.body.a() < on.body.a());
    }
}
