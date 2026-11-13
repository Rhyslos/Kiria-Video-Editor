//! The system's file chooser, and saying so when there is none.
//!
//! Every "Browse…", "Open…", "Import…" and "Find them…" asks the system's
//! own file chooser (on its own thread: it blocks). On Linux that is the
//! desktop's file chooser service (xdg-desktop-portal), with zenity as the
//! fallback; a system with neither opens nothing at all, and the person
//! would only see a button that does nothing. [`run`] wraps the asking: when
//! no chooser can open it leaves a note in the window's context, and the
//! window turns [`take_problem`] into an explained error in its error strip.
//!
//! There is no typed path field to fall back on: on Windows (the first
//! platform) the chooser is always there, on Linux the fix is one package,
//! and the editor also takes files dropped on its window.

use std::time::{Duration, Instant};

/// A chooser that answers "nothing picked" faster than this never opened:
/// nobody closes a file chooser that quickly.
const TOO_QUICK: Duration = Duration::from_millis(300);

fn problem_id() -> egui::Id {
    egui::Id::unique("file-dialog-problem")
}

/// Asks the system's file chooser with `ask` (call it on a thread of its
/// own). When no chooser could open, a note is left for [`take_problem`]
/// and nothing is picked.
pub fn run<T>(ctx: &egui::Context, ask: impl FnOnce() -> Option<T>) -> Option<T> {
    let system = System::now();
    if !system.can_open() {
        report(ctx);
        return None;
    }
    let started = Instant::now();
    let picked = ask();
    if system.never_opened(picked.is_none(), started.elapsed()) {
        report(ctx);
    }
    picked
}

fn report(ctx: &egui::Context) {
    tracing::warn!("no file chooser could open");
    ctx.data_mut(|d| d.insert_temp(problem_id(), true));
    ctx.request_repaint();
}

/// The explained error once a file chooser could not open (asked every
/// frame by the window; given once).
pub fn take_problem(ctx: &egui::Context) -> Option<diag::AppError> {
    let failed = ctx.data_mut(|d| d.remove_temp::<bool>(problem_id()))?;
    failed.then(no_chooser)
}

/// What is said when no file chooser could open.
pub fn no_chooser() -> diag::AppError {
    diag::AppError::new("No file chooser could open")
        .why(
            "This system has neither the desktop's file chooser service (xdg-desktop-portal) nor zenity, which are used to pick files and folders.",
        )
        .fix(
            "Install zenity (or your desktop's xdg-desktop-portal package), then try again. Files can also be dropped on the editor's window to import them.",
        )
}

/// What the system offers for choosing files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct System {
    /// zenity is on the path (the fallback chooser).
    zenity: bool,
    /// A session message bus is there to reach the desktop's chooser
    /// service through.
    bus: bool,
}

impl System {
    #[cfg(target_os = "linux")]
    fn now() -> System {
        let bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|a| !a.is_empty());
        let zenity = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("zenity").is_file()));
        System { zenity, bus }
    }

    /// Elsewhere the system's own chooser is always there.
    #[cfg(not(target_os = "linux"))]
    fn now() -> System {
        System {
            zenity: true,
            bus: true,
        }
    }

    /// Whether a chooser can be asked at all: zenity, or a bus to reach
    /// the desktop's service through.
    fn can_open(self) -> bool {
        self.zenity || self.bus
    }

    /// Whether nothing was picked because nothing opened: the answer came
    /// too quickly, and without zenity there was nothing to fall back on.
    fn never_opened(self, nothing: bool, took: Duration) -> bool {
        nothing && took < TOO_QUICK && !self.zenity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_without_a_chooser_is_told_apart() {
        let none = System {
            zenity: false,
            bus: false,
        };
        assert!(!none.can_open());
        let portal = System {
            zenity: false,
            bus: true,
        };
        assert!(portal.can_open(), "the desktop's service may answer");
        assert!(
            portal.never_opened(true, Duration::from_millis(20)),
            "nothing, at once: the service wasn't there"
        );
        assert!(
            !portal.never_opened(true, Duration::from_secs(4)),
            "the person cancelled"
        );
        assert!(
            !portal.never_opened(false, Duration::from_millis(20)),
            "something was picked"
        );
        let zenity = System {
            zenity: true,
            bus: false,
        };
        assert!(zenity.can_open());
        assert!(!zenity.never_opened(true, Duration::from_millis(20)));
    }

    #[test]
    fn the_problem_is_explained_once() {
        let ctx = egui::Context::default();
        assert!(take_problem(&ctx).is_none());
        report(&ctx);
        let err = take_problem(&ctx).unwrap();
        assert_eq!(err.what(), "No file chooser could open");
        assert!(err.why_text().is_some_and(|w| w.contains("zenity")));
        assert!(err.fix_text().contains("Install zenity"));
        assert!(take_problem(&ctx).is_none(), "said once");
    }
}
