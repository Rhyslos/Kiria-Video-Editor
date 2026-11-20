//! Notices hitches while the timeline plays, and says where the time went.
//!
//! A hitch someone sees on their own computer can have several causes: the
//! interface thread working too long on a frame, the window toolkit taking
//! long to draw a frame on screen (the graphics card or its driver busy),
//! or the picture alone coming late (decoding falling behind) while the
//! window goes on drawing. Each needs a different fix, and they look alike
//! from the outside, so while the timeline plays every frame is timed in
//! those parts, and a gap between frames well over what the screen needs
//! is written to the log with its parts and the last picture's decoding.
//! When playback stops, one line sums the play up, in the log and in
//! Diagnostics ("Last playback").
//!
//! The work per frame is a few comparisons and additions; the log is
//! written at most once a second while playing.

use std::time::{Duration, Instant};

use project::Time;

/// A gap between frames from here counts as a hitch: three frames of a
/// 60 Hz screen, plainly seen as a stop in movement.
pub const HITCH: Duration = Duration::from_millis(50);

/// The least time between two hitch lines in the log; those in between
/// are counted and the count written with the next line.
const LOG_EVERY: Duration = Duration::from_secs(1);

/// The name of the Diagnostics line.
pub const FACT: &str = "Last playback";

/// The last picture that came to the viewer, for a hitch's line.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrived {
    /// How long it took to decode and hand over, with where that went
    /// (see `slow_pictures::describe`).
    pub described: String,
    pub took: Duration,
}

/// What happened in one frame of the window.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The timeline is playing.
    pub playing: bool,
    /// The playhead, for naming where a hitch was.
    pub at: Time,
    /// A picture (not a first one standing in) came to the viewer.
    pub picture: Option<Arrived>,
}

/// What [`Watch::frame_ended`] has to say: lines for the log, and the
/// summary for Diagnostics when a play ended.
#[derive(Debug, Default, PartialEq)]
pub struct Said {
    pub log: Vec<String>,
    pub summary: Option<String>,
}

/// One play, from start to stop.
#[derive(Debug)]
struct Play {
    started: Instant,
    from: Time,
    frames: u32,
    pictures: u32,
    hitches: u32,
    longest_gap: Duration,
    longest_work: Duration,
    longest_picture: Duration,
    /// Hitches since the last line in the log, not written yet.
    unwritten: u32,
    last_written: Option<Instant>,
    last_picture: Option<Arrived>,
}

impl Play {
    fn new(started: Instant, from: Time) -> Self {
        Self {
            started,
            from,
            frames: 0,
            pictures: 0,
            hitches: 0,
            longest_gap: Duration::ZERO,
            longest_work: Duration::ZERO,
            longest_picture: Duration::ZERO,
            unwritten: 0,
            last_written: None,
            last_picture: None,
        }
    }
}

/// Times frames while playing.
#[derive(Debug, Default)]
pub struct Watch {
    /// When the last frame's work began and ended.
    last_start: Option<Instant>,
    last_end: Option<Instant>,
    this_start: Option<Instant>,
    play: Option<Play>,
}

impl Watch {
    /// The interface thread begins a frame.
    pub fn frame_started(&mut self, now: Instant) {
        self.last_start = self.this_start.replace(now);
    }

    /// The interface thread is done with a frame: says what is worth
    /// writing down.
    pub fn frame_ended(&mut self, now: Instant, frame: Frame) -> Said {
        let mut said = Said::default();
        let start = self.this_start.unwrap_or(now);
        let work = now.saturating_duration_since(start);
        // From the start of the last frame to the start of this one, and
        // the part of it that was not the interface's own work: the window
        // drawing the last frame on screen, and its events.
        let gap = self.last_start.map(|last| start.saturating_duration_since(last));
        let between = self.last_end.map(|end| start.saturating_duration_since(end));
        self.last_end = Some(now);

        if !frame.playing {
            if let Some(play) = self.play.take() {
                let summary = summary(&play, now, frame.at);
                said.log.push(format!("Playback: {summary}"));
                said.summary = Some(summary);
            }
            return said;
        }
        let play = self.play.get_or_insert_with(|| Play::new(start, frame.at));
        play.frames += 1;
        play.longest_work = play.longest_work.max(work);
        if let Some(picture) = frame.picture {
            play.pictures += 1;
            play.longest_picture = play.longest_picture.max(picture.took);
            play.last_picture = Some(picture);
        }
        // The first frame's gap is from before playing began.
        let Some(gap) = gap.filter(|_| play.frames > 1) else {
            return said;
        };
        play.longest_gap = play.longest_gap.max(gap);
        if gap < HITCH {
            return said;
        }
        play.hitches += 1;
        play.unwritten += 1;
        let due = play
            .last_written
            .is_none_or(|last| now.saturating_duration_since(last) >= LOG_EVERY);
        if due {
            let others = play.unwritten - 1;
            play.unwritten = 0;
            play.last_written = Some(now);
            // The work of the frame before the gap is what held the next one up.
            let previous_work = gap.saturating_sub(between.unwrap_or(Duration::ZERO));
            let picture = play
                .last_picture
                .as_ref()
                .map_or_else(|| "no picture yet".to_owned(), |p| p.described.clone());
            let mut line = format!(
                "Playback hitch at {}: {} between frames ({} working out the frame, {} drawing it on screen and the window's events); the last picture: {picture}",
                at_words(frame.at),
                ms(gap),
                ms(previous_work),
                ms(between.unwrap_or(Duration::ZERO)),
            );
            if others > 0 {
                line.push_str(&format!(" ({others} more since the last line)"));
            }
            said.log.push(line);
        }
        said
    }
}

/// One play in a line: how long, how smooth, and what held it up most.
fn summary(play: &Play, now: Instant, to: Time) -> String {
    let lasted = now.saturating_duration_since(play.started);
    let seconds = lasted.as_secs_f64().max(0.001);
    format!(
        "{} to {} over {:.1} s: {} frames drawn ({:.0} a second), {} pictures shown ({:.0} a second), {} {} of {} ms or more, the longest gap {}, the longest frame's work {}, the slowest picture {}",
        at_words(play.from),
        at_words(to),
        lasted.as_secs_f64(),
        play.frames,
        f64::from(play.frames) / seconds,
        play.pictures,
        f64::from(play.pictures) / seconds,
        play.hitches,
        if play.hitches == 1 { "hitch" } else { "hitches" },
        HITCH.as_millis(),
        ms(play.longest_gap),
        ms(play.longest_work),
        ms(play.longest_picture),
    )
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// A moment of the timeline as seconds, for the log.
fn at_words(at: Time) -> String {
    format!("{:.2} s", at.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms_(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Runs frames that each start `gap` after the last and take `work`.
    struct Clock {
        now: Instant,
        watch: Watch,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                now: Instant::now(),
                watch: Watch::default(),
            }
        }

        fn frame(&mut self, gap: Duration, work: Duration, frame: Frame) -> Said {
            self.now += gap;
            self.watch.frame_started(self.now);
            self.watch.frame_ended(self.now + work, frame)
        }
    }

    fn playing(seconds: i64) -> Frame {
        Frame {
            playing: true,
            at: Time::seconds(seconds),
            picture: None,
        }
    }

    fn stopped() -> Frame {
        Frame {
            playing: false,
            ..playing(5)
        }
    }

    fn picture(took: u64) -> Option<Arrived> {
        Some(Arrived {
            described: format!("{took} ms, software"),
            took: ms_(took),
        })
    }

    #[test]
    fn smooth_play_writes_only_its_summary() {
        let mut clock = Clock::new();
        for _ in 0..60 {
            let said = clock.frame(ms_(16), ms_(2), playing(1));
            assert!(said.log.is_empty());
        }
        let said = clock.frame(ms_(16), ms_(2), stopped());
        let summary = said.summary.expect("a summary when playback stops");
        assert!(summary.contains("60 frames drawn"), "{summary}");
        assert!(summary.contains("0 hitches"), "{summary}");
        assert_eq!(said.log.len(), 1);
        // Nothing more until the next play.
        assert_eq!(clock.frame(ms_(16), ms_(2), stopped()), Said::default());
    }

    #[test]
    fn a_hitch_names_its_parts_and_the_last_picture() {
        let mut clock = Clock::new();
        clock.frame(ms_(16), ms_(2), playing(1));
        clock.frame(
            ms_(16),
            ms_(2),
            Frame {
                picture: picture(120),
                ..playing(1)
            },
        );
        // A frame whose work took long: the next one starts late.
        clock.frame(ms_(16), ms_(70), playing(2));
        let said = clock.frame(ms_(72), ms_(2), playing(2));
        assert_eq!(said.log.len(), 1, "{said:?}");
        let line = &said.log[0];
        assert!(line.contains("72.0 ms between frames"), "{line}");
        assert!(line.contains("70.0 ms working out the frame"), "{line}");
        assert!(line.contains("2.0 ms drawing it on screen"), "{line}");
        assert!(line.contains("120 ms, software"), "{line}");
    }

    #[test]
    fn a_slow_screen_is_told_apart_from_slow_work() {
        let mut clock = Clock::new();
        clock.frame(ms_(16), ms_(1), playing(1));
        let said = clock.frame(ms_(90), ms_(1), playing(1));
        let line = &said.log[0];
        assert!(line.contains("1.0 ms working out the frame"), "{line}");
        assert!(line.contains("89.0 ms drawing it on screen"), "{line}");
    }

    #[test]
    fn hitches_close_together_share_a_line() {
        let mut clock = Clock::new();
        clock.frame(ms_(16), ms_(1), playing(1));
        let mut lines = 0;
        for _ in 0..5 {
            lines += clock.frame(ms_(100), ms_(1), playing(1)).log.len();
        }
        assert_eq!(lines, 1, "five hitches within half a second");
        // A second later: the next line, counting those left out.
        let said = clock.frame(ms_(600), ms_(1), playing(1));
        assert!(
            said.log[0].contains("(4 more since the last line)"),
            "{:?}",
            said.log
        );
        let summary = clock.frame(ms_(16), ms_(1), stopped()).summary.expect("summary");
        assert!(summary.contains("6 hitches"), "{summary}");
        assert!(summary.contains("the longest gap 600.0 ms"), "{summary}");
    }

    #[test]
    fn the_wait_before_playing_is_no_hitch() {
        let mut clock = Clock::new();
        clock.frame(ms_(16), ms_(1), stopped());
        // Seconds idle, then play starts: the first playing frame's gap is
        // from before it.
        let said = clock.frame(ms_(3000), ms_(1), playing(1));
        assert!(said.log.is_empty());
    }

    #[test]
    fn pictures_are_counted() {
        let mut clock = Clock::new();
        for i in 0..10 {
            let frame = Frame {
                picture: if i % 2 == 0 { picture(10) } else { None },
                ..playing(1)
            };
            clock.frame(ms_(16), ms_(1), frame);
        }
        let summary = clock.frame(ms_(16), ms_(1), stopped()).summary.expect("summary");
        assert!(summary.contains("5 pictures shown"), "{summary}");
        assert!(summary.contains("the slowest picture 10.0 ms"), "{summary}");
    }
}
