//! The keyboard layout the system uses, followed while the editor runs, as
//! the shortcuts page and the shortcut table read it.

use ui_kit::keyboard::SystemKeyboard;

/// What the page shows for what the system reports.
pub fn from_system(reported: Option<&os::SystemLayout>) -> SystemKeyboard {
    match reported {
        Some(os::SystemLayout::WindowsLanguage(language)) => SystemKeyboard::from_windows_language(*language),
        Some(os::SystemLayout::Xkb(name)) => SystemKeyboard::from_xkb_name(name),
        None => SystemKeyboard::default(),
    }
}

/// The system's keyboard now, asked once (may wait briefly): for the
/// command line, not for the window.
pub fn asked_once() -> SystemKeyboard {
    from_system(os::keyboard_layout_now().as_ref())
}

/// Follows the system's keyboard layout. `poll` is cheap enough for every
/// frame: it never waits on the system (see `os::KeyboardWatch`).
#[derive(Debug)]
pub struct Follower {
    watch: os::KeyboardWatch,
    now: SystemKeyboard,
}

impl Follower {
    pub fn start() -> Follower {
        Follower {
            watch: os::KeyboardWatch::start(),
            now: SystemKeyboard::default(),
        }
    }

    /// Takes in a switch; true when the system's keyboard changed.
    pub fn poll(&mut self) -> bool {
        if !self.watch.poll() {
            return false;
        }
        self.now = from_system(self.watch.layout());
        true
    }

    /// The system's keyboard as last reported.
    pub fn now(&self) -> &SystemKeyboard {
        &self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui_kit::keyboard::LayoutId;

    #[test]
    fn what_each_system_reports_becomes_a_drawn_keyboard() {
        let windows = os::SystemLayout::WindowsLanguage(0x0414);
        assert_eq!(from_system(Some(&windows)).layout, Some(LayoutId::Norwegian));
        let xkb = os::SystemLayout::Xkb("gb".into());
        assert_eq!(from_system(Some(&xkb)).layout, Some(LayoutId::Uk));
        let unknown = os::SystemLayout::Xkb("ru".into());
        assert_eq!(from_system(Some(&unknown)).layout, None);
        assert!(from_system(Some(&unknown)).unknown.is_some());
        assert_eq!(from_system(None), SystemKeyboard::default());
    }

    #[test]
    fn polling_never_waits() {
        let mut follower = Follower::start();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            follower.poll();
        }
        // A thousand frames' worth well inside one frame's budget.
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
    }
}
