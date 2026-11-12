//! Small questions to the operating system that the interface needs and
//! the windowing crate does not answer: today, which keyboard layout is in
//! use.
//!
//! This is the one crate besides the FFmpeg and GPU wrappers where `unsafe`
//! may appear (working rule 6), and only for foreign calls into the system,
//! each with its reason. Everything here is plain: it says what the system
//! reports in the system's own terms, and the interface decides what that
//! means (`ui_kit::keyboard::SystemKeyboard`).

mod xkb;

#[cfg(windows)]
mod windows;
#[cfg(not(windows))]
mod x11;

use std::time::Duration;

/// The keyboard layout the system reports, in its own terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemLayout {
    /// Windows: the language identifier of the active keyboard layout (the
    /// low word of its handle), such as 0x0414 for Norwegian.
    WindowsLanguage(u16),
    /// Linux: the XKB name of the active layout, without its variant, such
    /// as "no" or "us".
    Xkb(String),
}

/// How often the system is asked while a watch runs. A layout switch shows
/// within this long.
pub const ASK_EVERY: Duration = Duration::from_millis(500);

/// The keyboard layout in use now, asked once. It may wait on the system
/// (on Linux a round trip to the X server, or reading a file), so it is for
/// the command line; a window uses [`KeyboardWatch`].
pub fn keyboard_layout_now() -> Option<SystemLayout> {
    #[cfg(windows)]
    {
        windows::active_layout()
    }
    #[cfg(not(windows))]
    {
        x11::Asker::connect().map_or_else(xkb::from_settings, |mut asker| asker.ask())
    }
}

/// Follows the keyboard layout while a window is open. [`poll`] never
/// waits, so it can run every frame: on Windows it asks the system at most
/// every [`ASK_EVERY`] (one cheap call that reads the calling thread's
/// layout, so call it from the window's thread); on Linux a thread of its
/// own asks the X server that often and `poll` only picks up its answers.
///
/// [`poll`]: KeyboardWatch::poll
#[derive(Debug)]
pub struct KeyboardWatch {
    inner: Watch,
    layout: Option<SystemLayout>,
}

#[cfg(windows)]
type Watch = windows::Watch;
#[cfg(not(windows))]
type Watch = x11::Watch;

impl KeyboardWatch {
    /// Starts watching. Until the first answer the layout is unknown.
    pub fn start() -> KeyboardWatch {
        KeyboardWatch {
            inner: Watch::start(),
            layout: None,
        }
    }

    /// Takes in what the system said since the last call; true when the
    /// layout changed.
    pub fn poll(&mut self) -> bool {
        match self.inner.poll() {
            Some(now) if now != self.layout => {
                tracing::info!(layout = ?now, "keyboard layout");
                self.layout = now;
                true
            }
            _ => false,
        }
    }

    /// The layout as last reported; `None` before the first answer or when
    /// the system can't say.
    pub fn layout(&self) -> Option<&SystemLayout> {
        self.layout.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_the_host_answers_or_says_nothing_without_crashing() {
        // Whatever the machine running the tests has (a desktop, a virtual
        // screen, nothing at all), the answer is a layout or nothing.
        match keyboard_layout_now() {
            Some(SystemLayout::Xkb(name)) => assert!(!name.is_empty() && !name.contains(',')),
            Some(SystemLayout::WindowsLanguage(language)) => assert_ne!(language, 0),
            None => {}
        }
    }

    #[test]
    fn a_watch_settles_on_what_asking_once_says() {
        let mut watch = KeyboardWatch::start();
        let once = keyboard_layout_now();
        // The first answer comes within a moment; nothing changes after it.
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while watch.layout() != once.as_ref() && std::time::Instant::now() < until {
            watch.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(watch.layout(), once.as_ref());
        assert!(!watch.poll());
    }
}
