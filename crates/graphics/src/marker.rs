//! The markers: notes in the data folder that something a graphics driver
//! may not survive is under way. One says a start with Vulkan has begun and
//! hasn't drawn its first frames yet; the other that video is being decoded
//! on the window's device and no picture has come out of it yet. Each
//! program has its own, so the launcher and the editor can start at the
//! same time without reading each other's.

use diag::{AppError, Explain};
use std::path::{Path, PathBuf};

/// The programs that open a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Launcher,
    Editor,
}

impl Program {
    fn marker_file(self, guarded: Guarded) -> &'static str {
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

/// One program's marker file.
#[derive(Debug, Clone)]
pub struct StartMarker {
    program: Program,
    guarded: Guarded,
    path: PathBuf,
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
            path: data_dir.join(program.marker_file(guarded)),
        }
    }

    pub fn is_present(&self) -> bool {
        self.path.exists()
    }

    /// Reads the marker and checks whether the process that wrote it is still there.
    pub fn state(&self) -> MarkerState {
        let program = self.program;
        self.state_with(|pid| is_running(program, pid))
    }

    /// [`StartMarker::state`] with the process check passed in.
    pub fn state_with(&self, is_running: impl Fn(u32) -> bool) -> MarkerState {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return MarkerState::Absent,
            // There, but unreadable: count it as a failed start, the safe side.
            Err(_) => return MarkerState::LeftBehind,
        };
        let writer = text
            .strip_prefix("pid ")
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        match writer {
            // Our own number can only be a finished process whose number was given out again.
            Some(pid) if pid != std::process::id() && is_running(pid) => MarkerState::StartUnderWay,
            _ => MarkerState::LeftBehind,
        }
    }

    /// Call before what the marker guards: opening a window with Vulkan,
    /// or decoding on the window's device.
    pub fn write(&self) -> diag::Result<()> {
        std::fs::write(&self.path, format!("pid {}\n", std::process::id())).or_explain(|| {
            AppError::new(format!("The app can't note that it is {}", self.guarded.doing()))
                .why(format!("Writing {} failed.", self.path.display()))
                .fix(format!(
                    "Check that the disk is not full. {}",
                    self.guarded.instead()
                ))
        })
    }

    /// Call once what the marker guards has worked (Vulkan drew its first
    /// frames; pictures came out of the device), or has reported an error.
    pub fn clear(&self) -> diag::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::new("The app can't remove a note it left for itself")
                .why(format!("Removing {} failed.", self.path.display()))
                .fix("Nothing is lost; the next start may play safe without need.")
                .source(err)),
        }
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
    }

    #[test]
    fn the_launcher_and_the_editor_never_read_each_others_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = StartMarker::in_dir(Program::Launcher, tmp.path());
        let editor = StartMarker::in_dir(Program::Editor, tmp.path());
        launcher.write().unwrap();
        assert_eq!(editor.state_with(|_| false), MarkerState::Absent);
        assert_eq!(launcher.state_with(|_| false), MarkerState::LeftBehind);
        editor.write().unwrap();
        launcher.clear().unwrap();
        assert!(editor.is_present() && !launcher.is_present());
    }

    #[test]
    fn a_marker_whose_program_is_still_running_is_not_a_failed_start_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(Program::Launcher, tmp.path());
        std::fs::write(tmp.path().join("launcher-vulkan.starting"), "pid 4000000000\n").unwrap();
        assert_eq!(marker.state_with(|_| true), MarkerState::StartUnderWay);
        assert_eq!(marker.state_with(|_| false), MarkerState::LeftBehind);
        // No such process, and the test program is not the launcher either.
        assert_eq!(marker.state(), MarkerState::LeftBehind);
    }

    #[test]
    fn a_marker_with_our_own_number_or_no_number_is_a_failed_start() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(Program::Editor, tmp.path());
        marker.write().unwrap(); // as a finished process with our number left it
        assert_eq!(marker.state_with(|_| true), MarkerState::LeftBehind);
        std::fs::write(tmp.path().join("vulkan.starting"), "").unwrap();
        assert_eq!(marker.state_with(|_| true), MarkerState::LeftBehind);
    }
}
