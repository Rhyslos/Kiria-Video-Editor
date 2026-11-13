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
}
