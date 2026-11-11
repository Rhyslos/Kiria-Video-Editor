//! Windows: builds the app icon into the launcher program.

fn main() {
    windows_icon();
}

/// Windows: builds the app icon into the program, so Explorer, the Start
/// menu and shortcuts show it (the window and taskbar icon is set at run time).
fn windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let icon = "../../packaging/icons/icon.ico";
        println!("cargo::rerun-if-changed={icon}");
        if let Err(err) = winresource::WindowsResource::new().set_icon(icon).compile() {
            panic!("the app icon could not be built into the program: {err}");
        }
    }
}
