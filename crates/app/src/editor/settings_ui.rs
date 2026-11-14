//! The editor's Settings window: what it holds (`CATEGORIES`, the tabs,
//! their cards and rows, as `ui_kit::settings` describes them) and each
//! row's control. The window itself, search and jumping to a setting are
//! `ui_kit::settings`, the same as the launcher's. Changes go into
//! `Editor::settings`, which the settings keeper writes once they settle.

use super::Editor;
use crate::cache_cleanup::{CLEAR_ALL_ROW, CLEAR_PROJECT_ROW, USED_ROW};
use crate::settings::{ClipPictureWay, JumpPicture, LibraryMarks};
use jobs::{BackgroundPolicy, Limit};
use ui_kit::settings::{Category, Section, Setting};

/// Rows of the editor's Settings, by id: what other parts open Settings at
/// (`Editor::open_settings_at`; the search's "proxy" finds `PROXIES`).
pub const THEME: &str = "theme";
pub const EDIT_AT_PLAYHEAD: &str = "edit-at-playhead";
pub const CLICK_MOVES: &str = "click-moves";
pub const CUT_CLOSES_GAP: &str = "cut-closes-gap";
pub const CLIP_PICTURES: &str = "clip-pictures";
pub const CLEAR_WAVEFORMS: &str = "clear-waveforms";
pub const PROXIES: &str = "proxies";
pub const JUMP_PICTURE: &str = "jump-picture";
pub const LOUDNESS_TARGET: &str = "loudness-target";
pub const LIBRARY_MARKS: &str = "library-marks";
pub const WORKER_THREADS: &str = "worker-threads";
pub const RAM_CACHE: &str = "ram-cache";
pub const BACKGROUND: &str = "background-work";
pub const DISK_CACHE: &str = "disk-cache";
pub const SHORTCUTS: &str = "keyboard-shortcuts";
pub const DIAGNOSTICS: &str = "diagnostics";
pub const LOG_FOLDER: &str = "log-folder";
/// The Resources tab (the top bar's resource readout opens it).
pub const RESOURCES: &str = "resources";

/// Everything in the editor's Settings, tab by tab.
pub const CATEGORIES: &[Category] = &[
    Category {
        id: "look",
        label: "Look",
        sections: &[Section {
            id: "appearance",
            title: "Appearance",
            settings: &[
                Setting::new(THEME, "Theme", "The colours of every window, the launcher's too.")
                    .keywords("ash carbon dark colour color appearance"),
            ],
        }],
    },
    Category {
        id: "editing",
        label: "Editing",
        sections: &[
        Section {
            id: "playhead-and-needle",
            title: "Playhead and edit needle",
            settings: &[
                Setting::new(
                    EDIT_AT_PLAYHEAD,
                    "Edit at the playhead",
                    "Merge the red edit needle into the playhead: edits happen where it is.",
                )
                .keywords("needle cursor insert"),
                Setting::new(
                    CLICK_MOVES,
                    "Clicking the timeline moves",
                    "What a click or drag in the ruler moves; the other is dragged by its handle.",
                )
                .keywords("ruler click needle playhead scrub"),
                Setting::new(
                    CUT_CLOSES_GAP,
                    "Cut closes the gap",
                    "Cut moves everything after the clips back on every track, as \"Delete and close the gap\" does. Off: it leaves the gap.",
                )
                .keywords("cut ripple gap ctrl+x"),
            ],
        },
        Section {
            id: "timeline-pictures",
            title: "Timeline",
            settings: &[Setting::new(
                CLIP_PICTURES,
                "Clip pictures",
                "How the small pictures inside video clips are taken from long recordings.",
            )
            .keywords("thumbnails pictures filmstrip quick exact key frame")],
        }],
    },
    Category {
        id: "playback",
        label: "Playback & proxies",
        sections: &[Section {
            id: "proxies-section",
            title: "Proxies",
            settings: &[
                Setting::new(
                    PROXIES,
                    "Proxies",
                    "Lighter copies of recordings that are quick to jump around in.",
                )
                .keywords("proxy smooth playback slow preview"),
                Setting::new(
                    JUMP_PICTURE,
                    "Jumping in a file without a proxy",
                    "What the viewer shows while the exact frame is decoded.",
                )
                .keywords("seek key frame keyframe exact preview"),
            ],
        }],
    },
    Category {
        id: "audio",
        label: "Audio",
        sections: &[Section {
            id: "loudness",
            title: "Loudness",
            settings: &[Setting::new(
                LOUDNESS_TARGET,
                "Loudness to aim for",
                "A line across the master column's loudness lane; I turns green within 1 LU of it.",
            )
            .keywords("lufs target master meter normalise normalize")],
        }],
    },
    Category {
        id: "library",
        label: "Library",
        sections: &[Section {
            id: "marks",
            title: "Marks",
            settings: &[Setting::new(
                LIBRARY_MARKS,
                "Keep in and out marks",
                "Where the Library keeps a file's marks and the levels chosen for its sound.",
            )
            .keywords("in out mark levels session project save")],
        }],
    },
    Category {
        id: RESOURCES,
        label: "Resources",
        sections: &[Section {
            id: "processor-and-memory",
            title: "Processor and memory",
            settings: &[
                Setting::new(
                    WORKER_THREADS,
                    "Worker threads",
                    "How many background tasks run at once.",
                )
                .keywords("cpu cores processor performance")
                .restart(),
                Setting::new(
                    RAM_CACHE,
                    "Memory for frames",
                    "How much memory decoded frames may take.",
                )
                .keywords("ram cache memory mb")
                .restart(),
                Setting::new(
                    BACKGROUND,
                    "Background work",
                    "When proxies, analysis and other background work may run.",
                )
                .keywords("idle pause playing performance")
                .restart(),
            ],
        }],
    },
    Category {
        id: "cache",
        label: "Cache",
        sections: &[
            Section {
                id: "cache-space",
                title: "Space on disk",
                settings: &[
                    Setting::new(
                        DISK_CACHE,
                        "Disk space for proxies",
                        "Older proxies go first when it is full.",
                    )
                    .keywords("disk cache gb size limit")
                    .restart(),
                    Setting::new(
                        USED_ROW,
                        "In use",
                        "Proxies are kept by recording and shared by projects.",
                    )
                    .keywords("disk cache size space"),
                ],
            },
            Section {
                id: "cache-clear",
                title: "Clearing",
                settings: &[
                    Setting::new(
                        CLEAR_PROJECT_ROW,
                        "This project's proxies",
                        "Deletes the proxies of this project's recordings. Asks first.",
                    )
                    .keywords("clear delete remove cache"),
                    Setting::new(
                        CLEAR_ALL_ROW,
                        "Every proxy",
                        "For every project. Recordings and project files are never touched.",
                    )
                    .keywords("clear delete remove cache all"),
                    Setting::new(
                        CLEAR_WAVEFORMS,
                        "Waveforms, clip pictures and loudness",
                        "Kept so recordings are not read again each time. Clearing them reads them again when needed; proxies stay.",
                    )
                    .keywords("clear delete remove cache waveforms peaks sound pictures thumbnails loudness lufs readings"),
                ],
            },
        ],
    },
    Category {
        id: "graphics",
        label: "Graphics",
        sections: &[graphics::SETTINGS],
    },
    Category {
        id: "keyboard",
        label: "Keyboard",
        sections: &[Section {
            id: "keyboard-section",
            title: "Shortcuts",
            settings: &[Setting::new(
                SHORTCUTS,
                "Keyboard shortcuts",
                "See every shortcut on a drawn keyboard, change them, print a cheat sheet.",
            )
            .keywords("keys hotkeys bindings layout cheat sheet")],
        }],
    },
    Category {
        id: "diagnostics-tab",
        label: "Diagnostics",
        sections: &[Section {
            id: "problems",
            title: "When something goes wrong",
            settings: &[
                Setting::new(
                    DIAGNOSTICS,
                    "Diagnostics",
                    "Every problem of this session, and what this machine is using.",
                )
                .keywords("errors problems report gpu"),
                Setting::new(
                    LOG_FOLDER,
                    "Log folder",
                    "Where the editor writes what it did, for a bug report.",
                )
                .keywords("logs path report bug"),
            ],
        }],
    },
    Category {
        id: "help",
        label: "Help",
        sections: &[ui_kit::user_guide::SETTINGS],
    },
];

impl Editor {
    /// Opens Settings at a setting, section or tab by its id (one of this
    /// file's constants, or a tab's id). Returns false for an unknown id.
    pub(crate) fn open_settings_at(&mut self, id: &str) -> bool {
        let found = self.settings_view.open_at(CATEGORIES, id);
        if found {
            self.show_settings = true;
        }
        found
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        // Measured when Settings opens, and again while it is open as
        // proxies are built or deleted, so the Cache rows stay true.
        let changes = self.proxies.changes();
        if self.show_settings && (!self.settings_were_open || changes != self.cache_seen) {
            self.cache_seen = changes;
            self.measure_cache();
        }
        self.settings_were_open = self.show_settings;
        let has_project = self.document.is_some();
        let before = self.settings.clone();
        let theme = self.theme;
        let mut open = self.show_settings;
        let mut problems = Vec::new();
        let mut clear_waveforms = false;
        let in_use = *self.jobs.budgets();
        ui_kit::settings::window(ctx, &theme, &mut open, |ui| {
            self.settings_view.show(ui, &theme, CATEGORIES, |row| {
                let s = &mut self.settings;
                match row.id() {
                    THEME => {
                        let options: Vec<_> = ui_kit::ThemeName::ALL.iter().map(|t| (*t, t.label())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.theme, &options);
                    }
                    EDIT_AT_PLAYHEAD => {
                        ui_kit::settings::toggle(row.ui, row.theme, &mut s.edit_at_playhead);
                    }
                    CUT_CLOSES_GAP => {
                        ui_kit::settings::toggle(row.ui, row.theme, &mut s.cut_closes_gap);
                    }
                    CLIP_PICTURES => {
                        let options: Vec<_> = ClipPictureWay::ALL.iter().map(|w| (*w, w.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.clip_pictures, &options);
                        row.describe(s.clip_pictures.explained());
                    }
                    CLEAR_WAVEFORMS => {
                        if ui_kit::settings::action(row.ui, "Clear").clicked() {
                            clear_waveforms = true;
                        }
                    }
                    CLICK_MOVES => {
                        if s.edit_at_playhead {
                            row.disable("With Edit at the playhead there is no separate needle: a click moves the playhead.");
                        }
                        ui_kit::settings::segmented(
                            row.ui,
                            row.theme,
                            &mut s.click_moves,
                            &[
                                (ui_kit::ClickMoves::Playhead, "Playhead"),
                                (ui_kit::ClickMoves::EditNeedle, "Edit needle"),
                            ],
                        );
                    }
                    PROXIES => {
                        let options: Vec<_> = cache::proxy::ProxyMode::ALL.iter().map(|m| (*m, m.label())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.proxies, &options);
                        row.describe(s.proxies.explained());
                    }
                    JUMP_PICTURE => {
                        let options: Vec<_> = JumpPicture::ALL.iter().map(|j| (*j, j.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.jump_picture, &options)
                            .on_hover_text(s.jump_picture.label());
                        row.describe(s.jump_picture.explained());
                    }
                    LOUDNESS_TARGET => {
                        ui_kit::settings::dropdown(
                            row.ui,
                            LOUDNESS_TARGET,
                            &mut s.loudness_target,
                            &ui_kit::master::LoudnessTarget::ALL,
                            ui_kit::master::LoudnessTarget::label,
                        );
                    }
                    LIBRARY_MARKS => {
                        let options: Vec<_> = LibraryMarks::ALL.iter().map(|m| (*m, m.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.library_marks, &options)
                            .on_hover_text(s.library_marks.label());
                        row.describe(s.library_marks.explained());
                    }
                    WORKER_THREADS => {
                        let most = self.machine.cpu_threads.max(2) - 1;
                        limit(row, &mut s.resources.worker_threads, most.min(4), 1..=most, "");
                        row.note(format!("In use now: {}", in_use.worker_threads));
                    }
                    RAM_CACHE => {
                        let most = (self.machine.total_ram_mb * 3 / 4).max(256);
                        limit(row, &mut s.resources.ram_cache_mb, 256.max(most / 3), 256..=most, " MB");
                        row.note(format!("In use now: {} MB", in_use.ram_cache_mb));
                    }
                    BACKGROUND => {
                        ui_kit::settings::segmented(
                            row.ui,
                            row.theme,
                            &mut s.resources.background,
                            &[
                                (BackgroundPolicy::PauseWhilePlaying, "Pause to play"),
                                (BackgroundPolicy::Always, "Always"),
                                (BackgroundPolicy::OnlyWhenIdle, "When idle"),
                            ],
                        );
                        row.describe(match s.resources.background {
                            BackgroundPolicy::PauseWhilePlaying => "Paused while the timeline plays, so playback gets the whole machine.",
                            BackgroundPolicy::Always => "Runs all the time, playing or not.",
                            BackgroundPolicy::OnlyWhenIdle => "Runs only while nobody is using the editor.",
                        });
                    }
                    DISK_CACHE => {
                        limit(row, &mut s.resources.disk_cache_gb, 20, 1..=500, " GB");
                        row.note(format!("In use now: {} GB", in_use.disk_cache_gb));
                    }
                    USED_ROW | CLEAR_PROJECT_ROW | CLEAR_ALL_ROW => self.cache_cleanup.row(row, has_project),
                    SHORTCUTS => {
                        if ui_kit::settings::action(row.ui, "Open…").clicked() {
                            self.shortcuts_page.get_or_insert_with(Default::default);
                        }
                    }
                    DIAGNOSTICS => {
                        if ui_kit::settings::action(row.ui, "Open…").clicked() {
                            self.show_diagnostics = true;
                        }
                    }
                    LOG_FOLDER => {
                        let folder = self.data_dir.join("logs").display().to_string();
                        if ui_kit::settings::action(row.ui, "Copy path").clicked() {
                            row.ui.ctx().copy_text(folder.clone());
                        }
                        row.note(folder);
                    }
                    ui_kit::user_guide::ROW => {
                        if let Some(err) = self.user_guide.settings_row(row) {
                            problems.push(err);
                        }
                    }
                    _ => {
                        if let Some(err) = self.graphics.settings_row(row) {
                            problems.push(err);
                        }
                    }
                }
            })
        });
        for err in &problems {
            self.errors.push(err);
        }
        if clear_waveforms {
            self.waveforms.clear_kept(&self.jobs);
            self.pictures.clear_kept(&self.jobs);
            self.library_page.clear_loudness(&self.jobs);
            self.status = "Waveforms, clip pictures and loudness readings cleared: they are read again from the recordings as they are needed"
                .into();
        }
        self.show_settings = open;
        let project_proxies = self.project_proxies();
        let shelf = self.proxies.shelf();
        if self
            .cache_cleanup
            .question(ctx, &self.theme, &self.jobs, &shelf, project_proxies)
        {
            self.proxies.clearing();
        }
        if self.settings != before {
            if self.settings.theme != before.theme {
                self.theme = self.settings.theme.theme();
                self.theme.apply(ctx);
            }
            self.proxies.set_mode(self.settings.proxies);
            // On asks for a proxy for every video file: those cleared on
            // purpose are built again (Auto leaves them cleared).
            if self.settings.proxies == cache::proxy::ProxyMode::On && before.proxies != self.settings.proxies
            {
                let back = self.proxies.switched_on();
                if back > 0 {
                    self.status = format!(
                        "Proxies On: building the {back} cleared from the cache again, one at a time"
                    );
                }
            }
            if self.settings.library_marks != before.library_marks {
                self.library_marks_moved();
            }
            let key_frame_first = self.settings.jump_picture.key_frame_first();
            self.viewer.set_key_frame_first(key_frame_first);
            self.display.set_key_frame_first(key_frame_first);
            self.pictures
                .set_exact(self.settings.clip_pictures == ClipPictureWay::Exact);
        }
    }
}

/// A resource limit as one control: Auto, or a number set by hand starting
/// at `start`.
fn limit<T: egui::emath::Numeric>(
    row: &mut ui_kit::settings::Row<'_>,
    limit: &mut Limit<T>,
    start: T,
    range: std::ops::RangeInclusive<T>,
    unit: &str,
) {
    let mut value = match limit {
        Limit::Auto => None,
        Limit::Manual(n) => Some(*n),
    };
    if ui_kit::settings::auto_number(row.ui, row.theme, &mut value, start, range, unit) {
        *limit = value.map_or(Limit::Auto, Limit::Manual);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_its_own_id_and_the_usual_words_find_it() {
        assert!(ui_kit::settings::repeated_ids(CATEGORIES).is_empty());
        let finds = |words: &str, id: &str| {
            ui_kit::settings::search(CATEGORIES, words)
                .iter()
                .filter_map(|p| p.setting(CATEGORIES))
                .any(|s| s.id == id)
        };
        assert!(finds("proxy", PROXIES));
        assert!(finds("theme", THEME));
        assert!(finds("dark", THEME));
        assert!(finds("lufs", LOUDNESS_TARGET));
        assert!(finds("vulkan", "graphics-system"));
        assert!(finds("shortcuts", SHORTCUTS));
        assert!(finds("clear cache", CLEAR_ALL_ROW));
        assert!(finds("threads", WORKER_THREADS));
        assert!(finds("ripple", CUT_CLOSES_GAP));
        assert!(finds("thumbnails", CLIP_PICTURES));
        assert!(finds("clear waveforms", CLEAR_WAVEFORMS));
        for words in ["help", "guide", "manual", "docs"] {
            assert!(finds(words, ui_kit::user_guide::ROW), "{words}");
        }
        for section in crate::settings_sections::Section::ALL {
            let mut view = ui_kit::settings::SettingsView::default();
            assert!(view.open_at(CATEGORIES, section.id()), "{}", section.name());
        }
    }

    #[test]
    fn settings_open_at_any_of_their_rows() {
        for id in [
            THEME,
            PROXIES,
            WORKER_THREADS,
            DISK_CACHE,
            SHORTCUTS,
            RESOURCES,
            "graphics-system",
        ] {
            let mut view = ui_kit::settings::SettingsView::default();
            assert!(view.open_at(CATEGORIES, id), "{id}");
        }
        let mut view = ui_kit::settings::SettingsView::default();
        view.open_at(CATEGORIES, RESOURCES);
        assert_eq!(CATEGORIES[view.tab()].label, "Resources");
        view.open_at(CATEGORIES, PROXIES);
        assert_eq!(CATEGORIES[view.tab()].label, "Playback & proxies");
    }
}
