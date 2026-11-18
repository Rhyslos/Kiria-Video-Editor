//! Command-line options. Normal use needs none; the others exist for
//! automated checks, and everything a check scripts is one [`Automation`].

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// A project to open (created if it does not exist yet).
    pub project: Option<PathBuf>,
    /// Open the project as last saved, setting aside unsaved work from a crash
    /// (the launcher's "Open last save"). Normally that work is recovered.
    pub open_last_save: bool,
    /// Media files to import into the project at start (`--import <file>`,
    /// repeatable; a folder stands for every file in it, see [`expand_folders`]).
    pub import: Vec<PathBuf>,
    /// Write the printable cheat sheet here and exit, without opening a
    /// window (`--cheat-sheet keys.html`, for checks).
    pub cheat_sheet: Option<PathBuf>,
    pub mode: RunMode,
    /// What an automated run scripts: where the window starts and what it
    /// does by itself (screenshots, the responsiveness test).
    pub automation: Automation,
}

/// Everything the command line scripts for automated runs, in one value:
/// the window's start, the steps it takes by itself once the project is in
/// (`editor::script`) and the background work held part way. None of it is
/// used in normal runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Automation {
    /// Where the playhead starts, in seconds (`--playhead <seconds>`, for screenshots).
    pub playhead: Option<f64>,
    /// Start playing once the project and its imports are ready (`--play`, for checks).
    pub play: bool,
    /// Open the media drawer (`--media-drawer`, for screenshots).
    pub media_drawer: bool,
    /// Open the media drawer with these words in "Find a file" (`--find
    /// drone`, for screenshots of search).
    pub find: Option<String>,
    /// Watch this folder for new recordings too, as if it were one of the
    /// launcher's media folders (`--watch-folder <folder>`, repeatable, for checks).
    pub watch_folders: Vec<PathBuf>,
    /// Start on the Export page (`--export-page`, for screenshots).
    pub export_page: bool,
    /// Start on the Export page with its queue filled: an export done, one
    /// paused a third through, one waiting (`--export-queue`, for
    /// screenshots of the queue; the files go beside the project).
    pub export_queue: bool,
    /// Ask about closing as if the window's close button had been pressed,
    /// once an export is unfinished (`--close-request`, with
    /// `--export-queue` for screenshots of the question).
    pub close_request: bool,
    /// Start on this page (`--page library`, for screenshots and the
    /// responsiveness test of a page).
    pub page: Option<ui_kit::Page>,
    /// Open this tab of the Library page: a type or a list's name
    /// (`--library-tab audio`, for screenshots).
    pub library_tab: Option<String>,
    /// Show the file with this name in the Library page's detail panel
    /// (`--library-select voice.mp3`, for screenshots).
    pub library_select: Option<String>,
    /// Start with the viewer full screen (`--full-screen`, for screenshots).
    pub full_screen: bool,
    /// Select the first clip of the top video track that has one, once the
    /// project and its imports are in (`--select-first-clip`, for
    /// screenshots of what a selected clip shows: the inspector's values,
    /// the handles on its picture).
    pub select_first_clip: bool,
    /// Start with the media drawer open on its Effects side
    /// (`--effects-drawer`, for screenshots).
    pub effects_drawer: bool,
    /// Select every clip of the open timeline once the project and its
    /// imports are in, and run Sync by sound on them (`--sync-by-sound`,
    /// for screenshots of its result).
    pub sync_by_sound: bool,
    /// Commands to run once open, by name (`--action KeyboardShortcuts`, repeatable, for screenshots).
    pub actions: Vec<String>,
    /// Commands to run once the scripted pointer has done its part
    /// (`--then Copy`, repeatable, in order: for screenshots of what a
    /// command does to what a drag chose).
    pub then_actions: Vec<String>,
    /// Open Settings at a tab or a setting, by its id, with words typed in
    /// its search box when `settings_search` is set (`--settings proxies`,
    /// `--settings-search proxy`, for screenshots).
    pub settings: Option<String>,
    pub settings_search: Option<String>,
    /// Open the keyboard shortcuts page in a state: `filter`, `rebinding` or
    /// `clash` (`--shortcuts-page clash`, for screenshots).
    pub shortcuts_page: Option<ui_kit::keyboard::Preview>,
    /// Convert the viewer's colour on the CPU instead of the GPU (`--cpu-colour`, for comparing the two).
    pub cpu_colour: bool,
    /// Show the question about a failed start with Vulkan (`--graphics-warning`, for screenshots).
    pub graphics_warning: bool,
    /// Show the question about video decoding that stopped the app (`--decoding-warning`, for screenshots).
    pub decoding_warning: bool,
    /// Background work stopped part way, for screenshots of it in progress
    /// (`--hold-reading`, `--hold-proxy`, `--hold-package`).
    pub holds: Holds,
    /// With `--screenshot`: wait at most this many seconds for what the
    /// screenshot is of (`--screenshot-wait 60`, for slow machines).
    pub screenshot_wait: Option<f64>,
    /// Open Package project with this folder chosen (`--package-to out`,
    /// for screenshots of the sizes shown first).
    pub package_to: Option<PathBuf>,
    /// Put the pointer here, and maybe drag it from there with the left
    /// button held (`--pointer 640,420 [--drag-to 640,480]`, for
    /// screenshots of what pointing and dragging show). `--release` after
    /// `--drag-to` lets go at the end, so the drag is carried out;
    /// `--right-click` after `--pointer` clicks the right button there, for
    /// screenshots of menus, `--double-click` the left button twice, and
    /// `--scroll 400` turns the wheel 400 points down there (to see the
    /// bottom of a panel).
    /// `--click` once (the Display menu, opened).
    pub pointer: Option<crate::fake_pointer::FakePointer>,
    /// The Audio page's tool, bottom view and track, and a project made to
    /// have many audio tracks (`--audio-tool compressor`, `--audio-view
    /// stacked`, `--audio-band stereo`, `--audio-sizes 0.3,0.35`, `--audio-track 3`, `--audio-tracks 30`, `--audio-clarity-region`; for screenshots
    /// and the responsiveness test of the Audio page).
    pub audio: crate::audio_page::Start,
    /// The window's size in points (`--window-size 1280x800`, for
    /// screenshots of a smaller screen); 1440 × 900 when not given.
    pub window_size: Option<[f32; 2]>,
}

/// Background work stopped at a share, 0 to 1, for screenshots of it in
/// progress; `None` runs it to the end, as normal runs always do.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Holds {
    /// Waveforms and clip pictures (`--hold-reading 0.4`).
    pub reading: Option<f32>,
    /// Proxy builds (`--hold-proxy 0.43`).
    pub proxy: Option<f32>,
    /// With `--package-to`, also start packaging and stop copying here
    /// (`--hold-package 0.4`).
    pub package: Option<f32>,
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
    /// takes per frame meanwhile (Gate 0); with `--import`, until the
    /// imports are in too (Gate 5).
    ResponsivenessTest,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--smoke-test" => options.mode = RunMode::SmokeTest,
                "--responsiveness-test" => options.mode = RunMode::ResponsivenessTest,
                "--screenshot" => {
                    let path = args.next().ok_or("--screenshot needs a file name")?;
                    options.mode = RunMode::Screenshot(path.into());
                }
                "--open-last-save" => options.open_last_save = true,
                "--import" => {
                    let path = args.next().ok_or("--import needs a file name")?;
                    options.import.push(path.into());
                }
                "--cheat-sheet" => {
                    let path = args.next().ok_or("--cheat-sheet needs a file name")?;
                    options.cheat_sheet = Some(path.into());
                }
                other if other.starts_with("--") => {
                    if !options.automation.take(other, &mut args)? {
                        return Err(format!("unknown option {other}"));
                    }
                }
                path => options.project = Some(path.into()),
            }
        }
        Ok(options)
    }
}

/// The next argument read as a number, or `needs` as the error.
fn number<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>, needs: &str) -> Result<T, String> {
    args.next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| needs.to_string())
}

impl Automation {
    /// Takes one scripting option and the values it needs from `args`;
    /// `Ok(false)` when `arg` is not one of them.
    fn take(&mut self, arg: &str, args: &mut impl Iterator<Item = String>) -> Result<bool, String> {
        match arg {
            "--play" => self.play = true,
            "--media-drawer" => self.media_drawer = true,
            "--watch-folder" => {
                let folder = args.next().ok_or("--watch-folder needs a folder")?;
                self.watch_folders.push(folder.into());
            }
            "--find" => {
                let words = args.next().ok_or("--find needs words to find")?;
                self.media_drawer = true;
                self.find = Some(words);
            }
            "--export-page" => self.export_page = true,
            "--export-queue" => self.export_queue = true,
            "--close-request" => self.close_request = true,
            "--page" => {
                let name = args.next().unwrap_or_default();
                let page = ui_kit::Page::ALL
                    .into_iter()
                    .find(|p| p.label().eq_ignore_ascii_case(&name))
                    .ok_or("--page needs library, edit, color, audio or export")?;
                self.page = Some(page);
            }
            "--window-size" => {
                let size = args.next().unwrap_or_default();
                let (w, h) = size
                    .split_once('x')
                    .ok_or("--window-size needs WIDTHxHEIGHT, such as 1280x800")?;
                let (w, h): (f32, f32) = (
                    w.parse().map_err(|_| "--window-size needs a whole width")?,
                    h.parse().map_err(|_| "--window-size needs a whole height")?,
                );
                self.window_size = Some([w.max(960.0), h.max(600.0)]);
            }
            "--audio-tool" => {
                let name = args.next().unwrap_or_default();
                let tool = crate::audio_page::Tool::from_name(&name)
                    .ok_or("--audio-tool needs eq, compressor, clean-up, ducking, loudness or meters")?;
                self.audio.tool = Some(tool);
            }
            "--audio-band" => {
                let name = args.next().unwrap_or_default();
                let band = crate::audio_page::band::View::from_name(&name)
                    .ok_or("--audio-band needs clarity, spectrum, loudness, tracks or stereo")?;
                self.audio.band = Some(band);
            }
            "--audio-sizes" => {
                let text = args.next().unwrap_or_default();
                let sizes = crate::audio_page::start::sizes_named(&text)
                    .ok_or("--audio-sizes needs shares of the page's height: top,band or top,band,timeline, such as 0.3,0.35")?;
                self.audio.sizes = Some(sizes);
            }
            "--audio-view" => {
                let name = args.next().unwrap_or_default();
                let view = crate::audio_page::start::view_named(&name)
                    .ok_or("--audio-view needs timeline, mixer, split or stacked")?;
                self.audio.view = Some(view);
            }
            "--audio-track" => {
                self.audio.track = Some(number(args, "--audio-track needs a track's number, from 1")?);
            }
            "--audio-tracks" => {
                self.audio.tracks = Some(number(args, "--audio-tracks needs a number of tracks")?);
            }
            "--audio-clarity-region" => self.audio.clarity_region = true,
            "--library-tab" => {
                let name = args.next().ok_or("--library-tab needs a tab's name")?;
                self.library_tab = Some(name);
            }
            "--library-select" => {
                let name = args.next().ok_or("--library-select needs a file name")?;
                self.library_select = Some(name);
            }
            "--full-screen" => self.full_screen = true,
            "--select-first-clip" => self.select_first_clip = true,
            "--effects-drawer" => self.effects_drawer = true,
            "--sync-by-sound" => self.sync_by_sound = true,
            "--cpu-colour" => self.cpu_colour = true,
            "--graphics-warning" => self.graphics_warning = true,
            "--decoding-warning" => self.decoding_warning = true,
            "--action" => {
                let name = args.next().ok_or("--action needs a command name")?;
                self.actions.push(name);
            }
            "--then" => {
                let name = args.next().ok_or("--then needs a command name")?;
                self.then_actions.push(name);
            }
            "--settings" => {
                let id = args
                    .next()
                    .ok_or("--settings needs a tab's or a setting's id, such as proxies")?;
                self.settings = Some(id);
            }
            "--settings-search" => {
                let words = args.next().ok_or("--settings-search needs words to search for")?;
                self.settings_search = Some(words);
            }
            "--shortcuts-page" => {
                let state = args
                    .next()
                    .as_deref()
                    .and_then(ui_kit::keyboard::Preview::from_name)
                    .ok_or("--shortcuts-page needs filter, rebinding or clash")?;
                self.shortcuts_page = Some(state);
            }
            "--pointer" | "--drag-to" => {
                let pos = args
                    .next()
                    .as_deref()
                    .and_then(crate::fake_pointer::FakePointer::parse_pos)
                    .ok_or_else(|| format!("{arg} needs a position such as 640,420"))?;
                self.pointer = Some(match (arg, self.pointer) {
                    ("--drag-to", Some(pointer)) => crate::fake_pointer::FakePointer {
                        drag_to: Some(pos),
                        ..pointer
                    },
                    ("--drag-to", None) => return Err("--drag-to needs --pointer before it".into()),
                    _ => crate::fake_pointer::FakePointer::resting(pos),
                });
            }
            "--release" => match &mut self.pointer {
                Some(pointer) if pointer.drag_to.is_some() => pointer.release = true,
                _ => return Err("--release needs --drag-to before it".into()),
            },
            "--right-click" | "--double-click" | "--click" => match &mut self.pointer {
                Some(pointer) if pointer.drag_to.is_none() && arg == "--right-click" => {
                    pointer.right_click = true;
                }
                Some(pointer) if pointer.drag_to.is_none() && arg == "--click" => pointer.click = true,
                Some(pointer) if pointer.drag_to.is_none() => pointer.double_click = true,
                _ => return Err(format!("{arg} needs --pointer before it, and no --drag-to")),
            },
            "--scroll" => {
                let points: f32 = number(args, "--scroll needs a number of points")?;
                match &mut self.pointer {
                    Some(pointer) if pointer.drag_to.is_none() => pointer.scroll = points,
                    _ => return Err("--scroll needs --pointer before it, and no --drag-to".into()),
                }
            }
            "--hold-reading" => {
                self.holds.reading = Some(number(args, "--hold-reading needs a share between 0 and 1")?);
            }
            "--hold-proxy" => {
                self.holds.proxy = Some(number(args, "--hold-proxy needs a share between 0 and 1")?);
            }
            "--hold-package" => {
                self.holds.package = Some(number(args, "--hold-package needs a share between 0 and 1")?);
            }
            "--screenshot-wait" => {
                let seconds = args
                    .next()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .ok_or("--screenshot-wait needs a number of seconds")?;
                self.screenshot_wait = Some(seconds);
            }
            "--package-to" => {
                let folder = args.next().ok_or("--package-to needs a folder")?;
                self.package_to = Some(folder.into());
            }
            "--playhead" => self.playhead = Some(number(args, "--playhead needs seconds")?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// What is left of it when no project is open: most of it acts on the
    /// project (its page, clips, imports, holds), so only what is about
    /// the window stays (the pointer, the Settings window, the shortcuts
    /// page, the questions at start and the window's size).
    pub fn for_project(self, has_project: bool) -> Automation {
        if has_project {
            return self;
        }
        Automation {
            pointer: self.pointer,
            settings: self.settings,
            settings_search: self.settings_search,
            shortcuts_page: self.shortcuts_page,
            graphics_warning: self.graphics_warning,
            decoding_warning: self.decoding_warning,
            window_size: self.window_size,
            ..Automation::default()
        }
    }
}

/// The files to import, with each folder among them replaced by the files
/// directly in it, by name, hidden ones left out (`--import <folder>`: for
/// importing hundreds of files at once in a check, which would not fit on a
/// command line one by one). Files are kept as given, so a damaged or
/// unreadable one is refused by the import like any other; a folder that
/// can't be listed is kept as it is, for the import to say why.
pub fn expand_folders(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let listed = path.is_dir().then(|| std::fs::read_dir(path).ok()).flatten();
        let Some(entries) = listed else {
            out.push(path.clone());
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_to_import_stands_for_its_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.mp4", "a.wav", ".hidden.png"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("inner")).unwrap();
        std::fs::write(dir.path().join("inner").join("c.mp4"), b"x").unwrap();
        let single = PathBuf::from("single.mkv");
        let got = expand_folders(&[single.clone(), dir.path().to_path_buf()]);
        assert_eq!(got, [single, dir.path().join("a.wav"), dir.path().join("b.mp4")]);
    }

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
        assert!(parse(&["p.x", "--cpu-colour"]).unwrap().automation.cpu_colour);
        assert!(parse(&["p.x", "--export-page"]).unwrap().automation.export_page);
        assert!(parse(&["p.x", "--export-queue"]).unwrap().automation.export_queue);
        assert!(
            parse(&["p.x", "--close-request"])
                .unwrap()
                .automation
                .close_request
        );
        assert_eq!(
            parse(&["p.x", "--page", "Library"]).unwrap().automation.page,
            Some(ui_kit::Page::Library)
        );
        assert!(parse(&["p.x", "--page", "nowhere"]).is_err());
        assert!(parse(&["p.x", "--page"]).is_err());
        let o = parse(&["p.x", "--library-tab", "audio", "--library-select", "a.wav"]).unwrap();
        assert_eq!(
            (
                o.automation.library_tab.as_deref(),
                o.automation.library_select.as_deref()
            ),
            (Some("audio"), Some("a.wav"))
        );
        assert!(
            parse(&["p.x", "--select-first-clip"])
                .unwrap()
                .automation
                .select_first_clip
        );
        assert!(!parse(&["p.x"]).unwrap().automation.select_first_clip);
        assert!(
            parse(&["p.x", "--effects-drawer"])
                .unwrap()
                .automation
                .effects_drawer
        );
        assert!(
            parse(&["p.x", "--sync-by-sound"])
                .unwrap()
                .automation
                .sync_by_sound
        );
        assert!(!parse(&["p.x"]).unwrap().automation.cpu_colour);
        assert_eq!(
            parse(&["p.x", "--hold-reading", "0.4"])
                .unwrap()
                .automation
                .holds
                .reading,
            Some(0.4)
        );
        assert!(parse(&["p.x", "--hold-reading", "soon"]).is_err());
        assert_eq!(
            parse(&["p.x", "--screenshot-wait", "60"])
                .unwrap()
                .automation
                .screenshot_wait,
            Some(60.0)
        );
        assert!(parse(&["p.x", "--screenshot-wait", "-1"]).is_err());
        assert_eq!(
            parse(&["p.x", "--hold-proxy", "0.43"])
                .unwrap()
                .automation
                .holds
                .proxy,
            Some(0.43)
        );
        let packaged = parse(&["p.x", "--package-to", "out", "--hold-package", "0.4"]).unwrap();
        assert_eq!(packaged.automation.package_to, Some(PathBuf::from("out")));
        assert_eq!(packaged.automation.holds.package, Some(0.4));
        let o = parse(&["p.x", "--pointer", "1200,400", "--scroll", "400"]).unwrap();
        assert_eq!(o.automation.pointer.map(|p| p.scroll), Some(400.0));
        assert!(parse(&["p.x", "--scroll", "400"]).is_err());
        let o = parse(&["p.x", "--pointer", "640,420", "--drag-to", "640,480"]).unwrap();
        let pointer = o.automation.pointer.unwrap();
        assert_eq!(
            (pointer.at, pointer.drag_to),
            (egui::pos2(640.0, 420.0), Some(egui::pos2(640.0, 480.0)))
        );
        assert!(parse(&["p.x", "--drag-to", "640,480"]).is_err(), "from where?");
        assert!(parse(&["p.x", "--pointer", "640"]).is_err());
        assert!(!pointer.release && !pointer.right_click);
        let dragged = ["p.x", "--pointer", "640,420", "--drag-to", "640,480", "--release"];
        assert!(parse(&dragged).unwrap().automation.pointer.unwrap().release);
        assert!(
            parse(&["p.x", "--pointer", "640,420", "--release"]).is_err(),
            "of what?"
        );
        let o = parse(&["p.x", "--pointer", "40,300", "--right-click"]).unwrap();
        assert!(o.automation.pointer.unwrap().right_click);
        assert!(parse(&["p.x", "--right-click"]).is_err(), "where?");
        let o = parse(&["p.x", "--pointer", "40,300", "--double-click"]).unwrap();
        assert!(o.automation.pointer.unwrap().double_click);
        let o = parse(&["p.x", "--pointer", "900,610", "--click"]).unwrap();
        let pointer = o.automation.pointer.unwrap();
        assert!(pointer.click && !pointer.double_click);
        assert!(parse(&["p.x", "--click"]).is_err(), "where?");
        let both = [
            "p.x",
            "--pointer",
            "640,420",
            "--drag-to",
            "640,480",
            "--double-click",
        ];
        assert!(parse(&both).is_err(), "a drag or a double-click, not both");
        assert!(
            parse(&["--decoding-warning"])
                .unwrap()
                .automation
                .decoding_warning
        );
        assert_eq!(
            parse(&["--shortcuts-page", "clash"])
                .unwrap()
                .automation
                .shortcuts_page,
            Some(ui_kit::keyboard::Preview::Clash)
        );
        assert!(parse(&["--shortcuts-page", "sideways"]).is_err());
        assert_eq!(
            parse(&["--cheat-sheet", "keys.html"]).unwrap().cheat_sheet,
            Some("keys.html".into())
        );
        assert!(parse(&["--cheat-sheet"]).is_err());
    }

    #[test]
    fn scripting_is_kept_in_one_value_and_needs_a_project_for_most_of_it() {
        let args = [
            "--page",
            "audio",
            "--audio-tracks",
            "12",
            "--play",
            "--hold-reading",
            "0.4",
            "--pointer",
            "640,420",
            "--settings",
            "proxies",
            "--window-size",
            "1280x800",
            "--action",
            "Templates",
            "--graphics-warning",
        ];
        let o = parse(&args).unwrap();
        assert_eq!(
            (o.project.as_ref(), &o.import, o.mode.clone(), o.open_last_save),
            (None, &Vec::new(), RunMode::Normal, false),
            "nothing scripted lands outside the automation"
        );
        let a = o.automation;
        assert_eq!(
            (a.page, a.audio.tracks, a.play),
            (Some(ui_kit::Page::Audio), Some(12), true)
        );
        assert_eq!(
            a.holds,
            Holds {
                reading: Some(0.4),
                ..Holds::default()
            }
        );
        assert_eq!(
            a.clone().for_project(true),
            a,
            "with a project, all of it applies"
        );
        let windowed = a.clone().for_project(false);
        assert_eq!(
            windowed,
            Automation {
                pointer: a.pointer,
                settings: Some("proxies".into()),
                window_size: Some([1280.0, 800.0]),
                graphics_warning: true,
                ..Automation::default()
            },
            "without one, only what is about the window"
        );
    }
}
