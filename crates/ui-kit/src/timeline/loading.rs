//! Background work a clip or a track is still waiting for, as the timeline
//! shows it: a slim bar with a percentage on the clip, and a short line by
//! the track's name.
//!
//! The page says what is going on ([`Loading`]); everything about how it
//! looks and reads is here, so every page says it the same way.

use crate::Theme;
use crate::time_text::time_left;
use egui::{Color32, FontId, Painter, Rect, pos2, vec2};

/// What is being read for a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    /// The sound, for the waveform.
    Sound,
    /// The small pictures drawn inside video clips.
    Pictures,
}

/// How far the reading has come.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadState {
    /// Waiting for a free worker (background work also waits while the timeline plays).
    Waiting,
    /// Being read: the share done (0 to 1) and, once it can be told
    /// steadily, the seconds left.
    Reading { done: f32, seconds_left: Option<f32> },
    /// It could not be read: the clip stays without it.
    Failed,
}

/// Background work that what a clip (or a track) draws is still waiting for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loading {
    pub what: LoadKind,
    pub state: LoadState,
}

impl LoadKind {
    fn noun(self) -> &'static str {
        match self {
            LoadKind::Sound => "sound",
            LoadKind::Pictures => "pictures",
        }
    }

    /// What a clip says when the reading failed.
    fn missing(self) -> &'static str {
        match self {
            LoadKind::Sound => "no waveform",
            LoadKind::Pictures => "no pictures",
        }
    }
}

/// "43 %": whole percent, never 100 before the work is done.
pub fn percent(done: f32) -> String {
    let done = if done.is_finite() { done } else { 0.0 };
    format!("{} %", ((done * 100.0).floor() as i32).clamp(0, 99))
}

impl Loading {
    /// The share done, for the bar (nothing while waiting or after failing).
    pub fn done(&self) -> f32 {
        match self.state {
            LoadState::Reading { done, .. } if done.is_finite() => done.clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// What a clip `width` points wide says: everything when there is room,
    /// less when there isn't, nothing when it is very narrow.
    pub fn clip_text(&self, width: f32) -> Option<String> {
        let noun = self.what.noun();
        match self.state {
            LoadState::Failed if width >= 90.0 => Some(self.what.missing().to_owned()),
            LoadState::Waiting if width >= 160.0 => Some(format!("Waiting to read {noun}")),
            LoadState::Waiting if width >= 70.0 => Some("Waiting".to_owned()),
            LoadState::Reading { done, seconds_left } => {
                let percent = percent(done);
                match seconds_left {
                    Some(left) if width >= 280.0 => {
                        Some(format!("Reading {noun} {percent} · {}", time_left(left)))
                    }
                    _ if width >= 150.0 => Some(format!("Reading {noun} {percent}")),
                    _ if width >= 44.0 => Some(percent),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The short line by a track's name.
    pub fn header_text(&self) -> Option<String> {
        self.header_texts().map(|[long, _]| long)
    }

    /// The line by a track's name, and a shorter one for a narrow header
    /// ("Reading 43 %" or "43 %", "Waiting to read" or "Waiting…"): the
    /// header shows the first that fits whole, never a word cut off.
    pub fn header_texts(&self) -> Option<[String; 2]> {
        match self.state {
            LoadState::Reading { done, .. } => Some([format!("Reading {}", percent(done)), percent(done)]),
            LoadState::Waiting => Some(["Waiting to read".to_owned(), "Waiting…".to_owned()]),
            LoadState::Failed => None,
        }
    }

    /// The whole story, for the tooltip of a track's line.
    pub fn explanation(&self) -> String {
        let (noun, purpose) = match self.what {
            LoadKind::Sound => ("sound", "to draw its waveforms"),
            LoadKind::Pictures => ("pictures", "to draw them inside its clips"),
        };
        match self.state {
            LoadState::Reading { done, seconds_left } => {
                let left = seconds_left.map_or_else(String::new, |s| format!(", {}", time_left(s)));
                format!(
                    "Reading this track's {noun} {purpose}: {} done{left}. You can keep editing and playing meanwhile.",
                    percent(done)
                )
            }
            LoadState::Waiting => format!(
                "Waiting to read this track's {noun} {purpose}. Background work takes turns, and pauses while the timeline plays."
            ),
            LoadState::Failed => format!("This track's {noun} could not be read."),
        }
    }
}

/// Draws a clip's loading over its contents: a slim bar along the bottom of
/// the part of the clip that is in view, and what it says above the bar.
pub(super) fn draw_on_clip(p: &Painter, theme: &Theme, clip: Rect, loading: &Loading) {
    let seen = clip.intersect(p.clip_rect());
    if seen.width() < 3.0 || clip.height() < 12.0 {
        return;
    }
    let inside = p.with_clip_rect(clip.shrink(1.0));
    let failed = loading.state == LoadState::Failed;
    if !failed {
        // In a short clip (compact tracks) a thin line along its bottom
        // edge, under the name.
        let (top, bottom) = match clip.height() < 24.0 {
            true => (clip.bottom() - 3.0, clip.bottom() - 1.0),
            false => (clip.bottom() - 5.0, clip.bottom() - 2.0),
        };
        let bar = Rect::from_min_max(pos2(seen.left() + 2.0, top), pos2(seen.right() - 2.0, bottom));
        inside.rect_filled(bar, 1.5, Color32::from_black_alpha(110));
        let done = bar.left() + bar.width() * loading.done();
        if done > bar.left() {
            inside.rect_filled(
                Rect::from_min_max(bar.min, pos2(done, bar.bottom())),
                1.5,
                theme.text,
            );
        }
    }
    let Some(text) = loading.clip_text(seen.width()) else {
        return;
    };
    // Words only when the clip is tall enough to have room under its name.
    if clip.height() < 34.0 {
        return;
    }
    let colour = if failed { theme.muted } else { theme.text };
    let galley = inside.layout_no_wrap(text, FontId::proportional(10.0), colour);
    let at = pos2(seen.left() + 6.0, clip.bottom() - 8.0 - galley.size().y);
    let pill = Rect::from_min_size(at - vec2(3.0, 1.0), galley.size() + vec2(6.0, 2.0));
    inside.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
    inside.galley(at, galley, colour);
}

/// Draws the short line of a track's loading in its header, in the band
/// beside the track's buttons, and returns the rectangle it took (for a
/// tooltip). Nothing when the header is too short for it.
/// Draws the line by a track's name in `header`, within `room` points
/// from its left: the longest wording that fits whole, or nothing.
pub(super) fn draw_in_header(
    p: &Painter,
    theme: &Theme,
    header: Rect,
    room: f32,
    loading: &Loading,
) -> Option<Rect> {
    if header.height() < 44.0 {
        return None;
    }
    let galley = loading
        .header_texts()?
        .into_iter()
        .map(|text| p.layout_no_wrap(text, FontId::proportional(9.5), theme.muted))
        .find(|g| g.size().x <= room)?;
    let at = pos2(header.left() + 12.0, header.top() + 11.0 - galley.size().y / 2.0);
    let rect = Rect::from_min_size(at, galley.size());
    p.galley(at, galley, theme.muted);
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(done: f32, seconds_left: Option<f32>) -> Loading {
        Loading {
            what: LoadKind::Sound,
            state: LoadState::Reading { done, seconds_left },
        }
    }

    #[test]
    fn a_header_has_a_shorter_line_for_narrow_room() {
        let [long, short] = reading(0.43, None).header_texts().unwrap();
        assert_eq!((long.as_str(), short.as_str()), ("Reading 43 %", "43 %"));
        let waiting = Loading {
            what: LoadKind::Sound,
            state: LoadState::Waiting,
        };
        assert_eq!(waiting.header_texts().unwrap()[1], "Waiting…");
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter();
            let header = Rect::from_min_size(pos2(0.0, 0.0), vec2(140.0, 58.0));
            let wide = draw_in_header(p, &Theme::ASH, header, 400.0, &waiting).unwrap();
            // Just too narrow for the whole line: the short one, whole.
            let room = wide.width() - 1.0;
            let narrow = draw_in_header(p, &Theme::ASH, header, room, &waiting).unwrap();
            assert!(
                narrow.width() < wide.width() && narrow.width() <= room,
                "never cut mid-word"
            );
            assert!(draw_in_header(p, &Theme::ASH, header, 4.0, &waiting).is_none());
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn percent_is_whole_and_never_says_done_early() {
        assert_eq!(percent(0.0), "0 %");
        assert_eq!(percent(0.437), "43 %");
        assert_eq!(percent(0.999), "99 %");
        assert_eq!(percent(1.0), "99 %", "done is when the bar goes away");
        assert_eq!(percent(-3.0), "0 %");
        assert_eq!(percent(f32::NAN), "0 %");
    }

    #[test]
    fn a_clip_says_as_much_as_fits() {
        let l = reading(0.43, Some(21.0));
        assert_eq!(
            l.clip_text(400.0).unwrap(),
            "Reading sound 43 % · about 20 s left"
        );
        assert_eq!(l.clip_text(200.0).unwrap(), "Reading sound 43 %");
        assert_eq!(l.clip_text(60.0).unwrap(), "43 %");
        assert_eq!(l.clip_text(20.0), None, "the bar alone");
        // No time left until it can be told steadily.
        assert_eq!(reading(0.02, None).clip_text(400.0).unwrap(), "Reading sound 2 %");
    }

    #[test]
    fn waiting_and_failing_are_said_plainly() {
        let waiting = Loading {
            what: LoadKind::Pictures,
            state: LoadState::Waiting,
        };
        assert_eq!(waiting.clip_text(300.0).unwrap(), "Waiting to read pictures");
        assert_eq!(waiting.clip_text(100.0).unwrap(), "Waiting");
        assert_eq!(waiting.header_text().unwrap(), "Waiting to read");
        assert_eq!(waiting.done(), 0.0);
        let failed = |what| Loading {
            what,
            state: LoadState::Failed,
        };
        assert_eq!(failed(LoadKind::Sound).clip_text(300.0).unwrap(), "no waveform");
        assert_eq!(
            failed(LoadKind::Pictures).clip_text(300.0).unwrap(),
            "no pictures"
        );
        assert_eq!(
            failed(LoadKind::Sound).header_text(),
            None,
            "nothing is going on in the track"
        );
    }

    #[test]
    fn a_track_says_how_far_and_its_tooltip_the_rest() {
        let l = reading(0.43, Some(130.0));
        assert_eq!(l.header_text().unwrap(), "Reading 43 %");
        let told = l.explanation();
        assert!(told.contains("43 % done, about 2 min left"), "{told}");
        assert!(reading(0.43, None).explanation().contains("43 % done."));
        assert_eq!(l.done(), 0.43);
        assert_eq!(reading(7.0, None).done(), 1.0);
    }
}
