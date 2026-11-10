//! Command-line options. Normal use needs none; the others exist for automated checks.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// A project to open straight away, skipping the project browser.
    pub project: Option<PathBuf>,
    pub check_updates: bool,
    pub mode: RunMode,
    /// Fill a temporary data folder with this many sample projects (screenshots and scroll tests).
    pub demo: Option<usize>,
    /// Open the update window at this step with a sample version (screenshots).
    pub update_window: Option<UpdatePreview>,
}

/// A step of the update window to show with sample numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePreview {
    Available,
    Downloading,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum RunMode {
    #[default]
    Normal,
    /// Open the window, draw a few frames, close. Used by CI to catch start-up crashes.
    SmokeTest,
    /// Draw the window, save a PNG of it, close.
    Screenshot(PathBuf),
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut options = Options {
            check_updates: true,
            ..Options::default()
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-update-check" => options.check_updates = false,
                "--smoke-test" => options.mode = RunMode::SmokeTest,
                "--screenshot" => {
                    let path = args.next().ok_or("--screenshot needs a file name")?;
                    options.mode = RunMode::Screenshot(path.into());
                }
                "--demo" => {
                    let count = args.next().ok_or("--demo needs a number of projects")?;
                    let count = count
                        .parse()
                        .map_err(|_| format!("--demo needs a number, not {count}"))?;
                    options.demo = Some(count);
                    options.check_updates = false;
                }
                "--update-window" => {
                    let step = args
                        .next()
                        .ok_or("--update-window needs available, downloading or ready")?;
                    options.update_window = Some(match step.as_str() {
                        "available" => UpdatePreview::Available,
                        "downloading" => UpdatePreview::Downloading,
                        "ready" => UpdatePreview::Ready,
                        other => return Err(format!("--update-window: unknown step {other}")),
                    });
                    options.check_updates = false;
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
        let plain = parse(&[]).unwrap();
        assert!(plain.check_updates && plain.project.is_none() && plain.mode == RunMode::Normal);
        let o = parse(&["trip.x", "--no-update-check"]).unwrap();
        assert_eq!(o.project, Some("trip.x".into()));
        assert!(!o.check_updates);
        let o = parse(&["--demo", "200", "--screenshot", "s.png"]).unwrap();
        assert_eq!(o.demo, Some(200));
        assert_eq!(o.mode, RunMode::Screenshot("s.png".into()));
        assert!(parse(&["--demo", "many"]).is_err());
        let o = parse(&["--update-window", "downloading"]).unwrap();
        assert_eq!(o.update_window, Some(UpdatePreview::Downloading));
        assert!(!o.check_updates);
        assert!(parse(&["--update-window", "later"]).is_err());
        assert!(parse(&["--nope"]).is_err());
    }
}
