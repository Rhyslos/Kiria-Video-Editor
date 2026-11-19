//! Graphics errors on the window's device that nothing else catches.
//!
//! The graphics library ends the thread that made such an error unless it
//! is handed a handler; [`watch`] hands it one that writes the error to the
//! log instead. A broken pipeline or a lost device tends to give several
//! different errors every frame, so each distinct text is written once
//! (up to [`KEPT`] of them), and the repeats are counted and summed up in
//! one line at most every [`SUMMARY_EVERY`]. The first error is also told
//! to the person, once, through the window's error list.
//!
//! The handler may run on any thread: it holds a mutex only to count, and
//! writes to the log and the channel after letting it go.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diag::AppError;
use eframe::wgpu;

/// How many distinct texts are remembered (and written out) at most.
pub const KEPT: usize = 32;
/// Repeats are summed up in the log at most this often.
pub const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// Writes graphics errors on `device` to the log instead of ending the
/// thread that made them. The receiver gets one error for the window's
/// error list, the first time any error comes; poll it every frame.
pub fn watch(device: &wgpu::Device) -> Receiver<AppError> {
    let (tell, told) = std::sync::mpsc::channel();
    let tally = Mutex::new(Tally::default());
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let text = error.to_string();
        let noted = match tally.lock() {
            Ok(mut tally) => tally.note(&text, Instant::now()),
            Err(poisoned) => poisoned.into_inner().note(&text, Instant::now()),
        };
        handle(noted, &text, &tell);
    }));
    told
}

/// Writes what [`Tally::note`] said to write, and tells the person when due.
fn handle(noted: Noted, text: &str, tell: &Sender<AppError>) {
    for line in &noted.log {
        tracing::error!("{line}");
    }
    if noted.tell {
        // The window may be gone already; there is nobody left to tell.
        let _ = tell.send(for_the_person(text));
    }
}

/// The error the person sees, once a session.
fn for_the_person(text: &str) -> AppError {
    AppError::new("The graphics card reported an error")
        .why(format!(
            "Drawing on the graphics card went wrong, so the picture may look wrong: {text}"
        ))
        .fix(
            "Restart the app. If it happens again, try another graphics system in \
             Settings › Graphics, and send the log (Settings › Diagnostics) with a report.",
        )
}

/// What to do about one error.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Noted {
    /// Lines to write to the log.
    pub log: Vec<String>,
    /// Tell the person (the first error of the session).
    pub tell: bool,
}

/// The errors seen so far: which texts were written out, and how many came
/// again since the last summary.
#[derive(Debug, Default)]
pub struct Tally {
    /// The distinct texts written out, at most [`KEPT`].
    seen: Vec<String>,
    /// Errors since the last summary that weren't written out: repeats of
    /// a text in `seen`, or new texts once `seen` is full.
    unwritten: u64,
    /// When the count in `unwritten` started; `None` before any error.
    since: Option<Instant>,
}

impl Tally {
    /// Takes note of an error with `text` at `now`.
    pub fn note(&mut self, text: &str, now: Instant) -> Noted {
        let first = self.since.is_none();
        let since = *self.since.get_or_insert(now);
        let mut log = Vec::new();
        if !self.seen.iter().any(|seen| seen == text) && self.seen.len() < KEPT {
            self.seen.push(text.to_owned());
            log.push(format!("graphics error: {text}"));
        } else {
            self.unwritten += 1;
        }
        let waited = now.saturating_duration_since(since);
        if self.unwritten > 0 && waited >= SUMMARY_EVERY {
            log.push(format!(
                "graphics errors: {} more not written out in the last {} s \
                 (repeats, or more than {KEPT} different ones)",
                self.unwritten,
                waited.as_secs()
            ));
            self.unwritten = 0;
            self.since = Some(now);
        }
        Noted { log, tell: first }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logged(tally: &mut Tally, text: &str, at: Instant) -> usize {
        tally.note(text, at).log.len()
    }

    #[test]
    fn alternating_errors_are_written_once_each() {
        let mut tally = Tally::default();
        let start = Instant::now();
        let mut lines = 0;
        for frame in 0..600u64 {
            let at = start + Duration::from_millis(frame * 16);
            lines += logged(&mut tally, "A", at);
            lines += logged(&mut tally, "B", at);
        }
        assert_eq!(lines, 2, "A and B once each, nothing more within a minute");
    }

    #[test]
    fn repeats_are_summed_up_at_most_once_a_minute() {
        let mut tally = Tally::default();
        let start = Instant::now();
        assert_eq!(logged(&mut tally, "A", start), 1);
        let mut summaries = Vec::new();
        // Five minutes at 60 frames a second.
        for frame in 1..=(5 * 60 * 60u64) {
            let at = start + Duration::from_micros(frame * 16_667);
            let noted = tally.note("A", at);
            summaries.extend(noted.log);
        }
        assert_eq!(summaries.len(), 5, "{summaries:?}");
        assert!(summaries[0].contains("3600 more"), "{}", summaries[0]);
    }

    #[test]
    fn distinct_texts_are_capped() {
        let mut tally = Tally::default();
        let at = Instant::now();
        let lines: usize = (0..100)
            .map(|i| logged(&mut tally, &format!("error {i}"), at))
            .sum();
        assert_eq!(lines, KEPT);
        let later = at + SUMMARY_EVERY;
        let noted = tally.note("error 99", later);
        assert_eq!(noted.log.len(), 1);
        assert!(
            noted.log[0].contains(&format!("{} more", 100 - KEPT + 1)),
            "{:?}",
            noted.log
        );
    }

    #[test]
    fn the_person_is_told_once() {
        let mut tally = Tally::default();
        let at = Instant::now();
        assert!(tally.note("A", at).tell);
        assert!(!tally.note("B", at).tell);
        assert!(!tally.note("A", at + SUMMARY_EVERY).tell);
    }

    #[test]
    fn the_handler_sends_one_error_for_the_list() {
        let (tell, told) = std::sync::mpsc::channel();
        let mut tally = Tally::default();
        let at = Instant::now();
        for text in ["A", "B", "A"] {
            handle(tally.note(text, at), text, &tell);
        }
        let errors: Vec<AppError> = told.try_iter().collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].why_text().is_some_and(|why| why.contains('A')));
    }
}
