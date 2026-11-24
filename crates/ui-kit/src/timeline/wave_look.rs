//! How a clip's sound is drawn on the timeline.
//!
//! Three settings, each independent of the others:
//!
//! - the shape ([`WaveStyle`]): a plain wave (a stroke per pixel column from
//!   the lowest to the highest sample under it, so the shape is uneven
//!   above and below, as the sound is), smooth peaks, rounded bars, or
//!   flat-topped blocks;
//! - the colours ([`WaveColouring`]): the track's colour on a charcoal
//!   clip, a quieter track colour on a grey clip, or a loudness map (the
//!   colour says how loud, from cool teal to amber, the track's colour an
//!   edge of the clip);
//! - the side ([`WaveSide`]): centred on a middle line, or the lower half
//!   only, standing on the clip's floor with twice the room.
//!
//! Every shape is drawn crisp: whole pixels, one solid colour, no soft edge
//! and no second shade around it.
//!
//! The scale is in plain proportion to the sound: fitted
//! ([`WaveScale::fit`], the usual), the file's loudest peak reaches the
//! clip's edge; straight, full scale does. Either way heights follow the
//! clip's and its track's volume, and a column louder than full scale is
//! drawn in red with a bright cap at the edge, so a volume that makes the
//! sound clip is plain to see while it is dragged. The scale only changes
//! the drawing, never the sound.

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};
use serde::{Deserialize, Serialize};

/// Clipping's colour.
pub const OVER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
/// Peaks averaged for a column's usual level at most: far zoomed out, an
/// even sample of them.
const BODY_SAMPLES: usize = 16;
/// Pixel columns per bar in [`WaveStyle::RoundBars`], and how wide a bar is.
const BAR_EVERY: usize = 5;
const BAR_WIDTH: f32 = 3.0;
/// Pixel columns per block in [`WaveStyle::Blocks`], and how wide a block is.
const BLOCK_EVERY: usize = 5;
const BLOCK_WIDTH: f32 = 4.0;
/// Columns either side a smooth peak reaches over.
const SMOOTH_REACH: usize = 6;
/// The highest a stroke reaches, as a share of the room: never quite the edge.
const ROOM: f32 = 0.96;

/// The shape sound is drawn in (Settings › Editing › Waveform shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveStyle {
    /// A stroke per pixel column from the lowest to the highest sample
    /// under it (the usual). Earlier looks read as it.
    #[default]
    #[serde(alias = "Centred", alias = "BottomUp", alias = "Outline")]
    Plain,
    /// Smooth peaks: every peak keeps its height and falls away on a curve
    /// either side, so hits are rounded spikes rather than a saw.
    Smooth,
    /// Separate bars with round ends, a few pixels apart.
    #[serde(alias = "Bars")]
    RoundBars,
    /// Flat-topped blocks on a two-pixel grid.
    Blocks,
}

impl WaveStyle {
    pub const ALL: [WaveStyle; 4] = [
        WaveStyle::Plain,
        WaveStyle::Smooth,
        WaveStyle::RoundBars,
        WaveStyle::Blocks,
    ];

    /// Its name in Settings.
    pub fn words(self) -> &'static str {
        match self {
            WaveStyle::Plain => "Plain wave",
            WaveStyle::Smooth => "Smooth peaks",
            WaveStyle::RoundBars => "Rounded bars",
            WaveStyle::Blocks => "Blocks",
        }
    }

    /// A line about it in Settings.
    pub fn explained(self) -> &'static str {
        match self {
            WaveStyle::Plain => {
                "A fine stroke for every pixel, from the lowest to the highest sample under it: the sound as recorded, uneven above and below."
            }
            WaveStyle::Smooth => "Rounded spikes: each hit rises and falls smoothly instead of a jagged saw.",
            WaveStyle::RoundBars => {
                "Separate bars with round ends: calm when zoomed out, finer as you zoom in."
            }
            WaveStyle::Blocks => "Flat-topped blocks on a grid, like a level meter laid on its side.",
        }
    }
}

/// The colours of clips with sound (Settings › Editing › Waveform colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveColouring {
    /// The track's colour on a charcoal clip, the same for every track (the usual).
    #[default]
    Charcoal,
    /// A quieter, greyed track colour on a grey clip.
    Grey,
    /// The colour says how loud: cool teal for quiet, green for the usual,
    /// amber and orange for loud; the track's colour an edge of the clip.
    Loudness,
}

impl WaveColouring {
    pub const ALL: [WaveColouring; 3] = [
        WaveColouring::Charcoal,
        WaveColouring::Grey,
        WaveColouring::Loudness,
    ];

    /// Its name in Settings.
    pub fn words(self) -> &'static str {
        match self {
            WaveColouring::Charcoal => "Track colour",
            WaveColouring::Grey => "Muted on grey",
            WaveColouring::Loudness => "Loudness map",
        }
    }

    /// A line about it in Settings.
    pub fn explained(self) -> &'static str {
        match self {
            WaveColouring::Charcoal => "The sound in its track's colour on a charcoal clip.",
            WaveColouring::Grey => "A quieter version of the track's colour on a grey clip: nothing loud.",
            WaveColouring::Loudness => {
                "The colour shows how loud: teal for quiet, green for the usual, amber and orange for loud, red when it clips."
            }
        }
    }

    /// The clip's fill.
    fn fill(self) -> Color32 {
        match self {
            WaveColouring::Charcoal => Color32::from_rgb(0x2B, 0x2C, 0x2E),
            WaveColouring::Grey => Color32::from_rgb(0x36, 0x37, 0x3A),
            WaveColouring::Loudness => Color32::from_rgb(0x26, 0x28, 0x2B),
        }
    }
}

/// Which part of the clip the sound takes (Settings › Editing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveSide {
    /// Above and below a middle line (the usual).
    #[default]
    Centred,
    /// Standing on the clip's floor, one side only: twice the room.
    Half,
}

/// How tall levels are drawn, in plain proportion to the sound: fitted
/// (the file's loudest peak, `top`, reaches the edge) or straight (full
/// scale does).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveScale {
    Fitted { top: f32 },
    Straight,
}

impl WaveScale {
    /// The scale fitted to `peaks` (a whole file's): its loudest reaches
    /// the edge. Silence gives the straight scale.
    pub fn fit(peaks: &[f32]) -> WaveScale {
        let top = peaks.iter().fold(0f32, |m, v| m.max(*v)).min(1.0);
        if top <= 0.0 {
            WaveScale::Straight
        } else {
            WaveScale::Fitted { top }
        }
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
            WaveScale::Straight => level,
            WaveScale::Fitted { top } => level / top,
        };
        (share.min(1.0), over)
    }
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

/// The scale fitted to a whole file's `peaks`, kept per file and stream
/// (`id`, as [`crate::waveform::levels_for`] keeps levels) while they stay
/// the same length: worked out once, not every frame.
pub fn fitted_scale(ctx: &egui::Context, id: egui::Id, peaks: &[f32]) -> WaveScale {
    type Fits = std::collections::HashMap<egui::Id, (usize, WaveScale)>;
    let key = egui::Id::unique("waveform fitted scales");
    let kept = ctx.data(|d| {
        d.get_temp::<std::sync::Arc<Fits>>(key)
            .and_then(|fits| fits.get(&id).copied())
    });
    match kept {
        Some((len, scale)) if len == peaks.len() => scale,
        _ => {
            let scale = WaveScale::fit(peaks);
            ctx.data_mut(|d| {
                let fits = d.get_temp_mut_or_default::<std::sync::Arc<Fits>>(key);
                std::sync::Arc::make_mut(fits).insert(id, (peaks.len(), scale));
            });
            scale
        }
    }
}

/// One pixel column, every level already times the volume: its loudest
/// peak, its usual level, and how far its sound swings up and down (the
/// same as the peak when only the loudness is known).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub x: f32,
    pub peak: f32,
    pub body: f32,
    pub up: f32,
    pub down: f32,
}

/// The waveform settings, as every page draws sound by them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WaveSettings {
    pub style: WaveStyle,
    pub colouring: WaveColouring,
    pub side: WaveSide,
    /// On the file's own scale, else full scale at the edge.
    pub fitted: bool,
}

impl WaveSettings {
    /// How sound of `colour` is drawn by these settings.
    pub fn look(&self, colour: Color32) -> WaveLook {
        WaveLook {
            style: self.style,
            colouring: self.colouring,
            side: self.side,
            colour,
            muted: false,
        }
    }
}

/// How a clip's sound is drawn: the three settings, and its track's colour
/// and whether the track is muted.
#[derive(Debug, Clone, Copy)]
pub struct WaveLook {
    pub style: WaveStyle,
    pub colouring: WaveColouring,
    pub side: WaveSide,
    pub colour: Color32,
    pub muted: bool,
}

impl WaveLook {
    /// The sound's colour where nothing says how loud.
    fn ink(&self) -> Color32 {
        let ink = match self.colouring {
            WaveColouring::Charcoal | WaveColouring::Loudness => self.colour,
            WaveColouring::Grey => mix(self.colour, Color32::from_rgb(0xC8, 0xC8, 0xC8), 0.4),
        };
        if self.muted {
            muted(ink, self.colouring.fill())
        } else {
            ink
        }
    }

    /// The colour of a column `share` of the way up (loudness map), else the ink.
    fn ink_at(&self, share: f32) -> Color32 {
        match self.colouring {
            WaveColouring::Loudness => {
                let c = loudness_colour(share);
                if self.muted {
                    muted(c, self.colouring.fill())
                } else {
                    c
                }
            }
            _ => self.ink(),
        }
    }
}

/// The loudness map's colour `share` (0 to 1) of the way up.
pub fn loudness_colour(share: f32) -> Color32 {
    const STOPS: [(f32, [u8; 3]); 4] = [
        (0.0, [0x3B, 0xA9, 0xA0]),
        (0.45, [0x7C, 0xC6, 0x7A]),
        (0.75, [0xE8, 0xC2, 0x5A]),
        (1.0, [0xF0, 0x8A, 0x4B]),
    ];
    let share = share.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if share <= b {
            let t = (share - a) / (b - a);
            return mix(rgb(ca), rgb(cb), t);
        }
    }
    rgb(STOPS[3].1)
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// `a` moved `t` (0 to 1) of the way to `b`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A muted track's version of `c` on `fill`: grey, halfway into the fill.
fn muted(c: Color32, fill: Color32) -> Color32 {
    let l = (0.3 * f32::from(c.r()) + 0.59 * f32::from(c.g()) + 0.11 * f32::from(c.b())).round() as u8;
    mix(Color32::from_rgb(l, l, l), fill, 0.5)
}

/// A clip with sound's own body: its fill, the same for every track, and
/// for the loudness map the track's colour as an edge along its left side.
pub fn draw_body(p: &egui::Painter, r: Rect, look: WaveLook) {
    let mut fill = look.colouring.fill();
    if look.muted {
        fill = mix(fill, Color32::from_rgb(0x1A, 0x1B, 0x1C), 0.3);
    }
    p.rect_filled(r, 3.0, fill);
    if look.colouring == WaveColouring::Loudness {
        let edge = if look.muted {
            muted(look.colour, fill)
        } else {
            look.colour
        };
        let bar = Rect::from_min_max(r.min, pos2(r.left() + 3.0, r.bottom()));
        p.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 3,
                sw: 3,
                ne: 0,
                se: 0,
            },
            edge,
        );
    }
}

/// How far a column's stroke reaches above and below its base, in pixels
/// (whole ones), and whether it is over full scale. Centred, above and
/// below the middle from how far the sound swings up and down; on one
/// side, its loudest peak, all above the floor.
fn reach(scale: WaveScale, c: &Column, side: WaveSide, room: f32) -> (f32, f32, bool) {
    let (_, over) = scale.height(c.peak);
    let px = |level: f32| (scale.height(level).0 * room * ROOM).round().max(1.0);
    match side {
        WaveSide::Centred => (px(c.up), px(c.down), over),
        WaveSide::Half => (px(c.peak), 0.0, over),
    }
}

/// Draws `columns` in `area` as `look` says, on `scale`, louder than full
/// scale in red.
pub fn draw(p: &egui::Painter, area: Rect, columns: &[Column], look: WaveLook, scale: WaveScale) {
    if area.height() < 3.0 || columns.is_empty() {
        return;
    }
    // Whole pixels: the base on a pixel line, every stroke an exact rectangle.
    let (base, room) = match look.side {
        WaveSide::Centred => (area.center().y.round(), (area.height() / 2.0).floor()),
        WaveSide::Half => (area.bottom().round(), area.height().floor()),
    };
    if look.side == WaveSide::Centred {
        let line = mix(look.ink(), look.colouring.fill(), 0.6);
        p.rect_filled(
            Rect::from_min_max(pos2(area.left(), base), pos2(area.right(), base + 1.0)),
            0.0,
            line,
        );
    }
    let mut mesh = egui::Mesh::default();
    match look.style {
        WaveStyle::Plain => {
            for c in columns {
                let (up, down, over) = reach(scale, c, look.side, room);
                let colour = if over {
                    OVER
                } else {
                    look.ink_at(scale.height(c.peak).0)
                };
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(c.x, base - up), pos2(c.x + 1.0, base + down.max(0.0))),
                    colour,
                );
            }
            p.add(Shape::mesh(mesh));
        }
        WaveStyle::Smooth => {
            let smooth = smoothed(columns);
            for c in &smooth {
                let (up, down, over) = reach(scale, c, look.side, room);
                let colour = if over {
                    OVER
                } else {
                    look.ink_at(scale.height(c.peak).0)
                };
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(c.x, base - up), pos2(c.x + 1.0, base + down.max(0.0))),
                    colour,
                );
            }
            p.add(Shape::mesh(mesh));
        }
        WaveStyle::RoundBars => {
            for group in columns.chunks(BAR_EVERY) {
                let c = loudest(group);
                let (up, down, over) = reach(scale, &c, look.side, room);
                let colour = if over {
                    OVER
                } else {
                    look.ink_at(scale.height(c.peak).0)
                };
                let bar = Rect::from_min_max(
                    pos2(c.x + 1.0, base - up),
                    pos2(c.x + 1.0 + BAR_WIDTH, base + down),
                );
                p.rect_filled(bar, BAR_WIDTH / 2.0, colour);
            }
        }
        WaveStyle::Blocks => {
            for group in columns.chunks(BLOCK_EVERY) {
                let c = loudest(group);
                let (up, down, over) = reach(scale, &c, look.side, room);
                // On a two-pixel grid: flat tops in steps.
                let step = |v: f32| ((v / 2.0).round() * 2.0).max(if v > 0.0 { 2.0 } else { 0.0 });
                let colour = if over {
                    OVER
                } else {
                    look.ink_at(scale.height(c.peak).0)
                };
                mesh.add_colored_rect(
                    Rect::from_min_max(
                        pos2(c.x, base - step(up)),
                        pos2(c.x + BLOCK_WIDTH, base + step(down)),
                    ),
                    colour,
                );
            }
            p.add(Shape::mesh(mesh));
        }
    }
    // A clipped column's tip: a bright cap at the edge, so it reads at a glance.
    let edges: &[f32] = match look.side {
        WaveSide::Half => &[0.0],
        WaveSide::Centred => &[0.0, 1.0],
    };
    for c in columns.iter().filter(|c| c.peak > 1.0) {
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

/// The loudest of a group of columns, at the first one's place.
fn loudest(group: &[Column]) -> Column {
    let first = group.first().map_or(0.0, |c| c.x);
    group.iter().fold(
        Column {
            x: first,
            peak: 0.0,
            body: 0.0,
            up: 0.0,
            down: 0.0,
        },
        |m, c| Column {
            x: m.x,
            peak: m.peak.max(c.peak),
            body: m.body.max(c.body),
            up: m.up.max(c.up),
            down: m.down.max(c.down),
        },
    )
}

/// Every column as high as the highest of its neighbours within
/// [`SMOOTH_REACH`] would reach there, falling off as a curve from each:
/// a peak keeps its height and gets smooth sides, so hits are rounded
/// spikes and the quiet between them stays quiet.
fn smoothed(columns: &[Column]) -> Vec<Column> {
    let n = columns.len();
    let reach = SMOOTH_REACH as f32 + 1.0;
    (0..n)
        .map(|i| {
            let mut out = columns[i];
            let first = i.saturating_sub(SMOOTH_REACH);
            let near = &columns[first..(i + SMOOTH_REACH + 1).min(n)];
            for (j, c) in near.iter().enumerate().map(|(k, c)| (first + k, c)) {
                let d = (i as f32 - j as f32) / reach;
                let fall = 1.0 - d * d;
                // Over full scale stays only where it was.
                out.peak = out
                    .peak
                    .max((c.peak * fall).min(if j == i { f32::MAX } else { 1.0 }));
                out.up = out.up.max(c.up * fall);
                out.down = out.down.max(c.down * fall);
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_are_in_plain_proportion_and_over_full_scale_is_marked() {
        let s = WaveScale::straight();
        assert_eq!(s.height(0.0), (0.0, false));
        assert_eq!(s.height(0.5), (0.5, false), "half as loud, half as tall");
        assert_eq!(s.height(2.0), (1.0, true), "louder: the edge, in red");
        let fitted = WaveScale::fit(&[0.1, 0.25, 0.5]);
        assert_eq!(
            fitted.height(0.5),
            (1.0, false),
            "the file's loudest reaches the edge"
        );
        assert_eq!(fitted.height(0.25), (0.5, false));
        assert!(fitted.height(1.5).1, "over full scale stays marked");
        assert_eq!(WaveScale::fit(&[0.0; 50]), WaveScale::Straight, "silence");
    }

    #[test]
    fn the_usual_level_is_the_average_of_an_even_sample() {
        let peaks = [0.0, 1.0, 0.0, 1.0];
        assert_eq!(body(&peaks, 0, 4), 0.5);
        assert_eq!(body(&peaks, 1, 2), 1.0);
        assert_eq!(body(&peaks, 3, 2), 0.0, "nothing between");
        assert_eq!(body(&peaks, 0, 99), 0.5, "held to the peaks there are");
    }

    fn column(x: f32, up: f32, down: f32) -> Column {
        Column {
            x,
            peak: up.max(down),
            body: up.max(down) / 2.0,
            up,
            down,
        }
    }

    #[test]
    fn a_stroke_reaches_up_and_down_as_far_as_the_sound_on_whole_pixels() {
        let s = WaveScale::straight();
        let (up, down, over) = reach(s, &column(0.0, 0.5, 0.25), WaveSide::Centred, 20.0);
        assert_eq!(
            (up, down, over),
            (
                (0.5f32 * 20.0 * ROOM).round(),
                (0.25f32 * 20.0 * ROOM).round(),
                false
            )
        );
        assert_eq!(up.fract(), 0.0, "whole pixels");
        let (up, down, _) = reach(s, &column(0.0, 0.5, 0.25), WaveSide::Half, 40.0);
        assert_eq!(
            (up, down),
            ((0.5f32 * 40.0 * ROOM).round(), 0.0),
            "one side: its loudest, from the floor"
        );
        let (up, down, _) = reach(s, &column(0.0, 0.0, 0.0), WaveSide::Centred, 20.0);
        assert_eq!((up, down), (1.0, 1.0), "silence is still a hairline");
    }

    #[test]
    fn smooth_peaks_round_a_lone_hit_without_losing_it() {
        let mut cols: Vec<Column> = (0..21).map(|x| column(x as f32, 0.05, 0.05)).collect();
        cols[10] = column(10.0, 0.9, 0.9);
        let smooth = smoothed(&cols);
        assert_eq!(smooth[10].up, 0.9, "the hit keeps its height");
        assert!(
            smooth[8].up > 0.05 && smooth[8].up < smooth[10].up,
            "its sides rise to it"
        );
        assert!((smooth[0].up - 0.05).abs() < 1e-6, "far away, as it was");
    }

    #[test]
    fn every_setting_has_words_and_the_loudness_map_warms_with_level() {
        for style in WaveStyle::ALL {
            assert!(!style.words().is_empty() && !style.explained().is_empty());
        }
        for colouring in WaveColouring::ALL {
            assert!(!colouring.words().is_empty() && !colouring.explained().is_empty());
        }
        let (quiet, loud) = (loudness_colour(0.0), loudness_colour(1.0));
        assert!(quiet.b() > quiet.r() && loud.r() > loud.b(), "teal to orange");
        let look = WaveLook {
            style: WaveStyle::Plain,
            colouring: WaveColouring::Charcoal,
            side: WaveSide::Centred,
            colour: Color32::from_rgb(0x3E, 0x8E, 0x7E),
            muted: false,
        };
        assert_eq!(look.ink(), look.colour, "the track's own colour");
        let quiet_look = WaveLook { muted: true, ..look };
        assert!(quiet_look.ink().g() < look.ink().g(), "a muted track is dimmer");
    }

    #[test]
    fn older_looks_are_read_as_the_new_shapes() {
        let read = |s: &str| ron::from_str::<WaveStyle>(s).unwrap();
        assert_eq!(read("Centred"), WaveStyle::Plain);
        assert_eq!(read("Outline"), WaveStyle::Plain);
        assert_eq!(read("Bars"), WaveStyle::RoundBars);
    }
}
