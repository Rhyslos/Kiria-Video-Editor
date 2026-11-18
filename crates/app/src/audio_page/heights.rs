//! How the Audio page shares its height: the top row (the viewer and the
//! tools), the band across the middle, and the bottom (the bar and the
//! timeline, the mixer or both); stacked Split shares the bottom between
//! the timeline (with the bar over it) and the mixer. A handle in each gap
//! (`ui_kit::splitter::stack`) moves the boundary; what it was dragged to
//! is kept as a share of the page's height in [`Heights`], a setting.
//! [`layout`] is the whole arithmetic: the page's rectangle and the kept
//! shares in, every part's rectangle out.

use super::{Bottom, SplitWay};
use egui::{Rect, pos2, vec2};
use serde::{Deserialize, Serialize};
use ui_kit::splitter::Part;

/// Room between the page's parts, where the handles are.
pub const GAP: f32 = 10.0;
/// The bar over the bottom views, and the room under it.
pub const BAR_HEIGHT: f32 = 30.0;
const UNDER_BAR: f32 = 6.0;
/// The least height of the top row: the viewer's picture and its
/// controls, and every tool's settings (the Compressor's six rows and its
/// sentence the tallest).
pub const LEAST_TOP: f32 = 260.0;
/// The band's usual and least height: its column of five views.
pub const USUAL_BAND: f32 = 112.0;
/// The least height of the timeline under the bar: its ruler, the video
/// reference row and an audio track.
pub const LEAST_TIMELINE: f32 = 130.0;
/// The least height of the mixer: a strip's names, tags, a fader with room
/// to move, its volume and buttons, and the master's settings.
pub const LEAST_MIXER: f32 = 240.0;
/// How many timeline tracks show above the mixer when Split is stacked.
const STACKED_TRACKS: f32 = 3.0;

/// The shares of the page's height the parts were dragged to (`None`: the
/// usual height). A setting, the same for every project.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Heights {
    /// The top row: the viewer and the tools.
    pub top: Option<f32>,
    /// The band across the middle.
    pub band: Option<f32>,
    /// Stacked Split: the timeline with the bar over it (the mixer has the rest).
    pub timeline: Option<f32>,
}

/// Where everything on the page goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub top: Rect,
    pub band: Rect,
    pub bar: Rect,
    pub timeline: Option<Rect>,
    pub mixer: Option<Rect>,
}

/// Whether the bottom is stacked Split (four parts rather than three).
fn stacked(bottom: Bottom, way: SplitWay) -> bool {
    bottom == Bottom::Split && way == SplitWay::Stacked
}

/// The parts of a page `height` tall, top to bottom, with the shares kept.
pub fn parts(height: f32, kept: &Heights, bottom: Bottom, way: SplitWay) -> Vec<Part> {
    let top = (height * 0.36).clamp(LEAST_TOP, 420.0);
    let top_part = Part::new(LEAST_TOP, top, kept.top);
    let band = Part::new(USUAL_BAND, USUAL_BAND, kept.band);
    let bar = BAR_HEIGHT + UNDER_BAR;
    if stacked(bottom, way) {
        // A few tracks over the mixer, never more than about half of what
        // is under the bar.
        let under_bar = height - top - USUAL_BAND - 2.0 * GAP - bar;
        let tracks = 26.0 + 18.0 + 30.0 + STACKED_TRACKS * ui_kit::timeline::AUDIO_TRACK * 0.7 + 14.0;
        let timeline = tracks.min(under_bar * 0.55).max(LEAST_TIMELINE);
        vec![
            top_part,
            band,
            Part::new(bar + LEAST_TIMELINE, bar + timeline, kept.timeline),
            Part::new(LEAST_MIXER, 0.0, None),
        ]
    } else {
        let least = match bottom {
            Bottom::Timeline => LEAST_TIMELINE,
            Bottom::Mixer | Bottom::Split => LEAST_MIXER,
        };
        vec![top_part, band, Part::new(bar + least, 0.0, None)]
    }
}

/// Keeps the parts' shares in `kept` (the mixer's, last, is what is left).
pub fn keep(parts: &[Part], kept: &mut Heights) {
    if let Some(p) = parts.first() {
        kept.top = p.kept;
    }
    if let Some(p) = parts.get(1) {
        kept.band = p.kept;
    }
    if parts.len() == 4 {
        kept.timeline = parts[2].kept;
    }
}

/// What each handle shares, said in its tip.
pub fn tips(bottom: Bottom, way: SplitWay) -> &'static [&'static str] {
    if stacked(bottom, way) {
        &[
            "Drag to share the height between the viewer and the band under it.",
            "Drag to share the height between the band and the timeline under it.",
            "Drag to share the height between the timeline and the mixer.",
        ]
    } else {
        &[
            "Drag to share the height between the viewer and the band under it.",
            "Drag to share the height between the band and what is under it.",
        ]
    }
}

/// The rectangles of the parts (as `ui_kit::splitter::stacked` or
/// `stack` gives them) turned into the page's places.
pub fn places(parts: &[Rect], bottom: Bottom, way: SplitWay) -> Layout {
    let at = |i: usize| parts.get(i).copied().unwrap_or(Rect::NOTHING);
    let (top, band, lower) = (at(0), at(1), at(2));
    let bar = Rect::from_min_size(lower.min, vec2(lower.width(), BAR_HEIGHT));
    let under = Rect::from_min_max(pos2(lower.left(), bar.bottom() + UNDER_BAR), lower.max);
    let (timeline, mixer) = match (bottom, way) {
        (Bottom::Timeline, _) => (Some(under), None),
        (Bottom::Mixer, _) => (None, Some(under)),
        (Bottom::Split, SplitWay::Stacked) => (Some(under), Some(at(3))),
        (Bottom::Split, SplitWay::SideBySide) => {
            let strips =
                ((under.width() * 0.42).max(ui_kit::mixer::STRIP_WIDTH * 3.0)).min(under.width() - 400.0);
            let mixer = Rect::from_min_size(
                under.min,
                vec2(
                    strips + ui_kit::layout::MASTER_WIDTH + super::MASTER_SETTINGS + 8.0,
                    under.height(),
                ),
            );
            let timeline = Rect::from_min_max(pos2(mixer.right() + GAP, under.top()), under.max);
            (Some(timeline), Some(mixer))
        }
    };
    Layout {
        top,
        band,
        bar,
        timeline,
        mixer,
    }
}

/// The whole page laid out in `page` with the shares kept: what the page
/// draws when no handle moves this frame (the page itself goes through
/// `ui_kit::splitter::stack`, which also moves the handles).
#[cfg(test)]
pub fn layout(page: Rect, kept: &Heights, bottom: Bottom, way: SplitWay) -> Layout {
    let parts = parts(page.height(), kept, bottom, way);
    let rects = ui_kit::splitter::stacked(page, GAP, &parts);
    places(&rects, bottom, way)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(h: f32) -> Rect {
        Rect::from_min_size(pos2(16.0, 64.0), vec2(1408.0, h))
    }

    const SPLIT: (Bottom, SplitWay) = (Bottom::Split, SplitWay::SideBySide);
    const STACKED: (Bottom, SplitWay) = (Bottom::Split, SplitWay::Stacked);

    #[test]
    fn the_usual_layout_is_the_one_the_page_always_had() {
        let l = layout(page(820.0), &Heights::default(), SPLIT.0, SPLIT.1);
        let top = (820.0f32 * 0.36).clamp(LEAST_TOP, 420.0);
        assert_eq!(l.top.min, pos2(16.0, 64.0));
        assert_eq!(l.top.height(), top);
        assert_eq!(l.band.top(), l.top.bottom() + GAP);
        assert_eq!(l.band.height(), USUAL_BAND);
        assert_eq!(l.bar.top(), l.band.bottom() + GAP);
        assert_eq!(l.bar.height(), BAR_HEIGHT);
        let (t, m) = (l.timeline.unwrap(), l.mixer.unwrap());
        assert_eq!(t.bottom(), page(820.0).bottom());
        assert_eq!(m.top(), l.bar.bottom() + 6.0);
        assert_eq!(t.left(), m.right() + GAP, "side by side");
    }

    #[test]
    fn kept_shares_follow_the_page_and_every_part_keeps_its_least() {
        let kept = Heights {
            top: Some(0.3),
            band: Some(0.4),
            timeline: None,
        };
        let l = layout(page(1000.0), &kept, SPLIT.0, SPLIT.1);
        let room = 1000.0 - 2.0 * GAP;
        assert!((l.top.height() - 0.3 * room).abs() < 0.01);
        assert!(
            (l.band.height() - 0.4 * room).abs() < 0.01,
            "a tall band: {:?}",
            l.band
        );
        // A shorter window: the mixer keeps its faders, the others give way.
        let l = layout(page(700.0), &kept, SPLIT.0, SPLIT.1);
        assert!(l.mixer.unwrap().height() >= LEAST_MIXER - 0.01, "{l:?}");
        assert!(l.top.height() >= LEAST_TOP && l.band.height() >= USUAL_BAND);
        assert_eq!(l.mixer.unwrap().bottom(), page(700.0).bottom());
        // A small band and a small viewer never go under their least.
        let tiny = Heights {
            top: Some(0.01),
            band: Some(0.01),
            timeline: Some(0.01),
        };
        let l = layout(page(1000.0), &tiny, STACKED.0, STACKED.1);
        assert_eq!(l.top.height(), LEAST_TOP);
        assert_eq!(l.band.height(), USUAL_BAND);
        assert_eq!(l.timeline.unwrap().height(), LEAST_TIMELINE);
    }

    #[test]
    fn stacked_split_shares_the_bottom_between_timeline_and_mixer() {
        let l = layout(page(820.0), &Heights::default(), STACKED.0, STACKED.1);
        let (t, m) = (l.timeline.unwrap(), l.mixer.unwrap());
        assert_eq!(t.top(), l.bar.bottom() + 6.0);
        assert_eq!(m.top(), t.bottom() + GAP);
        assert_eq!(m.bottom(), page(820.0).bottom());
        assert!(t.height() >= LEAST_TIMELINE && m.height() >= LEAST_MIXER, "{l:?}");
        // The timeline dragged taller: the mixer gives way, down to its faders.
        let mut parts = parts(1000.0, &Heights::default(), STACKED.0, STACKED.1);
        let room = 1000.0 - 3.0 * GAP;
        ui_kit::splitter::drag(room, &mut parts, 2, 1_000.0);
        let mut kept = Heights::default();
        keep(&parts, &mut kept);
        assert!(kept.timeline.is_some() && kept.top.is_none() && kept.band.is_none());
        let l = layout(page(1000.0), &kept, STACKED.0, STACKED.1);
        assert!((l.mixer.unwrap().height() - LEAST_MIXER).abs() < 0.01, "{l:?}");
    }

    #[test]
    fn dragging_the_band_and_double_clicking_puts_it_back() {
        let usual = layout(page(1000.0), &Heights::default(), SPLIT.0, SPLIT.1);
        let mut parts = parts(1000.0, &Heights::default(), SPLIT.0, SPLIT.1);
        let room = 1000.0 - 2.0 * GAP;
        // The band's lower edge down 150: a taller band, the mixer smaller.
        ui_kit::splitter::drag(room, &mut parts, 1, 150.0);
        let mut kept = Heights::default();
        keep(&parts, &mut kept);
        assert_eq!(kept.top, None, "the viewer was not moved");
        let l = layout(page(1000.0), &kept, SPLIT.0, SPLIT.1);
        assert!((l.band.height() - (USUAL_BAND + 150.0)).abs() < 0.01);
        assert_eq!(l.top, usual.top);
        // The viewer's edge up: the band grows into it, its lower edge stays.
        ui_kit::splitter::drag(room, &mut parts, 0, -40.0);
        keep(&parts, &mut kept);
        let l2 = layout(page(1000.0), &kept, SPLIT.0, SPLIT.1);
        assert!((l2.top.height() - (usual.top.height() - 40.0)).abs() < 0.01);
        assert!((l2.bar.top() - l.bar.top()).abs() < 0.01);
        // Double-click on each handle: the usual page again.
        ui_kit::splitter::reset(room, &mut parts, 0);
        ui_kit::splitter::reset(room, &mut parts, 1);
        keep(&parts, &mut kept);
        assert_eq!(kept, Heights::default());
        assert_eq!(layout(page(1000.0), &kept, SPLIT.0, SPLIT.1), usual);
    }

    #[test]
    fn each_bottom_view_keeps_room_for_itself() {
        let kept = Heights {
            top: Some(0.6),
            band: Some(0.3),
            timeline: None,
        };
        let l = layout(page(820.0), &kept, Bottom::Timeline, SplitWay::SideBySide);
        assert!(
            (l.timeline.unwrap().height() - LEAST_TIMELINE).abs() < 0.01,
            "{l:?}"
        );
        assert!(l.mixer.is_none());
        let l = layout(page(820.0), &kept, Bottom::Mixer, SplitWay::SideBySide);
        assert!((l.mixer.unwrap().height() - LEAST_MIXER).abs() < 0.01, "{l:?}");
        assert!(l.timeline.is_none());
    }
}
