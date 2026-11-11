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
    /// Start with the viewer full screen (`--full-screen`, for screenshots).
    pub full_screen: bool,
    /// Commands to run once open, by name (`--action KeyboardShortcuts`, repeatable, for screenshots).
    pub actions: Vec<String>,
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
                "--full-screen" => options.full_screen = true,
                "--action" => {
                    let name = args.next().ok_or("--action needs a command name")?;
                    options.actions.push(name);
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
    }
}
