//! Settings: what people change in the Settings window, saved between runs.

use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ui_kit::ThemeName;
use ui_kit::keyboard::{KeyboardChoice, LayoutId};

/// Reads `keyboard_layout` as older settings wrote it (a bare layout).
fn older_keyboard<'de, D: serde::Deserializer<'de>>(from: D) -> Result<Option<LayoutId>, D::Error> {
    LayoutId::deserialize(from).map(Some)
}

/// Everything in the Settings window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeName,
    pub resources: jobs::Budgets,
    /// Merge the red edit needle into the playhead.
    pub edit_at_playhead: bool,
    /// How sharp the viewer's picture is.
    pub preview_quality: ui_kit::PreviewQuality,
    /// The timeline in the rough cut view (sound folded into the pictures).
    pub rough_cut: bool,
    /// Every track of the timeline a thin strip (the Display menu's
    /// Compact tracks). The heights tracks were given are kept per project
    /// and come back when it is off.
    pub compact_tracks: bool,
    /// No divider between the timeline's video and audio tracks: one list
    /// that scrolls together (the Display menu's divider line, off).
    pub one_track_list: bool,
    /// What a click or drag in the timeline's ruler moves: the playhead
    /// (as always) or the edit needle, the playhead then moved by its handle.
    pub click_moves: ui_kit::ClickMoves,
    /// How the height is shared between the viewer and the timeline (the
    /// handle between them), on every page that has both.
    pub viewer_split: ui_kit::splitter::Split,
    /// The shortcuts that were changed on the keyboard shortcuts page (the
    /// usual ones are not written down).
    pub shortcuts: ui_kit::shortcuts::SavedBindings,
    /// The keyboard the shortcuts page draws and interface text writes keys
    /// for: the system's (the usual), or one picked on the page.
    pub keyboard: KeyboardChoice,
    /// The keyboard as settings kept it before the system was asked: read
    /// from older files, turned into `keyboard` on loading, never written.
    #[serde(skip_serializing, deserialize_with = "older_keyboard")]
    keyboard_layout: Option<LayoutId>,
    /// The selected clip's outline and handles on the picture in the viewer.
    /// Off unless switched on (Q, or "On the picture" in the inspector).
    pub picture_handles: bool,
    /// The Edit page's viewer shows the selected clip's own picture (Source)
    /// beside the timeline's (Program).
    pub split_view: bool,
    /// Where the viewer's own window was last, so it opens there again.
    pub viewer_window: Option<crate::viewer_window::WindowPlace>,
    /// Proxies Off, On or Auto (the usual): the same for every project.
    pub proxies: cache::proxy::ProxyMode,
    /// What a jump in a file without a proxy shows first.
    pub jump_picture: JumpPicture,
    /// The loudness the master column aims at (its right-click menu).
    pub loudness_target: ui_kit::master::LoudnessTarget,
    /// Where the Library page keeps a file's in and out marks and the
    /// levels chosen for its sound streams.
    pub library_marks: LibraryMarks,
    /// How fast exports went on this machine (measured by the newest of
    /// each kind), for the time the Export page says one will take.
    pub export_pace: crate::export_page::Pace,
    /// What the Audio page's bottom shows: the timeline, the mixer, or both.
    pub audio_bottom: crate::audio_page::Bottom,
    /// How the Audio page's Split lays them out: side by side, or stacked.
    pub audio_split: crate::audio_page::SplitWay,
    /// How the Audio page's loudness overview counts integrated loudness,
    /// range and peaks: every play, until reset, or in the region.
    pub loudness_counting: crate::audio_page::Counting,
}

/// Where the Library page keeps what is marked to add (Settings › Library).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LibraryMarks {
    /// For this session only: nothing is written to the project (the usual).
    #[default]
    Session,
    /// In the project, each change an undo step.
    Project,
}

impl LibraryMarks {
    pub const ALL: [LibraryMarks; 2] = [LibraryMarks::Session, LibraryMarks::Project];

    pub fn label(self) -> &'static str {
        match self {
            LibraryMarks::Session => "Keep marks for this session",
            LibraryMarks::Project => "Save marks in the project",
        }
    }

    pub fn explained(self) -> &'static str {
        match self {
            LibraryMarks::Session => {
                "In and out marks and the levels chosen for a file's sound last until the editor closes; the project file is not changed."
            }
            LibraryMarks::Project => {
                "In and out marks and the levels chosen for a file's sound are saved in the project, each change an undo step. Marks made this session are saved when you switch."
            }
        }
    }
}

/// What the viewer shows when the playhead jumps in a file that has no
/// proxy, while the exact frame is decoded (Settings › Proxies). Proxy
/// frames shown first are not affected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum JumpPicture {
    /// The key frame before the moment at once, then the exact frame (the usual).
    #[default]
    KeyFrameFirst,
    /// Nothing new until the exact frame is there.
    ExactOnly,
}

impl JumpPicture {
    pub const ALL: [JumpPicture; 2] = [JumpPicture::KeyFrameFirst, JumpPicture::ExactOnly];

    pub fn label(self) -> &'static str {
        match self {
            JumpPicture::KeyFrameFirst => "Show the nearest key frame first",
            JumpPicture::ExactOnly => "Wait for the exact frame",
        }
    }

    pub fn explained(self) -> &'static str {
        match self {
            JumpPicture::KeyFrameFirst => {
                "The key frame before the moment shows at once (up to a few seconds early), then the exact frame replaces it."
            }
            JumpPicture::ExactOnly => {
                "The picture stays as it is until the exact frame is decoded: never a wrong frame, but a jump can take a moment."
            }
        }
    }

    /// Whether the preview worker shows a key frame first.
    pub fn key_frame_first(self) -> bool {
        self == JumpPicture::KeyFrameFirst
    }
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.ron")
    }

    /// Reads settings; missing or unreadable settings fall back to the defaults.
    pub fn load(data_dir: &Path) -> (Settings, Option<AppError>) {
        let path = Settings::path(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str::<Settings>(&text) {
                Ok(settings) => (settings.upgraded(), None),
                Err(err) => (
                    Settings::default(),
                    Some(
                        AppError::new("Your settings could not be read")
                            .why("The settings file is damaged.")
                            .fix("The defaults are in use. Change any setting to write a fresh file.")
                            .source(err),
                    ),
                ),
            },
            Err(_) => (Settings::default(), None),
        }
    }

    /// Older settings made current. Before the system was asked, every file
    /// held a keyboard, the US one unless another was picked: a picked one
    /// stays picked, and US (most often never picked at all) becomes
    /// Automatic, which still draws US English on a US system.
    fn upgraded(mut self) -> Settings {
        if let Some(older) = self.keyboard_layout.take()
            && older != LayoutId::Us
            && self.keyboard == KeyboardChoice::Automatic
        {
            self.keyboard = KeyboardChoice::Fixed(older);
        }
        self
    }

    /// The settings file's text.
    pub fn to_text(&self) -> diag::Result<String> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(|| {
            AppError::new("Your settings could not be saved").why("They could not be turned into file text.")
        })
    }

    /// Writes the settings now, whole or not at all, on this thread. The
    /// editor keeps them with a [`SettingsKeeper`] instead.
    #[cfg(test)]
    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let path = Settings::path(data_dir);
        project::disk::write_atomic(&path, self.to_text()?.as_bytes()).or_explain(|| {
            AppError::new("Your settings could not be saved")
                .why(format!("Writing {} failed.", path.display()))
                .fix("Check that the disk is not full.")
        })
    }
}

/// While the pointer is held (a slider being dragged), settings are written
/// once they have stayed the same this long, or when it is let go.
pub const SETTLED_AFTER: Duration = Duration::from_millis(500);

/// After a failed write, the next try waits this long.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// Keeps `settings.ron` up to date without the interface thread waiting for
/// the disk or writing on every frame of a drag: the editor shows it the
/// settings every frame ([`SettingsKeeper::tick`]); a change is handed to a
/// [`project::disk::Keeper`] (whole-or-nothing writes on a thread of its
/// own, newest last) once the pointer is let go or the change has settled.
#[derive(Debug)]
pub struct SettingsKeeper {
    keeper: project::disk::Keeper,
    /// What the file holds.
    kept: Settings,
    /// Handed over, with its number, until it is on disk.
    writing: Option<(u64, Settings)>,
    /// The settings as last seen, and since when.
    seen: Settings,
    changed_at: Instant,
    /// No write before this, after a failure.
    wait_until: Option<Instant>,
}

impl SettingsKeeper {
    /// `settings` are what the file in `data_dir` holds (as loaded).
    pub fn new(data_dir: &Path, settings: &Settings, now: Instant) -> SettingsKeeper {
        SettingsKeeper {
            keeper: project::disk::Keeper::new(Settings::path(data_dir), "Your settings"),
            kept: settings.clone(),
            writing: None,
            seen: settings.clone(),
            changed_at: now,
            wait_until: None,
        }
    }

    /// Call every frame with the settings in use and whether the pointer is
    /// held. Hands a change over when due; returns why a write failed, once.
    pub fn tick(&mut self, settings: &Settings, now: Instant, pointer_held: bool) -> Option<AppError> {
        if *settings != self.seen {
            self.seen = settings.clone();
            self.changed_at = now;
        }
        if let Some((number, written)) = self.writing.take() {
            if self.keeper.written() >= number {
                self.kept = written;
            } else if self.keeper.busy() {
                self.writing = Some((number, written));
            }
        }
        let failed = self.keeper.take_failure();
        if failed.is_some() {
            self.wait_until = Some(now + RETRY_AFTER);
        }
        let settled = !pointer_held || now.duration_since(self.changed_at) >= SETTLED_AFTER;
        let waiting = self.wait_until.is_some_and(|until| now < until);
        let handed = self.writing.as_ref().map_or(&self.kept, |(_, s)| s);
        if settled && !waiting && settings != handed {
            self.hand_over(settings);
        }
        failed
    }

    fn hand_over(&mut self, settings: &Settings) {
        let text = settings.clone();
        let number = self.keeper.keep(move || text.to_text());
        self.writing = Some((number, settings.clone()));
        self.wait_until = None;
    }

    /// On closing: writes what is not written yet and waits for it.
    pub fn close(&mut self, settings: &Settings) -> diag::Result<()> {
        let handed = self.writing.as_ref().map_or(&self.kept, |(_, s)| s);
        if settings != handed {
            self.hand_over(settings);
        }
        let result = self.keeper.flush();
        if result.is_ok() {
            self.kept = settings.clone();
            self.writing = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies_are_auto_until_set_and_kept() {
        assert_eq!(Settings::default().proxies, cache::proxy::ProxyMode::Auto);
        let older: Settings = ron::from_str("(edit_at_playhead: true)").unwrap();
        assert_eq!(
            older.proxies,
            cache::proxy::ProxyMode::Auto,
            "older files read as Auto"
        );
        let set = Settings {
            proxies: cache::proxy::ProxyMode::On,
            ..Settings::default()
        };
        let text = ron::ser::to_string(&set).unwrap();
        assert_eq!(
            ron::from_str::<Settings>(&text).unwrap().proxies,
            cache::proxy::ProxyMode::On
        );
    }

    #[test]
    fn a_jump_shows_the_key_frame_first_until_set_otherwise_and_is_kept() {
        assert_eq!(Settings::default().jump_picture, JumpPicture::KeyFrameFirst);
        assert!(Settings::default().jump_picture.key_frame_first());
        let older: Settings = ron::from_str("(edit_at_playhead: true)").unwrap();
        assert_eq!(
            older.jump_picture,
            JumpPicture::KeyFrameFirst,
            "as before the choice"
        );
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            jump_picture: JumpPicture::ExactOnly,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        let loaded = Settings::load(tmp.path()).0;
        assert_eq!(loaded, settings);
        assert!(!loaded.jump_picture.key_frame_first());
    }

    #[test]
    fn the_ways_of_looking_at_tracks_are_off_until_turned_on_and_kept() {
        let usual = Settings::default();
        assert!(
            !usual.compact_tracks && !usual.one_track_list,
            "usual tracks, with the divider"
        );
        // Settings written before they existed read them as off.
        let old: Settings = ron::from_str("(rough_cut: true)").unwrap();
        assert!(!old.compact_tracks && !old.one_track_list);
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            compact_tracks: true,
            one_track_list: true,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
    }

    #[test]
    fn a_click_moves_the_playhead_until_set_otherwise() {
        assert_eq!(Settings::default().click_moves, ui_kit::ClickMoves::Playhead);
        let old: Settings = ron::from_str("(rough_cut: true)").unwrap();
        assert_eq!(
            old.click_moves,
            ui_kit::ClickMoves::Playhead,
            "settings from before the choice"
        );
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            click_moves: ui_kit::ClickMoves::EditNeedle,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
    }

    #[test]
    fn handles_on_the_picture_are_off_unless_switched_on() {
        assert!(!Settings::default().picture_handles);
        // Settings written before the switch existed read it as off.
        let old: Settings = ron::from_str("(edit_at_playhead: true)").unwrap();
        assert!(!old.picture_handles);
    }

    #[test]
    fn settings_round_trip_and_damage_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            theme: ThemeName::Carbon,
            edit_at_playhead: true,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);

        std::fs::write(Settings::path(tmp.path()), "not settings").unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert_eq!(loaded, Settings::default());
        assert!(problem.is_some());
    }

    #[test]
    fn settings_with_fewer_or_other_fields_still_load() {
        // The Graphics choice was kept here for a while; it now has its own file.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(tmp.path()),
            "(edit_at_playhead: true, graphics: Direct3D12)",
        )
        .unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(loaded.edit_at_playhead);
        assert_eq!(
            loaded.viewer_split,
            ui_kit::splitter::Split::default(),
            "settings from before the handle between viewer and timeline"
        );
        assert_eq!(
            ui_kit::ShortcutTable::restored(&loaded.shortcuts),
            ui_kit::ShortcutTable::default(),
            "settings from before shortcuts could be changed"
        );
    }

    #[test]
    fn split_view_and_the_viewers_window_are_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!Settings::default().split_view, "off until turned on");
        let settings = Settings {
            split_view: true,
            viewer_window: Some(crate::viewer_window::WindowPlace {
                x: 1930.0,
                y: 20.0,
                width: 1280.0,
                height: 720.0,
            }),
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
        // Settings from before either was kept still load.
        std::fs::write(Settings::path(tmp.path()), "(rough_cut: true)").unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(!loaded.split_view && loaded.viewer_window.is_none());
    }

    #[test]
    fn the_viewer_and_timeline_split_is_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            viewer_split: ui_kit::splitter::Split(0.7),
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0.viewer_split.0, 0.7);
    }

    #[test]
    fn changed_shortcuts_and_the_keyboard_are_remembered() {
        use ui_kit::{Action, Shortcut, ShortcutTable};
        let tmp = tempfile::tempdir().unwrap();
        let mut table = ShortcutTable::default();
        let split = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Split)
            .unwrap();
        // Split and Save trade places, and Undo loses its shortcut.
        table.swap(split, Shortcut::ctrl(egui::Key::S));
        let undo = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Undo)
            .unwrap();
        table.clear(undo);
        let settings = Settings {
            shortcuts: table.saved(),
            keyboard: KeyboardChoice::Fixed(LayoutId::Norwegian),
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();

        let text = std::fs::read_to_string(Settings::path(tmp.path())).unwrap();
        assert!(text.contains("\"Ctrl+S\""), "kept as readable text: {text}");
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert_eq!(loaded, settings);
        let restored = ShortcutTable::restored(&loaded.shortcuts);
        assert_eq!(restored, table);
        assert_eq!(
            restored.shortcut_for(Action::Split),
            Some(Shortcut::ctrl(egui::Key::S))
        );
        assert_eq!(
            restored.shortcut_for(Action::Save),
            Some(Shortcut::key(egui::Key::S))
        );
        assert_eq!(restored.shortcut_for(Action::Undo), None);
    }

    #[test]
    fn shortcuts_this_version_does_not_know_never_cost_the_other_settings() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(tmp.path()),
            "(edit_at_playhead: true, shortcuts: [(command: \"FlyToTheMoon\", nth: 0, keys: Some(\"Ctrl+M\")), (command: \"Split\", keys: Some(\"F9\"))])",
        )
        .unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(loaded.edit_at_playhead);
        let table = ui_kit::ShortcutTable::restored(&loaded.shortcuts);
        assert_eq!(
            table.shortcut_for(ui_kit::Action::Split),
            Some(ui_kit::Shortcut::key(egui::Key::F9))
        );
    }

    #[test]
    fn the_keyboard_is_automatic_unless_one_was_picked_before() {
        let tmp = tempfile::tempdir().unwrap();
        let load = |text: &str| {
            std::fs::write(Settings::path(tmp.path()), text).unwrap();
            let (loaded, problem) = Settings::load(tmp.path());
            assert!(problem.is_none(), "{text}");
            loaded
        };
        assert_eq!(load("()").keyboard, KeyboardChoice::Automatic);
        // Files from before: a picked keyboard stays picked; US becomes Automatic.
        let older = load("(edit_at_playhead: true, keyboard_layout: Norwegian)");
        assert_eq!(older.keyboard, KeyboardChoice::Fixed(LayoutId::Norwegian));
        assert!(older.edit_at_playhead);
        assert_eq!(load("(keyboard_layout: Us)").keyboard, KeyboardChoice::Automatic);
        // Written back, only the new field is there.
        older.save(tmp.path()).unwrap();
        let text = std::fs::read_to_string(Settings::path(tmp.path())).unwrap();
        assert!(text.contains("keyboard: Fixed(Norwegian)"), "{text}");
        assert!(!text.contains("keyboard_layout"), "{text}");
        assert_eq!(Settings::load(tmp.path()).0, older);
        assert_eq!(
            load("(keyboard: Fixed(Uk))").keyboard,
            KeyboardChoice::Fixed(LayoutId::Uk)
        );
    }

    #[test]
    fn a_missing_file_means_defaults_and_no_complaint() {
        let tmp = tempfile::tempdir().unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert_eq!(loaded, Settings::default());
        assert!(problem.is_none());
    }

    fn keeper_in(dir: &Path) -> (SettingsKeeper, Instant) {
        let now = Instant::now();
        (SettingsKeeper::new(dir, &Settings::default(), now), now)
    }

    fn written(dir: &Path) -> Settings {
        Settings::load(dir).0
    }

    #[test]
    fn a_dragged_slider_writes_once_when_let_go() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut keeper, start) = keeper_in(tmp.path());
        let mut settings = Settings::default();
        let mut handed = 0;
        // Sixty frames of a held drag, a new value on each.
        for frame in 0..60u32 {
            settings.resources.ram_cache_mb = jobs::Limit::Manual(1000 + u64::from(frame));
            assert!(
                keeper
                    .tick(
                        &settings,
                        start + Duration::from_millis(16 * u64::from(frame)),
                        true
                    )
                    .is_none()
            );
            handed += usize::from(keeper.writing.is_some());
        }
        assert_eq!(handed, 0, "nothing is written while it moves");
        assert!(!Settings::path(tmp.path()).exists());
        // Let go: written at once, and only the last value.
        keeper.tick(&settings, start + Duration::from_millis(16 * 60), false);
        keeper.keeper.flush().unwrap();
        assert_eq!(written(tmp.path()), settings);
        keeper.tick(&settings, start + Duration::from_secs(2), false);
        assert!(keeper.writing.is_none() && keeper.kept == settings);
    }

    #[test]
    fn a_held_change_is_written_once_it_settles() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut keeper, start) = keeper_in(tmp.path());
        let settings = Settings {
            edit_at_playhead: true,
            ..Settings::default()
        };
        keeper.tick(&settings, start, true);
        keeper.tick(&settings, start + Duration::from_millis(200), true);
        assert!(keeper.writing.is_none());
        keeper.tick(&settings, start + SETTLED_AFTER, true);
        assert!(keeper.writing.is_some(), "settled while still held");
        keeper.keeper.flush().unwrap();
        assert_eq!(written(tmp.path()), settings);
    }

    #[test]
    fn a_failed_write_is_reported_and_tried_again_later() {
        let tmp = tempfile::tempdir().unwrap();
        // A folder where the file goes: every write fails.
        std::fs::create_dir(Settings::path(tmp.path())).unwrap();
        std::fs::write(Settings::path(tmp.path()).join("x"), "").unwrap();
        let (mut keeper, start) = keeper_in(tmp.path());
        let settings = Settings {
            rough_cut: true,
            ..Settings::default()
        };
        keeper.tick(&settings, start, false);
        while keeper.keeper.busy() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let failed = keeper.tick(&settings, start + Duration::from_millis(20), false);
        assert!(failed.expect("reported").what().contains("settings"));
        assert!(keeper.writing.is_none(), "not again straight away");
        std::fs::remove_dir_all(Settings::path(tmp.path())).unwrap();
        keeper.tick(&settings, start + Duration::from_millis(40), false);
        assert!(keeper.writing.is_none());
        keeper.tick(&settings, start + RETRY_AFTER + Duration::from_secs(1), false);
        assert!(keeper.writing.is_some(), "tried again");
        keeper.close(&settings).unwrap();
        assert_eq!(written(tmp.path()), settings);
    }

    #[test]
    fn closing_writes_what_is_not_written_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut keeper, start) = keeper_in(tmp.path());
        let settings = Settings {
            compact_tracks: true,
            ..Settings::default()
        };
        keeper.tick(&settings, start, true);
        keeper.close(&settings).unwrap();
        assert_eq!(written(tmp.path()), settings);
        let names: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "no temporary file is left");
    }
}
