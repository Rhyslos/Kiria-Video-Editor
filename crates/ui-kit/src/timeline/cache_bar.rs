//! The cache bar: a thin strip under the ruler that shows, along the
//! timeline, what has been prepared ahead of time and what is still being
//! prepared.
//!
//! The widget only draws states over stretches of time. What they stand for
//! is the page's business: today the page marks where waveforms and clip
//! pictures are still being read; frames kept in memory, proxies and
//! pre-rendered stretches report through the same ranges when they exist.

use crate::Theme;
use egui::{Color32, Painter, Rect, pos2};

/// Height of the strip, at the top of the labels row.
pub(super) const HEIGHT: f32 = 4.0;

/// How ready a stretch of the timeline is. Ordered from most to least ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CacheState {
    /// Prepared: it shows or plays at once.
    Ready,
    /// Being prepared right now.
    Preparing,
    /// Not prepared, and waiting its turn.
    Waiting,
}

/// A stretch of the timeline (seconds) in one state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheRange {
    pub start: f64,
    pub end: f64,
    pub state: CacheState,
}

/// What the timeline draws in the cache bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheBar<'a> {
    /// In time order and not overlapping, as [`flatten`] returns them.
    pub ranges: &'a [CacheRange],
    /// What the states stand for on this page right now, for the tooltip.
    pub about: &'a str,
}

/// Makes one tidy list out of ranges that may overlap, in any order (one
/// per clip and per kind of work, say): sorted, not overlapping, with
/// touching ranges of the same state joined. Where ranges overlap, the
/// least ready state wins: a stretch is only as ready as its slowest part.
pub fn flatten(ranges: &[CacheRange]) -> Vec<CacheRange> {
    // Every range opens at its start and closes at its end; between two
    // neighbouring times the state is the least ready one still open.
    let mut edges: Vec<(f64, bool, CacheState)> = ranges
        .iter()
        .filter(|r| r.end > r.start && r.start.is_finite() && r.end.is_finite())
        .flat_map(|r| [(r.start, true, r.state), (r.end, false, r.state)])
        .collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut open = [0usize; 3];
    let mut out: Vec<CacheRange> = Vec::new();
    let mut from = 0.0;
    for (at, opens, state) in edges {
        let current = [CacheState::Waiting, CacheState::Preparing, CacheState::Ready]
            .into_iter()
            .find(|s| open[*s as usize] > 0);
        if let Some(state) = current
            && at > from
        {
            match out.last_mut() {
                Some(last) if last.state == state && last.end >= from => last.end = at,
                _ => out.push(CacheRange {
                    start: from,
                    end: at,
                    state,
                }),
            }
        }
        if opens {
            open[state as usize] += 1;
        } else {
            open[state as usize] = open[state as usize].saturating_sub(1);
        }
        from = at;
    }
    out
}

/// The stretches to paint for the part of `ranges` between the times
/// `visible`, as screen spans `(x0, x1, state)`. Ranges out of view are
/// skipped without being looked at one by one, and ranges too thin to tell
/// apart share a span, so thousands of ranges cost no more than the strip
/// has pixels.
pub(super) fn spans(
    ranges: &[CacheRange],
    visible: (f64, f64),
    x_of: impl Fn(f64) -> f32,
) -> Vec<(f32, f32, CacheState)> {
    let first = ranges.partition_point(|r| r.end < visible.0);
    let mut out: Vec<(f32, f32, CacheState)> = Vec::new();
    for range in &ranges[first..] {
        if range.start >= visible.1 {
            break;
        }
        let x0 = x_of(range.start.max(visible.0));
        let x1 = x_of(range.end.min(visible.1));
        match out.last_mut() {
            // Joins the span before it, or hides behind it; otherwise it
            // starts where that one ends and is at least a pixel wide.
            Some(last) if last.2 == range.state && x0 - last.1 < 1.0 => last.1 = last.1.max(x1),
            Some(last) if x1 <= last.1 => {}
            Some(last) => {
                let x0 = x0.max(last.1);
                out.push((x0, x1.max(x0 + 1.0), range.state));
            }
            None => out.push((x0, x1.max(x0 + 1.0), range.state)),
        }
    }
    out
}

/// The colour of each state: status colours, the same in every theme.
fn colour(theme: &Theme, state: CacheState) -> Color32 {
    match state {
        CacheState::Ready => crate::status::GOOD.gamma_multiply(0.8),
        CacheState::Preparing => crate::status::WARNING.gamma_multiply(0.85),
        CacheState::Waiting => theme.line_strong,
    }
}

/// Paints the strip.
pub(super) fn draw(
    p: &Painter,
    theme: &Theme,
    strip: Rect,
    bar: &CacheBar,
    visible: (f64, f64),
    x_of: impl Fn(f64) -> f32,
) {
    p.rect_filled(strip, 0.0, theme.bg);
    let spans = spans(bar.ranges, visible, x_of);
    if spans.is_empty() {
        return;
    }
    // One mesh for the lot: far cheaper than a shape per span.
    let mut mesh = egui::Mesh::default();
    for (x0, x1, state) in spans {
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x0, strip.top()), pos2(x1, strip.bottom())),
            colour(theme, state),
        );
    }
    p.add(egui::Shape::mesh(mesh));
}

/// What the strip's tooltip says: the colours, then the page's note.
pub(super) fn legend(about: &str) -> String {
    let colours = "Green: prepared ahead, shows at once. Yellow: being prepared now. Grey: waiting its turn. Empty: nothing prepared ahead of time.";
    if about.is_empty() {
        colours.to_owned()
    } else {
        format!("{colours}\n{about}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CacheState::{Preparing, Ready, Waiting};

    fn range(start: f64, end: f64, state: CacheState) -> CacheRange {
        CacheRange { start, end, state }
    }

    #[test]
    fn overlapping_ranges_become_one_list_where_the_least_ready_wins() {
        let flat = flatten(&[
            range(0.0, 10.0, Ready),
            range(4.0, 6.0, Preparing),
            range(5.0, 12.0, Waiting),
            range(20.0, 30.0, Preparing),
        ]);
        assert_eq!(
            flat,
            [
                range(0.0, 4.0, Ready),
                range(4.0, 5.0, Preparing),
                range(5.0, 12.0, Waiting),
                range(20.0, 30.0, Preparing),
            ]
        );
    }

    #[test]
    fn touching_ranges_of_one_state_join_and_order_does_not_matter() {
        let flat = flatten(&[
            range(8.0, 9.0, Ready),
            range(2.0, 4.0, Preparing),
            range(0.0, 2.0, Preparing),
            range(4.0, 8.0, Preparing),
            range(3.0, 3.0, Waiting),
            range(5.0, 1.0, Waiting),
            range(f64::NAN, 2.0, Waiting),
        ]);
        assert_eq!(
            flat,
            [range(0.0, 8.0, Preparing), range(8.0, 9.0, Ready)],
            "empty, backward and broken ranges are dropped"
        );
        assert!(flatten(&[]).is_empty());
    }

    #[test]
    fn a_gap_keeps_ranges_of_one_state_apart() {
        let flat = flatten(&[range(0.0, 1.0, Ready), range(2.0, 3.0, Ready)]);
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn only_ranges_in_view_are_drawn() {
        let ranges: Vec<CacheRange> = (0..10_000)
            .map(|i| {
                range(
                    f64::from(i) * 2.0,
                    f64::from(i) * 2.0 + 1.0,
                    if i % 2 == 0 { Ready } else { Preparing },
                )
            })
            .collect();
        // Ten pixels a second, showing 100 s to 110 s.
        let seen = spans(&ranges, (100.0, 110.0), |t| ((t - 100.0) * 10.0) as f32);
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert_eq!(seen[0], (0.0, 10.0, Ready));
        assert_eq!(seen[1], (20.0, 30.0, Preparing));
        // A range reaching in from before the view starts at its edge.
        let seen = spans(&[range(0.0, 105.0, Waiting)], (100.0, 110.0), |t| {
            ((t - 100.0) * 10.0) as f32
        });
        assert_eq!(seen, [(0.0, 50.0, Waiting)]);
    }

    #[test]
    fn ranges_too_thin_to_tell_apart_share_a_span() {
        // Ten thousand one-second ranges with gaps, all on screen 200 pixels wide.
        let same: Vec<CacheRange> = (0..10_000)
            .map(|i| range(f64::from(i) * 2.0, f64::from(i) * 2.0 + 1.0, Ready))
            .collect();
        let seen = spans(&same, (0.0, 20_000.0), |t| (t / 100.0) as f32);
        assert_eq!(seen.len(), 1, "one span for the lot");
        assert!(seen[0].0 == 0.0 && seen[0].1 >= 199.0);
        // Alternating states: never more spans than there are pixels to put them in.
        let mixed: Vec<CacheRange> = (0..10_000)
            .map(|i| {
                range(
                    f64::from(i) * 2.0,
                    f64::from(i) * 2.0 + 2.0,
                    if i % 2 == 0 { Ready } else { Waiting },
                )
            })
            .collect();
        let seen = spans(&mixed, (0.0, 20_000.0), |t| (t / 100.0) as f32);
        assert!(seen.len() <= 201, "{}", seen.len());
        assert!(seen.windows(2).all(|w| w[0].1 <= w[1].0), "spans don't overlap");
    }

    #[test]
    fn the_legend_explains_the_colours_and_adds_the_pages_note() {
        assert!(legend("").starts_with("Green"));
        assert!(legend("Reading waveforms.").ends_with("\nReading waveforms."));
    }
}
