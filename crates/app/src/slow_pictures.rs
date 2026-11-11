//! Which pictures took long, and where the time went.
//!
//! Showing a frame should feel instant; when it doesn't, the question is
//! which part was slow: finding the place in the file, the first frame out
//! of the decoder, the frames after it, or handing the picture over. That
//! differs between machines, drives and graphics drivers, so the viewer
//! keeps the last few slow ones in words, for Diagnostics.

use std::collections::VecDeque;
use std::time::Duration;

/// A picture counts as slow from here: more than a click should take.
pub const SLOW: Duration = Duration::from_millis(50);
/// How many are kept.
const KEPT: usize = 8;

/// The name of the Diagnostics line.
pub const FACT: &str = "Pictures that took long";

/// The last few slow pictures.
#[derive(Debug, Default)]
pub struct SlowPictures {
    lines: VecDeque<String>,
    changed: bool,
}

impl SlowPictures {
    /// Takes note of a picture that took `took` in all, decoded by `method`.
    pub fn note(&mut self, took: Duration, method: &str, work: Option<media::Work>) {
        if took < SLOW {
            return;
        }
        if self.lines.len() == KEPT {
            self.lines.pop_front();
        }
        self.lines.push_back(line(took, method, work));
        self.changed = true;
    }

    /// The list, newest last, if it changed since this was last asked.
    pub fn take_changed(&mut self) -> Option<String> {
        std::mem::take(&mut self.changed).then(|| {
            let lines: Vec<&str> = self.lines.iter().map(String::as_str).collect();
            format!("\n{}", lines.join("\n"))
        })
    }
}

fn ms(d: Duration) -> String {
    format!("{} ms", d.as_millis())
}

/// One picture in words, for example: "312 ms, hardware (d3d11va): jumped
/// (4 ms), first frame after 180 ms, 96 frames done after 300 ms, 12 ms to
/// hand over".
fn line(took: Duration, method: &str, work: Option<media::Work>) -> String {
    let Some(work) = work else {
        return format!("{}, {method}", ms(took));
    };
    let how = if work.jumped {
        format!("jumped ({})", ms(work.jump))
    } else {
        "decoded on".to_owned()
    };
    let first = work
        .first_frame
        .map_or_else(String::new, |first| format!(", first frame after {}", ms(first)));
    format!(
        "{}, {method}: {how}{first}, {} frames done after {}, {} to hand over",
        ms(took),
        work.frames,
        ms(work.decoding),
        ms(took.saturating_sub(work.decoding)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_slow_picture_says_where_the_time_went() {
        let work = media::Work {
            jumped: true,
            jump: millis(4),
            first_frame: Some(millis(180)),
            frames: 96,
            decoding: millis(300),
        };
        assert_eq!(
            line(millis(312), "hardware (d3d11va)", Some(work)),
            "312 ms, hardware (d3d11va): jumped (4 ms), first frame after 180 ms, 96 frames done after 300 ms, 12 ms to hand over"
        );
        let on = media::Work {
            jumped: false,
            first_frame: Some(millis(2)),
            frames: 150,
            decoding: millis(140),
            ..work
        };
        assert_eq!(
            line(millis(141), "software", Some(on)),
            "141 ms, software: decoded on, first frame after 2 ms, 150 frames done after 140 ms, 1 ms to hand over"
        );
    }

    #[test]
    fn only_slow_pictures_are_kept_and_only_the_last_few() {
        let mut slow = SlowPictures::default();
        slow.note(millis(3), "software", None);
        assert_eq!(slow.take_changed(), None, "a quick one is not noted");
        for took in 0..10 {
            slow.note(millis(100 + took), "software", None);
        }
        let text = slow.take_changed().unwrap();
        assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), KEPT);
        assert!(text.ends_with("109 ms, software") && !text.contains("101 ms"));
        assert_eq!(slow.take_changed(), None, "unchanged since");
    }
}
