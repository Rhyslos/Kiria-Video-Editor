//! Opening a file with the program the system uses for it, such as a web
//! page in the default browser.

use std::io;
use std::path::Path;

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
