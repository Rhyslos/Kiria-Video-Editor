//! Track rows: how tall each track is, and where its row sits in its area.
//!
//! Every track can have its own height. Heights are view state: the page
//! keeps them (and may save them beside the project), and changing one is
//! never an edit, so it is not undone or redone.

use egui::{Rect, pos2};
use std::collections::HashMap;

/// The shortest a track can be made: its name and clip names still fit.
pub const MIN_TRACK: f32 = 28.0;
/// The tallest a track can be made.
pub const MAX_TRACK: f32 = 400.0;

/// How tall each track is, as a factor of the usual height of its kind in
/// the current view (so a track made twice as tall is twice as tall in the
/// rough cut view too). Tracks never touched have no entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackHeights {
    factors: HashMap<u64, f32>,
    /// Counts changes, so the page can tell when to save.
    revision: u64,
}

impl TrackHeights {
    /// Heights as they were saved: track ids with their factors.
    pub fn from_saved(saved: impl IntoIterator<Item = (u64, f32)>) -> TrackHeights {
        TrackHeights {
            factors: saved
                .into_iter()
                .filter(|(_, f)| f.is_finite() && *f > 0.0 && (*f - 1.0).abs() > f32::EPSILON)
                .collect(),
            revision: 0,
        }
    }

    /// What to save: track ids with their factors, in id order.
    pub fn to_save(&self) -> Vec<(u64, f32)> {
        let mut all: Vec<(u64, f32)> = self.factors.iter().map(|(id, f)| (*id, *f)).collect();
        all.sort_by_key(|(id, _)| *id);
        all
    }

    /// A number that changes whenever a height does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The height of `track`, whose kind is usually `usual` tall.
    pub fn height(&self, track: u64, usual: f32) -> f32 {
        match self.factors.get(&track) {
            Some(factor) => (usual * factor).clamp(MIN_TRACK, MAX_TRACK),
            None => usual,
        }
    }

    /// Makes `track` `height` tall, within the limits.
    pub fn set(&mut self, track: u64, usual: f32, height: f32) {
        if usual <= 0.0 || !height.is_finite() {
            return;
        }
        let factor = height.clamp(MIN_TRACK, MAX_TRACK) / usual;
        if self.factors.get(&track) != Some(&factor) {
            self.factors.insert(track, factor);
            self.revision += 1;
        }
    }

    /// Back to the usual height.
    pub fn reset(&mut self, track: u64) {
        if self.factors.remove(&track).is_some() {
            self.revision += 1;
        }
    }

    /// Whether `track` has a height of its own.
    pub fn is_changed(&self, track: u64) -> bool {
        self.factors.contains_key(&track)
    }

    /// Makes each of `tracks` `factor` times as tall as it is, within the limits.
    pub fn scale(&mut self, tracks: impl IntoIterator<Item = u64>, usual: f32, factor: f32) {
        for track in tracks {
            self.set(track, usual, self.height(track, usual) * factor);
        }
    }
}

/// The rows of one area, one per track, in track order.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rows {
    rects: Vec<Rect>,
    content: f32,
}

impl Rows {
    /// Rows of the given `heights` inside `area`, scrolled by `scroll`.
    /// Video rows stack upward from the area's bottom (the first track
    /// lowest); audio rows stack downward from its top.
    pub fn new(area: Rect, heights: impl IntoIterator<Item = f32>, scroll: f32, from_bottom: bool) -> Rows {
        let mut offset = 0.0;
        let rects = heights
            .into_iter()
            .map(|height| {
                let rect = if from_bottom {
                    let bottom = area.bottom() - offset + scroll;
                    Rect::from_min_max(pos2(area.left(), bottom - height), pos2(area.right(), bottom))
                } else {
                    let top = area.top() + offset - scroll;
                    Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + height))
                };
                offset += height;
                rect
            })
            .collect();
        Rows {
            rects,
            content: offset,
        }
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// The index of the track whose row holds screen position `y`.
    pub fn at(&self, y: f32) -> Option<usize> {
        self.rects.iter().position(|r| r.top() <= y && y < r.bottom())
    }

    /// How far an area `visible` tall can scroll before its last row is fully shown.
    pub fn max_scroll(&self, visible: f32) -> f32 {
        (self.content - visible).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 300.0))
    }

    #[test]
    fn video_rows_grow_up_and_audio_rows_down() {
        let v = Rows::new(area(), [50.0, 50.0], 0.0, true);
        assert_eq!(
            (v.rects()[0].top(), v.rects()[0].bottom()),
            (250.0, 300.0),
            "V1 at the bottom"
        );
        assert_eq!(v.rects()[1].bottom(), 250.0);
        let a = Rows::new(area(), [40.0, 40.0], 10.0, false);
        assert_eq!((a.rects()[0].top(), a.rects()[1].top()), (90.0, 130.0));
    }

    #[test]
    fn rows_of_mixed_heights_stack_without_gaps_and_are_found_by_position() {
        let a = Rows::new(area(), [40.0, 120.0, 28.0], 0.0, false);
        let tops: Vec<f32> = a.rects().iter().map(Rect::top).collect();
        assert_eq!(tops, [100.0, 140.0, 260.0]);
        assert_eq!(a.rects()[2].bottom(), 288.0);
        assert_eq!(a.at(100.0), Some(0));
        assert_eq!(a.at(139.9), Some(0));
        assert_eq!(a.at(140.0), Some(1), "an edge belongs to the row below it");
        assert_eq!(a.at(259.0), Some(1));
        assert_eq!(a.at(270.0), Some(2));
        assert_eq!(a.at(288.0), None, "under the last row");
        assert_eq!(a.at(99.0), None);

        // The same three tracks as video: the first is lowest.
        let v = Rows::new(area(), [40.0, 120.0, 28.0], 0.0, true);
        assert_eq!(v.at(299.0), Some(0));
        assert_eq!(v.at(259.0), Some(1));
        assert_eq!(v.at(139.0), Some(2));
        assert_eq!(v.at(111.0), None, "above the top row");
        // Scrolled by 20: every row 20 lower, so higher tracks come into view.
        let v = Rows::new(area(), [40.0, 120.0, 28.0], 20.0, true);
        assert_eq!(v.rects()[0].bottom(), 320.0);
        assert_eq!(v.at(135.0), Some(2));
        assert_eq!(v.at(131.0), None, "the top row begins at 132 now");
    }

    #[test]
    fn scrolling_stops_when_the_last_row_is_in_view() {
        let a = Rows::new(area(), [100.0, 150.0, 50.0], 0.0, false);
        assert_eq!(a.max_scroll(200.0), 100.0);
        assert_eq!(a.max_scroll(400.0), 0.0, "everything fits: nothing to scroll");
    }

    #[test]
    fn heights_are_factors_of_the_usual_height_within_limits() {
        let mut h = TrackHeights::default();
        assert_eq!(h.height(1, 58.0), 58.0);
        assert!(!h.is_changed(1));
        h.set(1, 58.0, 116.0);
        assert_eq!(h.height(1, 58.0), 116.0);
        assert_eq!(h.height(1, 96.0), 192.0, "twice as tall in the other view too");
        assert_eq!(h.height(2, 58.0), 58.0, "other tracks keep theirs");
        h.set(1, 58.0, 5.0);
        assert_eq!(h.height(1, 58.0), MIN_TRACK);
        h.set(1, 58.0, 5000.0);
        assert_eq!(h.height(1, 58.0), MAX_TRACK);
        assert_eq!(h.height(1, 96.0), MAX_TRACK, "the limits hold in every view");
        h.reset(1);
        assert_eq!(h.height(1, 58.0), 58.0);
    }

    #[test]
    fn scaling_changes_every_track_given_and_no_other() {
        let mut h = TrackHeights::default();
        h.set(1, 50.0, 100.0);
        h.scale([1, 2], 50.0, 1.5);
        assert_eq!(h.height(1, 50.0), 150.0);
        assert_eq!(h.height(2, 50.0), 75.0);
        assert_eq!(h.height(3, 50.0), 50.0);
        h.scale([1, 2], 50.0, 100.0);
        assert_eq!((h.height(1, 50.0), h.height(2, 50.0)), (MAX_TRACK, MAX_TRACK));
    }

    #[test]
    fn the_revision_counts_real_changes_only() {
        let mut h = TrackHeights::default();
        let start = h.revision();
        h.reset(7);
        assert_eq!(h.revision(), start, "nothing to reset");
        h.set(7, 50.0, 80.0);
        let changed = h.revision();
        assert_ne!(changed, start);
        h.set(7, 50.0, 80.0);
        assert_eq!(h.revision(), changed, "the same height again");
        h.reset(7);
        assert_ne!(h.revision(), changed);
    }

    #[test]
    fn saved_heights_come_back_and_nonsense_is_dropped() {
        let mut h = TrackHeights::default();
        h.set(9, 50.0, 100.0);
        h.set(3, 50.0, 25.0);
        let saved = h.to_save();
        assert_eq!(saved.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [3, 9]);
        let back = TrackHeights::from_saved(saved);
        assert_eq!(back.height(9, 50.0), 100.0);
        assert_eq!(back.height(3, 50.0), MIN_TRACK);
        let odd = TrackHeights::from_saved([(1, f32::NAN), (2, -3.0), (3, 0.0), (4, 2.0)]);
        assert_eq!(odd.to_save(), [(4, 2.0)]);
    }
}
