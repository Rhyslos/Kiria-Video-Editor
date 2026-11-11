//! The guard around decoding video on the window's device.
//!
//! A graphics driver can fail there without reporting anything: a crash or
//! a freeze the first time it is asked to decode, or to hand a picture
//! over. Like a start with Vulkan, that is caught with a marker file: it is
//! written before the device is first asked to decode in a session, and
//! removed once a few pictures decoded there have been drawn. Setting the
//! device up for the decoder at start is guarded the same way, for as long
//! as it takes. A marker found at the next start means the session ended in
//! between ([`plan`](crate::plan) then switches this way of decoding off
//! and the window asks what to do).
//!
//! Errors the device reports are not this guard's business: the file is
//! simply decoded another way. The marker is removed then, and written
//! again if the device is tried for another file.
//!
//! Closing the window normally ends the guard for good, marker removed:
//! decoding that is still under way on its own thread at that moment writes
//! nothing more, so a start is never misled by a session that closed
//! properly. The exception is a device that was asked for something and has
//! not answered for [`STUCK_AFTER`]: the decoding thread is frozen in the
//! driver while the window still works, and the marker stays (or is
//! written) so that the next start treats it like a crash.

use crate::StartMarker;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Pictures decoded on the device and drawn before it counts as working.
pub const PICTURES_BEFORE_TRUSTED: u32 = 10;

/// A device that has not answered for this long when the window closes
/// counts as frozen. Decoding one picture takes milliseconds, a far jump
/// well under a second.
pub const STUCK_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing asked of the device at the moment: no marker.
    Idle,
    /// The marker is written; this many pictures have been drawn since.
    Guarding { drawn: u32 },
    /// It works: no marker, for the rest of the session.
    Trusted,
    /// The window closed normally: nothing is written any more.
    Closed,
}

#[derive(Debug)]
struct Watch {
    state: State,
    /// When the device was asked for something it has not answered yet.
    asked: Option<Instant>,
}

/// One session's guard. The decoding thread reports what it is about to do
/// and how it went; the window reports pictures drawn.
#[derive(Debug)]
pub struct DecodeGuard {
    /// `None` when nothing is to be written.
    marker: Option<StartMarker>,
    watch: Mutex<Watch>,
}

impl DecodeGuard {
    /// `marker` is `None` for a program that decodes no video and in an
    /// automated run, which write nothing.
    pub(crate) fn new(marker: Option<StartMarker>) -> DecodeGuard {
        DecodeGuard {
            marker,
            watch: Mutex::new(Watch {
                state: State::Idle,
                asked: None,
            }),
        }
    }

    /// Changes what is watched, and writes or removes the marker when the
    /// state moves into or out of guarding.
    fn change(&self, next: impl FnOnce(&mut Watch)) {
        let Ok(mut watch) = self.watch.lock() else {
            return;
        };
        let before = watch.state;
        next(&mut watch);
        let guarding = |s: State| matches!(s, State::Guarding { .. });
        match (guarding(before), guarding(watch.state)) {
            (false, true) => self.write(),
            (true, false) => self.clear(),
            _ => {}
        }
    }

    fn write(&self) {
        if let Some(Err(err)) = self.marker.as_ref().map(StartMarker::write) {
            tracing::warn!("{}", err.report());
        }
    }

    fn clear(&self) {
        if let Some(Err(err)) = self.marker.as_ref().map(StartMarker::clear) {
            tracing::warn!("{}", err.report());
        }
    }

    /// Call before the device is asked for anything: opening a file on it,
    /// or decoding a picture. Then call [`DecodeGuard::answered`] or
    /// [`DecodeGuard::reported_an_error`] when it comes back.
    pub fn about_to_decode(&self) {
        self.change(|watch| {
            if watch.state == State::Closed {
                return;
            }
            watch.asked = Some(Instant::now());
            if watch.state == State::Idle {
                watch.state = State::Guarding { drawn: 0 };
            }
        });
    }

    /// The device did what it was asked.
    pub fn answered(&self) {
        self.change(|watch| watch.asked = None);
    }

    /// The device reported an error, which the caller handles: no crash.
    pub fn reported_an_error(&self) {
        self.stand_down();
    }

    /// Setting the device up for the decoder is over, whichever way it
    /// went: call [`DecodeGuard::about_to_decode`] before it and this after.
    /// A session that shows no video then leaves no marker behind.
    pub fn set_up(&self) {
        self.stand_down();
    }

    fn stand_down(&self) {
        self.change(|watch| {
            watch.asked = None;
            if matches!(watch.state, State::Guarding { .. }) {
                watch.state = State::Idle;
            }
        });
    }

    /// A picture decoded on the device has been drawn.
    pub fn picture_drawn(&self) {
        self.change(|watch| {
            watch.state = match watch.state {
                State::Guarding { drawn } if drawn + 1 >= PICTURES_BEFORE_TRUSTED => State::Trusted,
                State::Guarding { drawn } => State::Guarding { drawn: drawn + 1 },
                other => other,
            };
        });
    }

    /// The window closed normally. Nothing is written after this, whatever
    /// the decoding thread still reports.
    ///
    /// One case leaves the marker behind, or writes it: the device was
    /// asked for something [`STUCK_AFTER`] ago or longer and has not
    /// answered. The window still worked, so it could be closed, but the
    /// decoding thread is frozen in the driver: the next start treats that
    /// like a crash.
    pub(crate) fn close(&self) {
        self.close_at(Instant::now());
    }

    fn close_at(&self, now: Instant) {
        let Ok(mut watch) = self.watch.lock() else {
            return;
        };
        let was = std::mem::replace(&mut watch.state, State::Closed);
        let stuck = watch
            .asked
            .is_some_and(|since| now.saturating_duration_since(since) >= STUCK_AFTER);
        if was == State::Closed {
            return;
        }
        if stuck {
            tracing::warn!(
                "the window's device has not answered the decoder for {} seconds: counted as frozen",
                STUCK_AFTER.as_secs()
            );
            self.write();
        } else if matches!(was, State::Guarding { .. }) {
            self.clear();
        }
    }

    /// Whether enough pictures have been drawn for this session.
    pub fn is_trusted(&self) -> bool {
        self.watch.lock().is_ok_and(|watch| watch.state == State::Trusted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Program;

    fn guard(dir: &std::path::Path) -> (DecodeGuard, StartMarker) {
        let marker = StartMarker::for_decoding(Program::Editor, dir);
        (DecodeGuard::new(Some(marker.clone())), marker)
    }

    #[test]
    fn the_marker_is_there_from_the_first_decode_until_pictures_have_been_drawn() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        assert!(!marker.is_present(), "nothing is written until video is decoded");
        guard.about_to_decode();
        assert!(marker.is_present());
        for _ in 0..PICTURES_BEFORE_TRUSTED - 1 {
            guard.about_to_decode();
            guard.picture_drawn();
            assert!(marker.is_present() && !guard.is_trusted());
        }
        guard.picture_drawn();
        assert!(!marker.is_present() && guard.is_trusted());
        // Trusted for the rest of the session: nothing is written again.
        guard.about_to_decode();
        guard.reported_an_error();
        assert!(!marker.is_present() && guard.is_trusted());
    }

    #[test]
    fn a_reported_error_is_no_crash_and_the_next_try_is_guarded_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        guard.about_to_decode();
        guard.picture_drawn();
        guard.reported_an_error();
        assert!(
            !marker.is_present(),
            "the file went another way: nothing to remember"
        );
        guard.picture_drawn();
        assert!(
            !marker.is_present(),
            "a picture drawn from another file's way counts for nothing"
        );
        // Another file is tried on the device: guarded from the start.
        guard.about_to_decode();
        assert!(marker.is_present());
        for _ in 0..PICTURES_BEFORE_TRUSTED - 1 {
            guard.picture_drawn();
        }
        assert!(marker.is_present(), "the pictures before the error don't count");
        guard.picture_drawn();
        assert!(!marker.is_present() && guard.is_trusted());
    }

    #[test]
    fn setting_the_device_up_is_guarded_and_leaves_nothing_behind_once_over() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        guard.about_to_decode();
        assert!(marker.is_present(), "a crash while setting up is caught");
        guard.set_up();
        assert!(!marker.is_present(), "a session without video blames nothing");
        // The first video is guarded from its start.
        guard.about_to_decode();
        assert!(marker.is_present() && !guard.is_trusted());
    }

    #[test]
    fn a_session_that_ends_in_between_leaves_the_marker_for_the_next_start() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        guard.about_to_decode();
        guard.picture_drawn();
        drop(guard); // the driver took the program down
        assert!(marker.is_present());
    }

    #[test]
    fn closing_normally_removes_the_marker_and_nothing_writes_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        guard.about_to_decode();
        guard.picture_drawn();
        guard.close();
        assert!(!marker.is_present());
        // The decoding thread is still finishing a picture.
        guard.about_to_decode();
        guard.reported_an_error();
        guard.about_to_decode();
        guard.picture_drawn();
        assert!(!marker.is_present() && !guard.is_trusted());
    }

    #[test]
    fn a_device_that_stopped_answering_is_remembered_even_though_the_window_closed() {
        let later = |seconds| Instant::now() + Duration::from_secs(seconds);
        // Frozen on its first picture: the marker stays.
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = self::guard(tmp.path());
        guard.about_to_decode();
        guard.close_at(later(STUCK_AFTER.as_secs() + 1));
        assert!(marker.is_present());

        // Frozen after it had counted as working: the marker is written.
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = self::guard(tmp.path());
        for _ in 0..PICTURES_BEFORE_TRUSTED {
            guard.about_to_decode();
            guard.answered();
            guard.picture_drawn();
        }
        assert!(guard.is_trusted() && !marker.is_present());
        guard.about_to_decode();
        guard.close_at(later(STUCK_AFTER.as_secs() + 1));
        assert!(marker.is_present());

        // Closed while a picture was being decoded, a moment after asking:
        // nothing is frozen.
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = self::guard(tmp.path());
        guard.about_to_decode();
        guard.close_at(later(1));
        assert!(!marker.is_present());

        // Answered long ago, or turned down with an error: nothing is frozen.
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = self::guard(tmp.path());
        guard.about_to_decode();
        guard.answered();
        guard.about_to_decode();
        guard.reported_an_error();
        guard.close_at(later(3600));
        assert!(!marker.is_present());
    }

    #[test]
    fn closing_leaves_another_copys_marker_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard, marker) = guard(tmp.path());
        // Another copy of the editor is in the middle of its first pictures;
        // this one never decoded on the device.
        marker.write().unwrap();
        guard.close();
        assert!(marker.is_present());
    }

    #[test]
    fn an_automated_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = DecodeGuard::new(None);
        guard.about_to_decode();
        for _ in 0..PICTURES_BEFORE_TRUSTED {
            guard.picture_drawn();
        }
        assert!(guard.is_trusted());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
