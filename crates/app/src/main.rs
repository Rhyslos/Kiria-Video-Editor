//! The editor window and its five pages.
//!
//! For now it only starts diagnostics (logging, crash reports, session
//! tracking); the window arrives in Phase 0.6.

fn main() {
    let diagnostics = match diag::start(env!("CARGO_PKG_VERSION")) {
        Ok(diagnostics) => Some(diagnostics),
        Err(err) => {
            // Without logging the app still runs; say why in the terminal.
            eprintln!("{}", err.report());
            None
        }
    };

    if let Some(d) = &diagnostics {
        if d.previous_run_crashed {
            println!("The previous run did not close cleanly. Recovery arrives in Phase 0.4.");
        }
        println!("Logs: {}", d.dirs.logs.display());
    }
    println!(
        "{} editor: nothing to show yet. The window arrives in Phase 0.6 of docs/build-plan.md.",
        brand::APP_NAME
    );

    if let Some(d) = diagnostics
        && let Err(err) = d.finish()
    {
        eprintln!("{}", err.report());
    }
}
