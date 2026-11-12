//! Windows: the active keyboard layout, from the calling thread.
//!
//! The one foreign call in this crate. Windows keeps a layout per thread,
//! updated when the person switches while the thread's window has the
//! keyboard, so the window's own thread is the one to ask.
#![allow(unsafe_code)]

use crate::{ASK_EVERY, SystemLayout};
use std::time::Instant;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;

/// The calling thread's active keyboard layout.
pub fn active_layout() -> Option<SystemLayout> {
    // SAFETY: GetKeyboardLayout takes a thread identifier by value (0 means
    // the calling thread), touches no memory of ours and returns a handle
    // that is only read as a number here, never dereferenced or freed.
    let handle = unsafe { GetKeyboardLayout(0) };
    // The handle's low word is the layout's language identifier.
    let language = (handle as usize & 0xFFFF) as u16;
    (language != 0).then_some(SystemLayout::WindowsLanguage(language))
}

/// Asks on the caller's thread, at most every [`ASK_EVERY`].
#[derive(Debug, Default)]
pub struct Watch {
    asked: Option<Instant>,
}

impl Watch {
    pub fn start() -> Watch {
        Watch::default()
    }

    /// The layout now, or `None` when it was asked too recently.
    pub fn poll(&mut self) -> Option<Option<SystemLayout>> {
        if self.asked.is_some_and(|at| at.elapsed() < ASK_EVERY) {
            return None;
        }
        self.asked = Some(Instant::now());
        Some(active_layout())
    }
}
