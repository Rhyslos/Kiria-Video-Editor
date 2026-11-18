//! Opening a file with the program the system uses for it, such as a web
//! page in the default browser, and showing a folder in the file manager.

use diag::AppError;
use std::io;
use std::path::{Path, PathBuf};

/// Opens `path` with the program the system has for it (an `.html` file in
/// the default web browser), as a double-click on it would. It may take a
/// moment while the system starts that program, so call it off the
/// interface thread. An error when the system refuses or has no program.
pub fn open_with_system(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        shell_open(path)
    }
    #[cfg(not(windows))]
    {
        // The desktop's own opener; it returns once the program is asked to
        // open the file, and fails when no program is set up for it.
        let status = std::process::Command::new("xdg-open").arg(path).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("xdg-open ended with {status}")))
        }
    }
}

/// Windows: the shell's "open" verb on the file. The one foreign call here.
#[cfg(windows)]
#[allow(unsafe_code)]
fn shell_open(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |text: &std::ffi::OsStr| text.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let verb = wide(std::ffi::OsStr::new("open"));
    let file = wide(path.as_os_str());
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that live until
    // the call returns; the window, parameters and folder may be null, as
    // the function's documentation allows. It reads its arguments only
    // during the call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // The result is a number made to look like a handle: over 32 means the
    // program was started; 32 and under is the reason it wasn't.
    let code = result as usize;
    if code > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the system could not open it (ShellExecute answered {code})"
        )))
    }
}

/// Shows `folder` in the system's file manager. Only an existing folder is
/// shown: a missing path or a file is refused with the reason, before
/// anything is started, and the path handed over is always a full one (so
/// typed text never reaches the file manager as an option). It touches the
/// disk and may wait while the file manager is asked, so call it off the
/// interface thread. No ended process is left behind.
pub fn show_folder(folder: &Path) -> diag::Result<()> {
    show_folder_with(folder, start_file_manager)
}

/// [`show_folder`] with the starting of the file manager handed in (tests
/// start nothing).
fn show_folder_with(folder: &Path, start: impl FnOnce(&Path) -> io::Result<()>) -> diag::Result<()> {
    let full = existing_folder(folder)?;
    start(&full).map_err(|err| {
        AppError::new("The folder can't be shown")
            .why(format!(
                "Starting the file manager for {} failed.",
                full.display()
            ))
            .fix(format!("Open {} yourself in the file manager.", full.display()))
            .source(err)
    })
}

/// `folder` as a full path, when it is an existing folder.
fn existing_folder(folder: &Path) -> diag::Result<PathBuf> {
    let refused = |why: String| {
        AppError::new("The folder can't be shown")
            .why(why)
            .fix("Check the folder's path: it may have been moved, renamed or removed.")
    };
    if folder.as_os_str().is_empty() {
        return Err(refused("No folder was given.".to_owned()));
    }
    let full = std::path::absolute(folder).map_err(|err| {
        refused(format!("{} isn't a path the system can use.", folder.display())).source(err)
    })?;
    if full.is_dir() {
        Ok(full)
    } else if full.exists() {
        Err(refused(format!("{} is a file, not a folder.", full.display())))
    } else {
        Err(refused(format!("{} can't be found.", full.display())))
    }
}

/// Starts the file manager at `folder`. Windows: `explorer`, let go of at
/// once (its handles are closed, and Windows keeps nothing of a process
/// once it ends; with folders in a process of their own it lives as long
/// as the window, and its answer is 1 even when it worked, so it isn't
/// waited for). Elsewhere `xdg-open`, waited for (an ended child is kept
/// by the system until its parent reads its answer): it asks the desktop's
/// file manager and ends, failing when there is none.
fn start_file_manager(folder: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(folder)
            .spawn()
            .map(drop)
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("xdg-open").arg(folder).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("xdg-open ended with {status}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows `folder` with a file manager that only notes what it was given.
    fn shown(folder: &Path) -> (diag::Result<()>, Option<PathBuf>) {
        let mut given = None;
        let result = show_folder_with(folder, |f| {
            given = Some(f.to_path_buf());
            Ok(())
        });
        (result, given)
    }

    #[test]
    fn an_existing_folder_is_handed_over_in_full() {
        let dir = tempfile::tempdir().unwrap();
        let (result, given) = shown(dir.path());
        assert!(result.is_ok());
        let given = given.unwrap();
        assert!(given.is_absolute());
        assert_eq!(given, std::path::absolute(dir.path()).unwrap());
    }

    #[test]
    fn a_missing_folder_a_file_and_nothing_are_refused_before_starting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clip.mp4");
        std::fs::write(&file, b"not a folder").unwrap();
        let missing = dir.path().join("gone");
        for (path, why) in [
            (file.as_path(), "is a file, not a folder"),
            (missing.as_path(), "can't be found"),
            (Path::new(""), "No folder was given"),
        ] {
            let (result, given) = shown(path);
            let err = result.unwrap_err();
            assert_eq!(err.what(), "The folder can't be shown");
            assert!(
                err.why_text().is_some_and(|w| w.contains(why)),
                "{path:?}: {err:?}"
            );
            assert!(given.is_none(), "nothing started for {path:?}");
        }
    }

    #[test]
    fn a_relative_path_never_reaches_the_file_manager_as_an_option() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("-n")).unwrap();
        // Relative to the working directory: it doesn't exist there.
        let (result, given) = shown(Path::new("-n"));
        assert!(result.is_err() && given.is_none());
        let (result, given) = shown(&dir.path().join("-n"));
        assert!(result.is_ok());
        assert!(given.unwrap().is_absolute());
    }

    #[test]
    fn a_file_manager_that_fails_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let err = show_folder_with(dir.path(), |_| Err(io::Error::other("no file manager"))).unwrap_err();
        assert!(
            err.why_text()
                .is_some_and(|w| w.contains("Starting the file manager"))
        );
        assert!(err.fix_text().contains("yourself"));
    }
}
