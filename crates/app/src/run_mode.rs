//! Command-line options. Normal use needs none; the others exist for automated checks.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// A project to open (created if it does not exist yet).
    pub project: Option<PathBuf>,
    /// Open the project as last saved, setting aside unsaved work from a crash
    /// (the launcher's "Open last save"). Normally that work is recovered.
    pub open_last_save: bool,
    /// Media files to import into the project at start (`--import <file>`, repeatable).
    pub import: Vec<PathBuf>,
    /// Where the playhead starts, in seconds (`--playhead <seconds>`, for screenshots).
    pub playhead: Option<f64>,
    /// Start playing once the project and its imports are ready (`--play`, for checks).
    pub play: bool,
    /// Open the media drawer (`--media-drawer`, for screenshots).
    pub media_drawer: bool,
    /// Start on the Export page (`--export-page`, for screenshots).
    pub export_page: bool,
    /// Start with the viewer full screen (`--full-screen`, for screenshots).
    pub full_screen: bool,
    /// Select the first clip of the top video track that has one, once the
    /// project and its imports are in (`--select-first-clip`, for
    /// screenshots of what a selected clip shows: the inspector's values,
    /// the handles on its picture).
    pub select_first_clip: bool,
    /// Commands to run once open, by name (`--action KeyboardShortcuts`, repeatable, for screenshots).
    pub actions: Vec<String>,
    /// Open the keyboard shortcuts page in a state: `filter`, `rebinding` or
    /// `clash` (`--shortcuts-page clash`, for screenshots).
    pub shortcuts_page: Option<ui_kit::keyboard::Preview>,
    /// Write the printable cheat sheet here and exit, without opening a
    /// window (`--cheat-sheet keys.html`, for checks).
    pub cheat_sheet: Option<PathBuf>,
    /// Convert the viewer's colour on the CPU instead of the GPU (`--cpu-colour`, for comparing the two).
    pub cpu_colour: bool,
    /// Show the question about a failed start with Vulkan (`--graphics-warning`, for screenshots).
    pub graphics_warning: bool,
    /// Stop the background reading of waveforms and clip pictures at this
    /// share, 0 to 1 (`--hold-reading 0.4`, for screenshots of reading in progress).
    pub hold_reading: Option<f32>,
    /// Put the pointer here, and maybe drag it from there with the left
    /// button held (`--pointer 640,420 [--drag-to 640,480]`, for
    /// screenshots of what pointing and dragging show). `--release` after
    /// `--drag-to` lets go at the end, so the drag is carried out;
    /// `--right-click` after `--pointer` clicks the right button there, for
    /// screenshots of menus, and `--double-click` the left button twice.
    pub pointer: Option<crate::fake_pointer::FakePointer>,
    /// Show the question about video decoding that stopped the app (`--decoding-warning`, for screenshots).
    pub decoding_warning: bool,
    pub mode: RunMode,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum RunMode {
    #[default]
    Normal,
    /// Open the window, draw a few frames, close. Used by CI to catch start-up crashes.
    SmokeTest,
    /// Draw the window, save a PNG of it, close.
    Screenshot(PathBuf),
    /// Run an 8-second background job and measure how long the interface
    /// takes per frame meanwhile (Gate 0).
    ResponsivenessTest,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--smoke-test" => options.mode = RunMode::SmokeTest,
                "--open-last-save" => options.open_last_save = true,
                "--responsiveness-test" => options.mode = RunMode::ResponsivenessTest,
                "--import" => {
                    let path = args.next().ok_or("--import needs a file name")?;
                    options.import.push(path.into());
                }
                "--play" => options.play = true,
                "--media-drawer" => options.media_drawer = true,
                "--export-page" => options.export_page = true,
                "--full-screen" => options.full_screen = true,
                "--select-first-clip" => options.select_first_clip = true,
                "--cpu-colour" => options.cpu_colour = true,
                "--graphics-warning" => options.graphics_warning = true,
                "--decoding-warning" => options.decoding_warning = true,
                "--action" => {
                    let name = args.next().ok_or("--action needs a command name")?;
                    options.actions.push(name);
                }
                "--cheat-sheet" => {
                    let path = args.next().ok_or("--cheat-sheet needs a file name")?;
                    options.cheat_sheet = Some(path.into());
                }
                "--shortcuts-page" => {
                    let state = args
                        .next()
                        .as_deref()
                        .and_then(ui_kit::keyboard::Preview::from_name)
                        .ok_or("--shortcuts-page needs filter, rebinding or clash")?;
                    options.shortcuts_page = Some(state);
                }
                "--pointer" | "--drag-to" => {
                    let pos = args
                        .next()
                        .as_deref()
                        .and_then(crate::fake_pointer::FakePointer::parse_pos)
                        .ok_or_else(|| format!("{arg} needs a position such as 640,420"))?;
                    options.pointer = Some(match (arg.as_str(), options.pointer) {
                        ("--drag-to", Some(pointer)) => crate::fake_pointer::FakePointer {
                            drag_to: Some(pos),
                            ..pointer
                        },
                        ("--drag-to", None) => return Err("--drag-to needs --pointer before it".into()),
                        _ => crate::fake_pointer::FakePointer::resting(pos),
                    });
                }
                "--release" => match &mut options.pointer {
                    Some(pointer) if pointer.drag_to.is_some() => pointer.release = true,
                    _ => return Err("--release needs --drag-to before it".into()),
                },
                "--right-click" | "--double-click" => match &mut options.pointer {
                    Some(pointer) if pointer.drag_to.is_none() && arg == "--right-click" => {
                        pointer.right_click = true;
                    }
                    Some(pointer) if pointer.drag_to.is_none() => pointer.double_click = true,
                    _ => return Err(format!("{arg} needs --pointer before it, and no --drag-to")),
                },
                "--hold-reading" => {
                    let share = args
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or("--hold-reading needs a share between 0 and 1")?;
                    options.hold_reading = Some(share);
                }
                "--playhead" => {
                    let secs = args
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or("--playhead needs seconds")?;
                    options.playhead = Some(secs);
                }
                "--screenshot" => {
                    let path = args.next().ok_or("--screenshot needs a file name")?;
                    options.mode = RunMode::Screenshot(path.into());
                }
                other if other.starts_with("--") => return Err(format!("unknown option {other}")),
                path => options.project = Some(path.into()),
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn options_parse() {
        assert_eq!(parse(&[]).unwrap(), Options::default());
        let o = parse(&["trip.project", "--screenshot", "shot.png"]).unwrap();
        assert_eq!(o.project, Some("trip.project".into()));
        assert_eq!(o.mode, RunMode::Screenshot("shot.png".into()));
        assert!(parse(&["--screenshot"]).is_err());
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["p.x", "--open-last-save"]).unwrap().open_last_save);
        let o = parse(&["p.x", "--import", "a.mkv", "--import", "b.mp4"]).unwrap();
        assert_eq!(o.import, [PathBuf::from("a.mkv"), PathBuf::from("b.mp4")]);
        assert!(parse(&["--import"]).is_err());
        assert!(parse(&["p.x", "--cpu-colour"]).unwrap().cpu_colour);
        assert!(parse(&["p.x", "--export-page"]).unwrap().export_page);
        assert!(parse(&["p.x", "--select-first-clip"]).unwrap().select_first_clip);
        assert!(!parse(&["p.x"]).unwrap().select_first_clip);
        assert!(!parse(&["p.x"]).unwrap().cpu_colour);
        assert_eq!(
            parse(&["p.x", "--hold-reading", "0.4"]).unwrap().hold_reading,
            Some(0.4)
        );
        assert!(parse(&["p.x", "--hold-reading", "soon"]).is_err());
        let o = parse(&["p.x", "--pointer", "640,420", "--drag-to", "640,480"]).unwrap();
        let pointer = o.pointer.unwrap();
        assert_eq!(
            (pointer.at, pointer.drag_to),
            (egui::pos2(640.0, 420.0), Some(egui::pos2(640.0, 480.0)))
        );
        assert!(parse(&["p.x", "--drag-to", "640,480"]).is_err(), "from where?");
        assert!(parse(&["p.x", "--pointer", "640"]).is_err());
        assert!(!pointer.release && !pointer.right_click);
        let dragged = ["p.x", "--pointer", "640,420", "--drag-to", "640,480", "--release"];
        assert!(parse(&dragged).unwrap().pointer.unwrap().release);
        assert!(
            parse(&["p.x", "--pointer", "640,420", "--release"]).is_err(),
            "of what?"
        );
        let o = parse(&["p.x", "--pointer", "40,300", "--right-click"]).unwrap();
        assert!(o.pointer.unwrap().right_click);
        assert!(parse(&["p.x", "--right-click"]).is_err(), "where?");
        let o = parse(&["p.x", "--pointer", "40,300", "--double-click"]).unwrap();
        assert!(o.pointer.unwrap().double_click);
        let both = [
            "p.x",
            "--pointer",
            "640,420",
            "--drag-to",
            "640,480",
            "--double-click",
        ];
        assert!(parse(&both).is_err(), "a drag or a double-click, not both");
        assert!(parse(&["--decoding-warning"]).unwrap().decoding_warning);
        assert_eq!(
            parse(&["--shortcuts-page", "clash"]).unwrap().shortcuts_page,
            Some(ui_kit::keyboard::Preview::Clash)
        );
        assert!(parse(&["--shortcuts-page", "sideways"]).is_err());
        assert_eq!(
            parse(&["--cheat-sheet", "keys.html"]).unwrap().cheat_sheet,
            Some("keys.html".into())
        );
        assert!(parse(&["--cheat-sheet"]).is_err());
    }
}
