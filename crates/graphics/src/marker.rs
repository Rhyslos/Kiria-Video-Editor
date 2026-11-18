//! The markers: notes in the data folder that something a graphics driver
//! may not survive is under way. One says a start with Vulkan has begun and
//! hasn't drawn its first frames yet; the other that video is being decoded
//! on the window's device and no picture has come out of it yet. Each
//! program has its own, so the launcher and the editor can start at the
//! same time without reading each other's.
//!
//! They are kept per process, like the editor's run markers
//! ([`diag::marker`]): `starting/<program>-<what>/<process id>.starting`,
//! with a lock beside each that its process holds while the marker stands.
//! Several editors can run at once: each writes and clears only its own, a
//! marker whose lock is held belongs to a copy still in the middle of it,
//! and one whose lock is free was left by a process that is gone, whatever
//! process has its number now. The one file per program that earlier
//! versions wrote is still read, so a failure there is caught once after an
//! update.

use diag::marker::{HeldLock, MarkerSet};
use diag::{AppError, Explain};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The folder of every program's markers, in the data folder.
const MARKERS_DIR: &str = "starting";
const MARKER_EXTENSION: &str = "starting";

/// The programs that open a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Launcher,
    Editor,
}

impl Program {
    /// The folder of the program's markers of one kind.
    fn markers_dir(self, guarded: Guarded) -> &'static str {
        match (self, guarded) {
            (Program::Launcher, Guarded::VulkanStart) => "launcher-vulkan",
            (Program::Editor, Guarded::VulkanStart) => "editor-vulkan",
            (Program::Launcher, Guarded::DeviceDecoding) => "launcher-video-decoding",
            (Program::Editor, Guarded::DeviceDecoding) => "editor-video-decoding",
        }
    }

    /// The one marker file earlier versions wrote.
    fn old_marker_file(self, guarded: Guarded) -> &'static str {
        match (self, guarded) {
            (Program::Launcher, Guarded::VulkanStart) => "launcher-vulkan.starting",
            (Program::Editor, Guarded::VulkanStart) => "vulkan.starting",
            (Program::Launcher, Guarded::DeviceDecoding) => "launcher-video-decoding.starting",
            (Program::Editor, Guarded::DeviceDecoding) => "video-decoding.starting",
        }
    }

    /// What the program's file name starts with, on every system.
    fn process_name(self) -> &'static str {
        match self {
            Program::Launcher => "launcher",
            Program::Editor => "editor",
        }
    }

    /// Whether the program shows video, so that its window's device is worth
    /// opening for video decoding. The launcher only lists projects.
    pub fn plays_video(self) -> bool {
        self == Program::Editor
    }

    /// The program's window in a sentence: "The editor window could not open".
    pub fn window_name(self) -> &'static str {
        match self {
            Program::Launcher => "launcher window",
            Program::Editor => "editor window",
        }
    }
}

/// What a marker guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Guarded {
    /// Opening the window with Vulkan, until its first frames are drawn.
    VulkanStart,
    /// Decoding video on the window's device, until pictures have come out of it.
    DeviceDecoding,
}

impl Guarded {
    /// "The app can't note that it is ...".
    fn doing(self) -> &'static str {
        match self {
            Guarded::VulkanStart => "starting with Vulkan",
            Guarded::DeviceDecoding => "decoding video on the window's graphics device",
        }
    }

    /// What is used when the note can't be written.
    fn instead(self) -> &'static str {
        match self {
            Guarded::VulkanStart => "Direct3D 12 is used meanwhile.",
            Guarded::DeviceDecoding => "Video is decoded the usual way meanwhile.",
        }
    }
}

/// What a start finds where the marker would be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerState {
    Absent,
    /// A start with Vulkan never drew a frame, and that process is gone.
    LeftBehind,
    /// Another copy of the program is starting with Vulkan right now, or is
    /// stuck doing so: too early to say that Vulkan failed.
    StartUnderWay,
}

/// One program's markers of one kind, and this process's own among them.
///
/// Clones share this process's marker: what one writes, another clears.
#[derive(Debug, Clone)]
pub struct StartMarker {
    program: Program,
    guarded: Guarded,
    markers: MarkerSet,
    /// The one file earlier versions wrote.
    old_file: PathBuf,
    /// The process whose marker this writes.
    pid: u32,
    /// The lock beside this process's marker while it stands.
    held: Arc<Mutex<Option<HeldLock>>>,
}

impl StartMarker {
    /// The marker for `program`'s start with Vulkan.
    pub fn in_dir(program: Program, data_dir: &Path) -> StartMarker {
        StartMarker::guarding(Guarded::VulkanStart, program, data_dir)
    }

    /// The marker for `program` decoding video on its window's device.
    pub fn for_decoding(program: Program, data_dir: &Path) -> StartMarker {
        StartMarker::guarding(Guarded::DeviceDecoding, program, data_dir)
    }

    fn guarding(guarded: Guarded, program: Program, data_dir: &Path) -> StartMarker {
        StartMarker {
            program,
            guarded,
            markers: MarkerSet::new(
                data_dir.join(MARKERS_DIR).join(program.markers_dir(guarded)),
                MARKER_EXTENSION,
            ),
            old_file: data_dir.join(program.old_marker_file(guarded)),
            pid: std::process::id(),
            held: Arc::new(Mutex::new(None)),
        }
    }

    /// The same marker as written by process `pid` (tests act as several).
    #[cfg(test)]
    pub(crate) fn of_process(mut self, pid: u32) -> StartMarker {
        self.pid = pid;
        self.held = Arc::new(Mutex::new(None));
        self
    }

    /// Whether any process's marker of this kind is there.
    pub fn is_present(&self) -> bool {
        self.old_file.exists() || !self.markers.found().is_empty()
    }

    /// Reads the markers and checks whether the processes that wrote them
    /// are still there.
    pub fn state(&self) -> MarkerState {
        let program = self.program;
        self.state_with(|pid| is_running(program, pid))
    }

    /// [`StartMarker::state`] with the process check passed in, for the
    /// file earlier versions wrote (the others have their lock to tell).
    /// A marker left behind counts first: one copy still in the middle of
    /// it doesn't hide another that failed.
    pub fn state_with(&self, is_running: impl Fn(u32) -> bool) -> MarkerState {
        let found = self
            .markers
            .found()
            .into_iter()
            .filter_map(|marker| match marker.running {
                Some(true) => Some(MarkerState::StartUnderWay),
                Some(false) => Some(MarkerState::LeftBehind),
                // No lock beside it: its process is gone, unless it has
                // just removed both and the marker is gone too.
                None => marker.path.exists().then_some(MarkerState::LeftBehind),
            });
        let states: Vec<MarkerState> = self.old_state(is_running).into_iter().chain(found).collect();
        [MarkerState::LeftBehind, MarkerState::StartUnderWay]
            .into_iter()
            .find(|state| states.contains(state))
            .unwrap_or(MarkerState::Absent)
    }

    /// What the file earlier versions wrote says, when it is there.
    fn old_state(&self, is_running: impl Fn(u32) -> bool) -> Option<MarkerState> {
        let text = match std::fs::read_to_string(&self.old_file) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            // There, but unreadable: count it as a failed start, the safe side.
            Err(_) => return Some(MarkerState::LeftBehind),
        };
        let writer = text
            .strip_prefix("pid ")
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        Some(match writer {
            // Our own number can only be a finished process whose number was given out again.
            Some(pid) if pid != std::process::id() && is_running(pid) => MarkerState::StartUnderWay,
            _ => MarkerState::LeftBehind,
        })
    }

    /// Call before what the marker guards: opening a window with Vulkan,
    /// or decoding on the window's device. Writes this process's marker and
    /// holds the lock beside it until [`StartMarker::clear`]. A marker left
    /// by an earlier process with the same number is kept, not overwritten.
    pub fn write(&self) -> diag::Result<()> {
        let cannot = |why: String| {
            AppError::new(format!("The app can't note that it is {}", self.guarded.doing()))
                .why(why)
                .fix(format!(
                    "Check that the disk is not full. {}",
                    self.guarded.instead()
                ))
        };
        let path = self.markers.marker_path(self.pid);
        let mut held = self
            .held
            .lock()
            .map_err(|_| cannot("Another thread failed while writing it.".into()))?;
        if held.is_none() {
            let lock = self
                .markers
                .hold(self.pid)
                .or_explain(|| cannot(format!("The lock beside {} can't be taken.", path.display())))?;
            *held = Some(lock);
        }
        std::fs::write(&path, format!("pid {}\n", self.pid))
            .or_explain(|| cannot(format!("Writing {} failed.", path.display())))
    }

    /// Call once what the marker guards has worked (Vulkan drew its first
    /// frames; pictures came out of the device), or has reported an error.
    /// Removes only this process's own marker, never another copy's.
    pub fn clear(&self) -> diag::Result<()> {
        let path = self.markers.marker_path(self.pid);
        let cannot = |err| {
            AppError::new("The app can't remove a note it left for itself")
                .why(format!("Removing {} failed.", path.display()))
                .fix("Nothing is lost; the next start may play safe without need.")
                .source(err)
        };
        let Ok(mut held) = self.held.lock() else {
            return Ok(());
        };
        match held.take() {
            Some(lock) => lock.release().map_err(cannot),
            None => Ok(()),
        }
    }

    /// Removes every marker of this kind left behind by a process that is
    /// gone (and the file earlier versions wrote), once a start has dealt
    /// with them. Markers of copies still running stay.
    pub fn forget_left_behind(&self) -> diag::Result<()> {
        let program = self.program;
        let mut gone: Vec<PathBuf> = self
            .markers
            .found()
            .into_iter()
            .filter(|marker| marker.running != Some(true))
            .map(|marker| marker.path)
            .collect();
        if self.old_state(|pid| is_running(program, pid)) == Some(MarkerState::LeftBehind) {
            gone.push(self.old_file.clone());
        }
        for path in gone {
            let found = diag::marker::FoundMarker {
                path: path.clone(),
                running: None,
            };
            found.forget().map_err(|err| {
                AppError::new("The app can't remove a note it left for itself")
                    .why(format!("Removing {} failed.", path.display()))
                    .fix("Nothing is lost; the next start may play safe without need.")
                    .source(err)
            })?;
        }
        Ok(())
    }
}

/// Whether a process with this id is running and is this program.
fn is_running(program: Program, pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(mut system) = System::new() else {
        // Can't tell: count the marker as a failed start, the safe side.
        return false;
    };
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).is_some_and(|p| {
        p.name()
            .to_string_lossy()
            .to_lowercase()
            .starts_with(program.process_name())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_written_found_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(Program::Editor, tmp.path());
        assert!(!marker.is_present());
        assert_eq!(marker.state(), MarkerState::Absent);
        marker.clear().unwrap(); // nothing to clear is fine
        marker.write().unwrap();
        assert!(StartMarker::in_dir(Program::Editor, tmp.path()).is_present());
        marker.clear().unwrap();
        assert!(!marker.is_present());
        assert_eq!(marker.state(), MarkerState::Absent);
    }

    #[test]
    fn the_launcher_and_the_editor_never_read_each_others_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = StartMarker::in_dir(Program::Launcher, tmp.path());
        let editor = StartMarker::in_dir(Program::Editor, tmp.path());
        launcher.write().unwrap();
        assert_eq!(editor.state(), MarkerState::Absent);
        assert_eq!(launcher.state(), MarkerState::StartUnderWay);
        editor.write().unwrap();
        launcher.clear().unwrap();
        assert!(editor.is_present() && !launcher.is_present());
    }

    #[test]
    fn two_copies_at_once_never_clear_each_others_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let a = StartMarker::in_dir(Program::Editor, tmp.path()).of_process(100);
        let b = StartMarker::in_dir(Program::Editor, tmp.path()).of_process(200);
        a.write().unwrap();
        b.write().unwrap();
        a.clear().unwrap();
        let fresh = StartMarker::in_dir(Program::Editor, tmp.path());
        assert!(fresh.is_present(), "B's marker stays");
        assert_eq!(fresh.state(), MarkerState::StartUnderWay);
        fresh.forget_left_behind().unwrap();
        assert!(fresh.is_present(), "a copy still at it is never forgotten");
        // B's driver takes it down: its marker is left behind.
        drop(b);
        assert_eq!(fresh.state(), MarkerState::LeftBehind);
        fresh.forget_left_behind().unwrap();
        assert!(!fresh.is_present());
    }

    #[test]
    fn a_failed_copy_counts_even_while_another_is_at_it() {
        let tmp = tempfile::tempdir().unwrap();
        let running = StartMarker::in_dir(Program::Editor, tmp.path()).of_process(100);
        running.write().unwrap();
        StartMarker::in_dir(Program::Editor, tmp.path())
            .of_process(200)
            .write()
            .unwrap(); // dropped: that process is gone
        let fresh = StartMarker::in_dir(Program::Editor, tmp.path());
        assert_eq!(fresh.state(), MarkerState::LeftBehind);
    }

    #[test]
    fn a_marker_left_by_a_dead_process_with_our_number_is_still_found() {
        let tmp = tempfile::tempdir().unwrap();
        // The earlier process with this number wrote it and crashed.
        StartMarker::in_dir(Program::Editor, tmp.path()).write().unwrap();
        let marker = StartMarker::in_dir(Program::Editor, tmp.path());
        assert_eq!(marker.state(), MarkerState::LeftBehind);
        // Even when this process writes its own before that one is dealt
        // with, the earlier one is kept, and stays once this one is cleared.
        marker.write().unwrap();
        marker.clear().unwrap();
        assert_eq!(marker.state(), MarkerState::LeftBehind);
    }

    #[test]
    fn an_older_versions_marker_still_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(Program::Launcher, tmp.path());
        let old = tmp.path().join("launcher-vulkan.starting");
        std::fs::write(&old, "pid 4000000000\n").unwrap();
        assert!(marker.is_present());
        assert_eq!(marker.state_with(|_| true), MarkerState::StartUnderWay);
        assert_eq!(marker.state_with(|_| false), MarkerState::LeftBehind);
        // No such process, and the test program is not the launcher either.
        assert_eq!(marker.state(), MarkerState::LeftBehind);
        // Our own number, or none, can only be a finished process.
        std::fs::write(&old, format!("pid {}\n", std::process::id())).unwrap();
        assert_eq!(marker.state_with(|_| true), MarkerState::LeftBehind);
        std::fs::write(&old, "").unwrap();
        assert_eq!(marker.state_with(|_| true), MarkerState::LeftBehind);
        marker.forget_left_behind().unwrap();
        assert!(!marker.is_present());
    }
}
