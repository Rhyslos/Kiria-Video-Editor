//! Two ways of looking at the tracks that the Display menu switches, as
//! pure sums the widget lays its rows out by.
//!
//! - **Compact tracks**: every track, video and audio, a thin strip
//!   [`COMPACT_TRACK`] tall that still shows each clip's colour and name, so
//!   twenty tracks of each kind can be seen at a glance. The heights given
//!   to tracks are kept as they are and come back when it is turned off.
//! - **No divider**: the video tracks and the audio tracks are one list
//!   that scrolls as one (video on top, V1 lowest; audio under it), with no
//!   divider to drag. The divider's place is kept for when it comes back.
//!
//! In the one list the widget still has a video area and an audio area: the
//! parts of the list's window each kind's rows are in. [`OneList`] works
//! out where they are for a scroll, and the scroll each area's rows are
//! drawn with, so the rest of the widget treats them as it always does.

/// The height of every track with compact tracks on: a clip's name still
/// reads in it, and twenty video and twenty audio tracks take 720 points.
pub const COMPACT_TRACK: f32 = 18.0;

/// How far a clip is drawn inside its row, top and bottom: compact rows
/// keep almost all of their height for the clip.
pub(super) fn clip_inset(compact: bool) -> f32 {
    if compact { 1.0 } else { 3.0 }
}

/// Video and audio tracks as one list in a window `body` tall, scrolled
/// `scroll` points down from the top of the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OneList {
    /// The height of every video track together.
    pub video: f32,
    /// The height of every audio track together.
    pub audio: f32,
    pub body: f32,
    pub scroll: f32,
}

impl OneList {
    /// How far the list can scroll: until its last audio track is in view.
    pub fn max_scroll(&self) -> f32 {
        (self.video + self.audio - self.body).max(0.0)
    }

    /// The same list scrolled no further than it can go.
    pub fn in_range(self) -> OneList {
        OneList {
            scroll: self.scroll.clamp(0.0, self.max_scroll()),
            ..self
        }
    }

    /// How much of the window the video rows take, from its top: none once
    /// they are scrolled out of view, all of it while they fill it.
    pub fn video_area(&self) -> f32 {
        (self.video - self.scroll).clamp(0.0, self.body.max(0.0))
    }

    /// Where the last video row meets the first audio row, from the top of
    /// the window (negative above it, more than `body` below it).
    pub fn boundary(&self) -> f32 {
        self.video - self.scroll
    }

    /// The scrolls the video and the audio area's rows are drawn with, in
    /// the widget's own terms (video rows stack upward from the bottom of
    /// their area and shift down by their scroll; audio rows stack down
    /// from the top of theirs and shift up by it), so that both kinds of
    /// row lie end to end at [`Self::boundary`], for a video area
    /// `video_area` tall (as laid out; [`Self::video_area`] for this scroll).
    pub fn area_scrolls(&self, video_area: f32) -> (f32, f32) {
        let gap = self.boundary() - video_area;
        (gap, -gap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(video: f32, audio: f32, scroll: f32) -> OneList {
        OneList {
            video,
            audio,
            body: 300.0,
            scroll,
        }
    }

    #[test]
    fn twenty_compact_tracks_of_each_kind_fit_a_tall_timeline() {
        const { assert!(40.0 * COMPACT_TRACK <= 720.0) };
        assert!(
            COMPACT_TRACK - 2.0 * clip_inset(true) >= 14.0,
            "room for a clip's name"
        );
        assert_eq!(clip_inset(false), 3.0, "usual rows are drawn as before");
    }

    #[test]
    fn at_the_top_the_video_rows_come_first_and_audio_follows_under_them() {
        let l = list(100.0, 120.0, 0.0);
        assert_eq!(l.video_area(), 100.0);
        assert_eq!(l.boundary(), 100.0);
        assert_eq!(l.area_scrolls(l.video_area()), (0.0, 0.0), "nothing to shift");
        assert_eq!(l.max_scroll(), 0.0, "220 points fit in 300");
    }

    #[test]
    fn scrolled_past_the_video_rows_their_area_is_empty_and_audio_moves_up() {
        let l = list(100.0, 400.0, 150.0);
        assert_eq!(l.video_area(), 0.0);
        assert_eq!(l.boundary(), -50.0, "the first audio row begins above the window");
        let (video, audio) = l.area_scrolls(l.video_area());
        // Video rows end (V1's bottom) at the area's bottom (0) plus their scroll.
        assert_eq!(0.0 + video, -50.0);
        // Audio rows begin at the area's top (0) less their scroll.
        assert_eq!(0.0 - audio, -50.0);
        assert_eq!(l.max_scroll(), 200.0);
    }

    #[test]
    fn video_rows_taller_than_the_window_fill_it_and_audio_waits_below() {
        let l = list(500.0, 100.0, 0.0);
        assert_eq!(l.video_area(), 300.0, "all of the window");
        let (video, audio) = l.area_scrolls(l.video_area());
        assert_eq!(300.0 + video, 500.0, "V1's bottom is 200 points below the window");
        assert_eq!(300.0 - audio, 500.0, "and so is the first audio row's top");
        assert_eq!(l.max_scroll(), 300.0);
    }

    #[test]
    fn the_scroll_stays_between_the_top_and_the_last_track() {
        assert_eq!(list(500.0, 100.0, -20.0).in_range().scroll, 0.0);
        assert_eq!(list(500.0, 100.0, 900.0).in_range().scroll, 300.0);
        assert_eq!(list(50.0, 50.0, 30.0).in_range().scroll, 0.0, "nothing to scroll");
    }
}
