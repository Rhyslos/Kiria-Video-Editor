//! The status line's age: a message stays for `SHOWN` and then fades out
//! over `FADING`, so an old one is never read as news. A new message (any
//! change to the text) starts again at full strength.

use std::time::{Duration, Instant};

/// How long a message stays at full strength.
pub const SHOWN: Duration = Duration::from_secs(6);
/// How long it then takes to fade out.
pub const FADING: Duration = Duration::from_secs(1);

/// The message last seen and since when.
#[derive(Debug, Default)]
pub struct Fading {
    seen: String,
    since: Option<Instant>,
}

impl Fading {
    /// Once a frame, before the line is drawn: notices a new message, clears
    /// `status` once it has faded out, and says how strongly to draw it
    /// (1 full, 0 gone) and when to look again (while it is still showing).
    pub fn follow(&mut self, status: &mut String, now: Instant) -> (f32, Option<Duration>) {
        if *status != self.seen {
            self.seen.clone_from(status);
            self.since = Some(now);
        }
        let Some(since) = self.since.filter(|_| !status.is_empty()) else {
            return (1.0, None);
        };
        let age = now.saturating_duration_since(since);
        if age >= SHOWN + FADING {
            status.clear();
            self.seen.clear();
            return (0.0, None);
        }
        if age < SHOWN {
            return (1.0, Some(SHOWN - age));
        }
        let left = (SHOWN + FADING - age).as_secs_f32() / FADING.as_secs_f32();
        (left.clamp(0.0, 1.0), Some(Duration::from_millis(30)))
    }
}

/// Marks the message the status line holds as being about the timeline
/// (the lines tool's hint), so a page without a timeline drops it. The mark
/// is set beside the message when it is said and holds only while the line
/// still shows that message: any later message is not about the timeline
/// unless it is marked too.
#[derive(Debug, Default)]
pub struct AboutTimeline {
    marked: Option<String>,
}

impl AboutTimeline {
    /// Says `message` on the status line, marked as about the timeline.
    pub fn say(&mut self, status: &mut String, message: &str) {
        message.clone_into(status);
        self.marked = Some(message.to_owned());
    }

    /// On a page without a timeline: clears the status line when it still
    /// shows the marked message.
    pub fn leave_timeline(&mut self, status: &mut String) {
        if let Some(marked) = self.marked.take()
            && *status == marked
        {
            status.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_fades_out_and_a_new_one_starts_again() {
        let start = Instant::now();
        let mut fading = Fading::default();
        let mut status = "Pasted 2 clips".to_owned();
        assert_eq!(fading.follow(&mut status, start).0, 1.0);
        let (half, again) = fading.follow(&mut status, start + SHOWN + FADING / 2);
        assert!(half > 0.3 && half < 0.7, "{half}");
        assert!(again.is_some());
        // A new message is at full strength, however old the last one was.
        status = "Undid: removed 1 clip".to_owned();
        let later = start + SHOWN + FADING / 2;
        assert_eq!(fading.follow(&mut status, later).0, 1.0);
        fading.follow(&mut status, later + SHOWN + FADING);
        assert!(status.is_empty(), "gone once faded out");
    }

    #[test]
    fn only_a_message_marked_about_the_timeline_goes_with_it() {
        let mut about = AboutTimeline::default();
        let mut status = String::new();
        about.say(&mut status, "Lines off");
        about.leave_timeline(&mut status);
        assert!(status.is_empty(), "the marked message goes");
        // A later message in its place stays, whatever it says.
        about.say(&mut status, "Lines on");
        status = "Imported 3 files".to_owned();
        about.leave_timeline(&mut status);
        assert_eq!(status, "Imported 3 files");
        // And the mark is not kept for later.
        status = "Lines on".to_owned();
        about.leave_timeline(&mut status);
        assert_eq!(status, "Lines on");
    }
}
