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
pub const ALWAYS_UNLINKED: &str = "always-unlinked";
pub const DETAILED_GAIN_SELECTED: &str = "detailed-gain-selected";
pub const CLIP_PICTURES: &str = "clip-pictures";
pub const WAVEFORM_LOOK: &str = "waveform-look";
pub const WAVEFORM_COLOURS: &str = "waveform-colours";
pub const WAVEFORM_SIDE: &str = "waveform-side";
pub const WAVEFORM_FITTED: &str = "waveform-fitted";
pub const CLEAR_WAVEFORMS: &str = "clear-waveforms";
pub const PROXIES: &str = "proxies";
pub const JUMP_PICTURE: &str = "jump-picture";
pub const LOUDNESS_TARGET: &str = "loudness-target";
pub const EVERY_TRACK_COLUMN: &str = "every-track-column";
pub const MICROPHONE: &str = "microphone";
pub const RECORDING_CHANNEL: &str = "recording-channel";
pub const COUNT_IN: &str = "count-in";
pub const LIBRARY_MARKS: &str = "library-marks";
pub const WORKER_THREADS: &str = "worker-threads";
pub const RAM_CACHE: &str = "ram-cache";
pub const CARD_FRAMES: &str = "playback-memory";
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
                Setting::new(
                    ALWAYS_UNLINKED,
                    "Always unlink audio and video",
                    "Every clip on its own: a picture and each of its sound streams are clicked, moved and trimmed apart, in every project.",
                )
                .keywords("unlink link linked separate audio video sound tracks streams"),
            ],
        },
        Section {
            id: "timeline-pictures",
            title: "Timeline",
            settings: &[
                Setting::new(
                    CLIP_PICTURES,
                    "Clip pictures",
                    "How the small pictures inside video clips are taken from long recordings.",
                )
                .keywords("thumbnails pictures filmstrip quick exact key frame"),
                Setting::new(
                    WAVEFORM_LOOK,
                    "Waveform shape",
                    "How sound is drawn in clips on the timeline.",
                )
                .keywords("waveform audio sound clip look style shape plain smooth bars blocks"),
                Setting::new(
                    WAVEFORM_COLOURS,
                    "Waveform colours",
                    "The colours of clips with sound.",
                )
                .keywords("waveform colour color charcoal grey gray loudness map heatmap heat"),
                Setting::new(
                    WAVEFORM_SIDE,
                    "Waveform side",
                    "Centred on a middle line, or only the upper side standing on the clip's floor, with twice the room.",
                )
                .keywords("waveform centred centered half bottom one side mirrored"),
                Setting::new(
                    WAVEFORM_FITTED,
                    "Waveforms fitted to each recording",
                    "The recording's loudest moment reaches the clip's edge, so a quiet recording is still easy to see. Off, full scale does, so tracks can be compared by eye. Only the drawing changes, never the sound.",
                )
                .keywords("waveform scale fit normalise normalize loud flat decibel"),
                Setting::new(
                    DETAILED_GAIN_SELECTED,
                    "Detailed gain control on the selected audio clip",
                    "The track of the selected audio clip shows the large volume knob with its full scale, growing to fit it, and shrinks back when nothing on it is selected.",
                )
                .keywords("knob gain volume detailed fine track selected"),
            ],
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
                Setting::new(
                    CARD_FRAMES,
                    "Memory for playback",
                    "Graphics memory for frames decoded ahead of the playhead and kept behind it. More plays heavy stretches without a stall and keeps more to go back to.",
                )
                .keywords("vram gpu graphics memory buffer read ahead stutter smooth playback cache frames"),
            ],
        }],
    },
    Category {
        id: "audio",
        label: "Audio",
        sections: &[
            Section {
                id: "loudness",
                title: "Loudness",
                settings: &[Setting::new(
                    LOUDNESS_TARGET,
                    "Loudness to aim for",
                    "A line across the master column's loudness lane; I turns green within 1 LU of it.",
                )
                .keywords("lufs target master meter normalise normalize")],
            },
            Section {
                id: "track-columns",
                title: "Track columns",
                settings: &[Setting::new(
                    EVERY_TRACK_COLUMN,
                    "A column for every audio track",
                    "Every audio track gets its tall fader and meter beside the master column on the Edit page, as if pinned. The × on one takes it off.",
                )
                .keywords("pin pinned fader strip meter mixer column master ultrawide")],
            },
            Section {
                id: "voice-over",
                title: "Voice-over",
                settings: &[
                    Setting::new(
                        MICROPHONE,
                        "Microphone",
                        "What voice-overs are recorded with (Record a voice-over: on the audio target track, or a Voice-over track).",
                    )
                    .keywords("record recording mic input voice over voiceover narration commentary"),
                    Setting::new(
                        RECORDING_CHANNEL,
                        "Recording channel",
                        "A voice-over is one channel: the microphone's left, its right (a microphone on an interface's second input), or both mixed.",
                    )
                    .keywords("record mic input mono left right interface"),
                    Setting::new(
                        COUNT_IN,
                        "Count-in",
                        "Seconds counted down before the timeline plays and the take starts, with the microphone's level showing, to check it and get ready.",
                    )
                    .keywords("record countdown pre-roll preroll wait level check"),
                ],
            },
        ],
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
                    "Memory for frames that no longer fit in the graphics card's memory for playback: going back to them, or playing on into them, needs no decoding.",
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
        // Only the window changes settings here: with it closed there is
        // nothing to copy or compare each frame.
        let before = self.show_settings.then(|| self.settings.clone());
        let theme = self.theme;
        let mut open = self.show_settings;
        let mut problems = Vec::new();
        let mut clear_waveforms = false;
        let in_use = *self.jobs.budgets();
        // Asked of the system on a thread of its own, the first time
        // Settings opens; `None` until it answers.
        let microphones = if self.show_settings {
            self.microphones.names(ctx).map(<[String]>::to_vec)
        } else {
            None
        };
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
                    ALWAYS_UNLINKED => {
                        ui_kit::settings::toggle(row.ui, row.theme, &mut s.always_unlinked);
                    }
                    MICROPHONE => microphone_row(row.ui, &mut s.microphone, microphones.as_deref()),
                    RECORDING_CHANNEL => {
                        use crate::settings::RecordingChannel;
                        let options: Vec<_> = RecordingChannel::ALL.iter().map(|c| (*c, c.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.recording_channel, &options);
                    }
                    COUNT_IN => {
                        use crate::settings::CountIn;
                        let options: Vec<_> = CountIn::ALL.iter().map(|c| (*c, c.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.count_in, &options);
                    }
                    EVERY_TRACK_COLUMN => {
                        ui_kit::settings::toggle(row.ui, row.theme, &mut s.columns_for_every_track);
                    }
                    DETAILED_GAIN_SELECTED => {
                        ui_kit::settings::toggle(row.ui, row.theme, &mut s.detailed_gain_selected);
                    }
                    CLIP_PICTURES => {
                        let options: Vec<_> = ClipPictureWay::ALL.iter().map(|w| (*w, w.short())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.clip_pictures, &options);
                        row.describe(s.clip_pictures.explained());
                    }
                    WAVEFORM_LOOK => {
                        use ui_kit::timeline::wave_look::WaveStyle;
                        ui_kit::settings::dropdown(
                            row.ui,
                            WAVEFORM_LOOK,
                            &mut s.waveform_look,
                            &WaveStyle::ALL,
                            WaveStyle::words,
                        );
                        row.describe(s.waveform_look.explained());
                    }
                    WAVEFORM_COLOURS => {
                        use ui_kit::timeline::wave_look::WaveColouring;
                        let options: Vec<_> = WaveColouring::ALL.iter().map(|c| (*c, c.words())).collect();
                        ui_kit::settings::segmented(row.ui, row.theme, &mut s.waveform_colours, &options);
                        row.describe(s.waveform_colours.explained());
                    }
                    WAVEFORM_SIDE => {
                        use ui_kit::timeline::wave_look::WaveSide;
                        ui_kit::settings::segmented(
                            row.ui,
                            row.theme,
                            &mut s.waveform_side,
                            &[(WaveSide::Centred, "Centred"), (WaveSide::Half, "One side")],
                        );
                    }
                    WAVEFORM_FITTED => {
                        let mut fitted = !s.waveform_straight;
                        ui_kit::settings::toggle(row.ui, row.theme, &mut fitted);
                        s.waveform_straight = !fitted;
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
                    CARD_FRAMES => match self.viewer.card_memory_mb() {
                        Some((card, auto)) => {
                            let most = (card / 4 * 3).max(256);
                            limit(row, &mut s.resources.card_frames_mb, auto, 256..=most, " MB");
                            row.note(format!(
                                "Auto keeps {} of this card's {}",
                                gigabytes(auto),
                                gigabytes(card)
                            ));
                        }
                        None => row.disable("Video is decoded on the processor here: no frames are kept on the graphics card."),
                    },
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
        if let Some(before) = before.filter(|before| self.settings != *before) {
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
            if self.settings.resources.card_frames_mb != before.resources.card_frames_mb {
                self.viewer
                    .set_frames_kept(self.settings.resources.card_frames_mb);
            }
            if self.settings.library_marks != before.library_marks {
                self.library_marks_moved();
            }
            if self.settings.always_unlinked != before.always_unlinked {
                self.always_unlinked_changed();
            }
            let key_frame_first = self.settings.jump_picture.key_frame_first();
            self.viewer.set_key_frame_first(key_frame_first);
            self.display.set_key_frame_first(key_frame_first);
            self.pictures
                .set_exact(self.settings.clip_pictures == ClipPictureWay::Exact);
        }
    }
}

/// The microphone voice-overs are recorded with: the system's default, or
/// one by name. A name no longer offered (unplugged) stays chosen, so it
/// is used again once it is back; until then recording uses the default.
fn microphone_row(ui: &mut egui::Ui, chosen: &mut Option<String>, offered: Option<&[String]>) {
    const DEFAULT: &str = "The system's default";
    let shown = chosen.clone().unwrap_or_else(|| DEFAULT.to_owned());
    egui::ComboBox::from_id_salt("microphone")
        .selected_text(&shown)
        .width(260.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(chosen, None, DEFAULT);
            match offered {
                None => {
                    ui.label("Looking for microphones…");
                }
                Some([]) => {
                    ui.label("No microphone found");
                }
                Some(names) => {
                    for name in names {
                        ui.selectable_value(chosen, Some(name.clone()), name);
                    }
                }
            }
            if let Some(name) = chosen.clone().filter(|n| !offered.is_some_and(|o| o.contains(n))) {
                ui.label(format!("{name} (not plugged in)"));
            }
        });
}

/// Megabytes in gigabytes, for reading ("5.0 GB").
fn gigabytes(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
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

    /// The tab every "Settings › …" in interface text (the editor's and
    /// ui-kit's, shown in the editor) names exists by that name. Comments
    /// may name a section; text the person reads names a tab.
    #[test]
    fn every_settings_place_named_in_interface_text_is_a_tab() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        let mut folders = vec![root.join("app/src"), root.join("ui-kit/src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() > 20, "found the sources");
        // Put together, so this test's own lines aren't read as text.
        let needle = ["Settings", " › "].concat();
        let mut wrong = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for (at, _) in line.match_indices(needle.as_str()) {
                    let after = &line[at + needle.len()..];
                    let named = after.starts_with('{')
                        || CATEGORIES.iter().any(|c| {
                            after
                                .strip_prefix(c.label)
                                .is_some_and(|rest| !rest.starts_with(char::is_alphanumeric))
                        });
                    if !named {
                        wrong.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "no such Settings tab:\n{}", wrong.join("\n"));
    }

    #[test]
    fn a_picture_too_large_opens_settings_at_the_proxies() {
        let error = ui_kit::yuv_renderer::Refused::Size {
            luma: (16384, 8640),
            largest: 8192,
        }
        .error();
        let fix = error.fix_action().unwrap();
        let id = ui_kit::settings::open_at_id(&fix.command).unwrap();
        assert_eq!(id, PROXIES);
        let mut view = ui_kit::settings::SettingsView::default();
        assert!(view.open_at(CATEGORIES, id));
    }

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
