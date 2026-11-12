//! Linux: the active keyboard layout from the X server's keyboard
//! extension (which also serves programs under a Wayland desktop's X
//! support), or from the system's settings when there is no X server.
//!
//! Asking is a round trip to the server, so a window never asks on its own
//! thread: [`Watch`] keeps a thread that asks every [`ASK_EVERY`] and hands
//! over the answers.

use crate::{ASK_EVERY, SystemLayout, xkb};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{ConnectionExt as _, ID, NameDetail};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// A connection to the X server that can ask for the keyboard layout.
pub struct Asker {
    conn: RustConnection,
}

impl std::fmt::Debug for Asker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Asker")
    }
}

impl Asker {
    /// Connects to the X server named by `DISPLAY` and turns on its keyboard
    /// extension; `None` without one.
    pub fn connect() -> Option<Asker> {
        // Without DISPLAY there is nothing to try (and no reason to log it).
        std::env::var_os("DISPLAY")?;
        let (conn, _) = x11rb::connect(None)
            .inspect_err(|err| tracing::debug!(%err, "no X server for the keyboard layout"))
            .ok()?;
        let used = conn.xkb_use_extension(1, 0).ok()?.reply().ok()?;
        used.supported.then_some(Asker { conn })
    }

    /// The layout of the keyboard's active group; `None` when the server
    /// can't say (or has gone).
    pub fn ask(&mut self) -> Option<SystemLayout> {
        let device = ID::USE_CORE_KBD.into();
        let group = u8::from(self.conn.xkb_get_state(device).ok()?.reply().ok()?.group);
        let names = self
            .conn
            .xkb_get_names(device, NameDetail::SYMBOLS)
            .ok()?
            .reply()
            .ok()?;
        let atom = names.value_list.symbols_name?;
        let symbols = self.conn.get_atom_name(atom).ok()?.reply().ok()?.name;
        xkb::layout_of_group(&String::from_utf8_lossy(&symbols), usize::from(group)).map(SystemLayout::Xkb)
    }

    /// Whether the connection still works.
    fn alive(&self) -> bool {
        self.conn.flush().is_ok()
    }
}

/// The thread that asks, and its answers.
#[derive(Debug)]
pub struct Watch {
    answers: Option<Receiver<Option<SystemLayout>>>,
    /// Dropped with the watch, which ends the thread.
    _stop: Sender<()>,
}

impl Watch {
    pub fn start() -> Watch {
        let (answer, answers) = mpsc::channel();
        let (stop, stopped) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("keyboard-layout".into())
            .spawn(move || ask_until_stopped(&answer, &stopped));
        if let Err(err) = &started {
            tracing::warn!(%err, "the keyboard layout can't be followed");
        }
        Watch {
            answers: started.ok().map(|_| answers),
            _stop: stop,
        }
    }

    /// The newest answer since the last call, if any.
    pub fn poll(&mut self) -> Option<Option<SystemLayout>> {
        self.answers.as_ref()?.try_iter().last()
    }
}

/// Asks every [`ASK_EVERY`] and sends what changed, until the watch is
/// dropped. Without an X server it says what the settings say, once.
fn ask_until_stopped(answer: &Sender<Option<SystemLayout>>, stopped: &Receiver<()>) {
    let Some(mut asker) = Asker::connect() else {
        let _ = answer.send(xkb::from_settings());
        return;
    };
    let mut last = None;
    let mut first = true;
    loop {
        let now = asker.ask();
        if (first || now != last) && answer.send(now.clone()).is_err() {
            return;
        }
        if !asker.alive() {
            return;
        }
        (first, last) = (false, now);
        match stopped.recv_timeout(ASK_EVERY) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
