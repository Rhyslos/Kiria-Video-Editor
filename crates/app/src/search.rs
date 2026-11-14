//! The search palette (Ctrl+K): commands, the sections of Settings, the
//! project's timelines, timeline labels, clips and files, matched by any
//! part of their name. Files (and the clips that play them) are found by the
//! library index, so a file's labels and note find it too.
//!
//! The palette is a dialog (`ui_kit::dialog::palette`): while it is up its
//! field has the keys and nothing reaches the page behind it; Enter runs the
//! first result, Escape or a click outside closes it (the click goes no
//! further).

use crate::settings_sections::Section;
use project::{ClipId, MediaId, Sequence, SequenceId};
use ui_kit::{Action, ShortcutTable, Theme};

/// The name the palette asks the library index under.
pub const ASKER: library::Asker = "search palette";

/// Most files listed (the drawer shows them all).
const FILES_SHOWN: usize = 8;

/// One thing search found.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Command {
        action: Action,
        keys: String,
    },
    Label {
        name: String,
        at: f64,
    },
    Clip {
        id: ClipId,
        name: String,
        at: f64,
    },
    /// A file of the project: shown in the media drawer.
    File {
        id: MediaId,
        name: String,
    },
    Settings,
    /// A section of Settings: the window opens scrolled to it.
    Setting(Section),
    /// A timeline of the project: it opens.
    Timeline {
        id: SequenceId,
        name: String,
    },
    TimelineSettings,
    Shortcuts,
}

impl Hit {
    /// The line shown for it.
    pub fn text(&self, fps: f64) -> String {
        match self {
            Hit::Command { action, keys } => format!("{}    {keys}", action.label()).trim_end().to_owned(),
            Hit::Label { name, at } => {
                format!("Go to label: {name}    {}", ui_kit::timeline::timecode(*at, fps))
            }
            Hit::Clip { name, at, .. } => {
                format!("Go to clip: {name}    {}", ui_kit::timeline::timecode(*at, fps))
            }
            Hit::File { name, .. } => format!("Show file: {name}    in the media drawer"),
            Hit::Settings => "Settings: theme, editing, resources".into(),
            Hit::Setting(section) => format!("Settings › {}: {}", section.name(), section.about()),
            Hit::Timeline { name, .. } => format!("Go to timeline: {name}"),
            Hit::TimelineSettings => "Timeline settings: picture size, frame rate, sound rate".into(),
            Hit::Shortcuts => "Keyboard shortcuts: every key, drawn; change them here".into(),
        }
    }
}

/// Most lines shown at once.
pub const SHOWN: usize = 30;

/// Everything matching `query` (any case), commands first, then settings
/// and timelines. `timelines` are the project's (id and name; the open one
/// is left out by the caller). `clip_name` names a clip by its file.
/// `files` are the files the library index found for `query`, with their
/// names; until it has answered (`None`), clips are matched by their
/// file's name.
pub fn results(
    query: &str,
    shortcuts: &ShortcutTable,
    seq: Option<&Sequence>,
    timelines: &[(SequenceId, String)],
    clip_name: impl Fn(&project::Clip) -> String,
    files: Option<&[(MediaId, String)]>,
) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let matches = |text: &str| text.to_lowercase().contains(&q);
    let mut hits = Vec::new();
    for action in Action::ALL {
        if action.found_by(&q) {
            hits.push(Hit::Command {
                action: *action,
                keys: shortcuts.keys_for(*action),
            });
        }
    }
    for (hit, words) in [
        (Hit::Settings, "settings preferences theme"),
        (Hit::TimelineSettings, "timeline settings resolution frame rate"),
        (Hit::Shortcuts, "keyboard shortcuts keybinds hotkeys keys"),
    ] {
        if matches(words) {
            hits.push(hit);
        }
    }
    hits.extend(
        Section::ALL
            .into_iter()
            .filter(|s| s.matches(&q))
            .map(Hit::Setting),
    );
    for (id, name) in timelines {
        if !q.is_empty() && (matches(name) || "timeline".contains(&q)) {
            hits.push(Hit::Timeline {
                id: *id,
                name: name.clone(),
            });
        }
    }
    let Some(seq) = seq else {
        hits.extend(file_hits(&q, files));
        return hits;
    };
    for label in &seq.labels {
        if matches(&label.name) || matches("label") {
            hits.push(Hit::Label {
                name: label.name.clone(),
                at: label.at.as_secs_f64(),
            });
        }
    }
    // Nothing typed: no clip is found, so none is named (a name per clip
    // every frame on a big timeline is milliseconds).
    if q.is_empty() {
        hits.extend(file_hits(&q, files));
        return hits;
    }
    // Picture clips only, so a file shows once rather than once per stream.
    let mut clips: Vec<&project::Clip> = seq.video_tracks.iter().flat_map(|t| t.clips.iter()).collect();
    if clips.is_empty() {
        clips = seq.audio_tracks.iter().flat_map(|t| t.clips.iter()).collect();
    }
    clips.sort_by_key(|c| c.start);
    let found_files: Option<std::collections::HashSet<MediaId>> =
        files.map(|files| files.iter().map(|(id, _)| *id).collect());
    for clip in clips {
        let found = match &found_files {
            Some(found) => found.contains(&clip.media),
            None => matches(&clip_name(clip)),
        };
        if found {
            let name = clip_name(clip);
            hits.push(Hit::Clip {
                id: clip.id,
                name,
                at: clip.start.as_secs_f64(),
            });
        }
    }
    hits.extend(file_hits(&q, files));
    hits
}

fn file_hits(q: &str, files: Option<&[(MediaId, String)]>) -> Vec<Hit> {
    if q.is_empty() {
        return Vec::new();
    }
    files
        .unwrap_or_default()
        .iter()
        .take(FILES_SHOWN)
        .map(|(id, name)| Hit::File {
            id: *id,
            name: name.clone(),
        })
        .collect()
}

/// What the palette was answered with this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// A result was clicked, or Enter ran the first.
    Chosen(Hit),
    /// Escape, or a click outside.
    Closed,
}

/// The search palette for one frame: the field (it has the keys while the
/// palette is up), the hint, and up to [`SHOWN`] of `hits`.
pub fn palette(
    ctx: &egui::Context,
    theme: &Theme,
    text: &mut String,
    hits: &[Hit],
    fps: f64,
) -> Option<Answer> {
    let mut answer = None;
    let modal = ui_kit::dialog::palette(ctx, theme, "search-palette", |ui| {
        ui.set_width(520.0);
        let field = ui.add(
            egui::TextEdit::singleline(text)
                .hint_text("Search")
                .desired_width(f32::INFINITY),
        );
        field.request_focus();
        if ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(first) = hits.first()
        {
            answer = Some(Answer::Chosen(first.clone()));
        }
        ui.label(
            egui::RichText::new(
                "Commands, settings, timelines, timeline labels, clips and files (by name, label or note). Enter runs the first; Escape closes.",
            )
            .size(11.5)
            .color(theme.muted),
        );
        ui.add_space(4.0);
        for (i, hit) in hits.iter().take(SHOWN).enumerate() {
            if ui.selectable_label(i == 0, hit.text(fps)).clicked() {
                answer = Some(Answer::Chosen(hit.clone()));
            }
        }
        if hits.len() > SHOWN {
            ui.label(
                egui::RichText::new(format!("{} more: type more of the name.", hits.len() - SHOWN))
                    .color(theme.muted),
            );
        }
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(Answer::Closed);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, LabelColor, LabelId, Project, Time, TimelineLabel};

    #[test]
    fn with_nothing_typed_no_clip_is_named() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let p = Project::new_automatic("T");
        let sequence = p.active_sequence;
        let media = media::describe(&fixture).unwrap();
        let p = Command::Import { sequence, media }.apply(&p).unwrap();
        let table = ShortcutTable::default();
        let named = std::cell::Cell::new(0);
        let name = |_: &project::Clip| {
            named.set(named.get() + 1);
            "game-3-audio.mkv".to_owned()
        };
        let hits = results("", &table, p.active(), &[], name, None);
        assert!(!hits.iter().any(|h| matches!(h, Hit::Clip { .. })));
        assert_eq!(named.get(), 0, "named only when something is typed");
        let hits = results("game", &table, p.active(), &[], name, None);
        assert!(hits.iter().any(|h| matches!(h, Hit::Clip { .. })));
    }

    #[test]
    fn finds_commands_labels_clips_and_settings() {
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let p = Command::AddLabel {
            sequence: seq,
            label: TimelineLabel {
                id: LabelId(0),
                name: "Big fight".into(),
                at: Time::seconds(12),
                color: LabelColor::Red,
            },
        }
        .apply(&p)
        .unwrap();
        let table = ShortcutTable::default();
        let find = |q: &str| results(q, &table, p.active(), &[], |_| "match.mkv".into(), None);

        assert!(find("undo").iter().any(|h| matches!(
            h,
            Hit::Command {
                action: Action::Undo,
                ..
            }
        )));
        assert!(find("fight").iter().any(|h| *h
            == Hit::Label {
                name: "Big fight".into(),
                at: 12.0
            }));
        assert!(
            find("label").iter().any(|h| matches!(h, Hit::Label { .. })),
            "\"label\" lists them all"
        );
        assert!(
            find("clear the region").contains(&Hit::Command {
                action: Action::ClearRegion,
                keys: String::new()
            }),
            "a command without a shortcut is found too"
        );
        let commands = |q: &str| -> Vec<Action> {
            find(q)
                .iter()
                .filter_map(|h| match h {
                    Hit::Command { action, .. } => Some(*action),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            commands("template"),
            [Action::Templates, Action::SaveTemplate],
            "both template commands, with their keys"
        );
        assert!(commands("intro").contains(&Action::Templates));
        assert!(find("theme").contains(&Hit::Settings));
        assert!(find("frame rate").contains(&Hit::TimelineSettings));
        assert!(find("hotkeys").contains(&Hit::Shortcuts));
        assert!(find("zzzz").is_empty());
    }

    #[test]
    fn files_the_index_found_and_their_clips_are_listed() {
        let mut p = Project::new_automatic("T");
        let media = project::MediaRef {
            id: project::MediaId(0),
            path: "rec/DJI_0042.mp4".into(),
            fingerprint: project::Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: project::MediaKind::Video,
            duration: Time::seconds(5),
            info: project::MediaInfo {
                container: String::new(),
                video: Some(project::VideoStream {
                    index: 0,
                    codec: "h264".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: project::Rate::FPS_60,
                    variable_frame_rate: false,
                    rotation: 0,
                }),
                audio: Vec::new(),
                stated_length: None,
            },
        };
        let sequence = p.active_sequence;
        p = Command::Import { sequence, media }.apply(&p).unwrap();
        let id = *p.media.keys().next().unwrap();
        let table = ShortcutTable::default();
        // "drone" is the file's label: only the index knows it.
        let files = [(id, "DJI_0042.mp4".to_owned())];
        let hits = results(
            "drone",
            &table,
            p.active(),
            &[],
            |_| "DJI_0042.mp4".into(),
            Some(&files),
        );
        assert!(
            hits.iter()
                .any(|h| matches!(h, Hit::Clip { name, .. } if name == "DJI_0042.mp4"))
        );
        assert!(hits.contains(&Hit::File {
            id,
            name: "DJI_0042.mp4".into()
        }));
        let before = results("drone", &table, p.active(), &[], |_| "DJI_0042.mp4".into(), None);
        assert!(
            !before
                .iter()
                .any(|h| matches!(h, Hit::Clip { .. } | Hit::File { .. })),
            "until the index answers, clips match by name"
        );
    }

    #[test]
    fn commands_show_the_keys_they_have_now() {
        let mut table = ShortcutTable::default();
        let keys_of = |table: &ShortcutTable, query: &str| {
            results(query, table, None, &[], |_| String::new(), None)
                .into_iter()
                .find_map(|hit| match hit {
                    Hit::Command { keys, .. } => Some(keys),
                    _ => None,
                })
        };
        assert_eq!(keys_of(&table, "split at"), Some("S".into()));
        let split = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Split)
            .unwrap();
        table.swap(split, ui_kit::Shortcut::ctrl(egui::Key::B));
        assert_eq!(keys_of(&table, "split at"), Some("Ctrl+B".into()));
    }

    fn run(ctx: &egui::Context, events: Vec<egui::Event>, text: &mut String, hits: &[Hit]) -> Option<Answer> {
        let theme = ui_kit::ThemeName::Ash.theme();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..egui::RawInput::default()
        };
        let mut answer = None;
        ctx.run_ui(input, |ui| {
            answer = palette(ui.ctx(), &theme, text, hits, 60.0);
        })
        .drop_without_applying_deltas();
        answer
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        [true, false]
            .into_iter()
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect()
    }

    #[test]
    fn enter_runs_the_first_result_and_escape_closes() {
        let ctx = egui::Context::default();
        ui_kit::setup(&ctx, ui_kit::ThemeName::Ash);
        let table = ShortcutTable::default();
        let mut text = String::new();
        let hits = results("package", &table, None, &[], |_| String::new(), None);
        assert!(matches!(
            hits.first(),
            Some(Hit::Command {
                action: Action::PackageProject,
                ..
            })
        ));
        assert_eq!(run(&ctx, Vec::new(), &mut text, &hits), None);
        // Typed letters go into the field, never to the timeline behind.
        let mut typed = vec![egui::Event::Text("s ".into())];
        typed.extend(key(egui::Key::S));
        typed.extend(key(egui::Key::Space));
        let input = egui::RawInput {
            events: typed,
            ..egui::RawInput::default()
        };
        let theme = ui_kit::ThemeName::Ash.theme();
        let mut pressed = Vec::new();
        ctx.run_ui(input, |ui| {
            pressed = table.pressed(ui.ctx());
            palette(ui.ctx(), &theme, &mut text, &hits, 60.0);
        })
        .drop_without_applying_deltas();
        assert!(pressed.is_empty(), "no split, no play: {pressed:?}");
        assert_eq!(text, "s ");
        assert_eq!(
            run(&ctx, key(egui::Key::Enter), &mut text, &hits),
            Some(Answer::Chosen(hits[0].clone()))
        );
        assert_eq!(
            run(&ctx, key(egui::Key::Escape), &mut text, &hits),
            Some(Answer::Closed)
        );
    }

    #[test]
    fn a_click_outside_closes_it() {
        let ctx = egui::Context::default();
        ui_kit::setup(&ctx, ui_kit::ThemeName::Ash);
        let mut text = String::new();
        run(&ctx, Vec::new(), &mut text, &[]);
        run(&ctx, Vec::new(), &mut text, &[]);
        let at = egui::pos2(40.0, 700.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            run(
                &ctx,
                vec![egui::Event::PointerMoved(at), click(true)],
                &mut text,
                &[]
            ),
            None
        );
        assert_eq!(
            run(&ctx, vec![click(false)], &mut text, &[]),
            Some(Answer::Closed)
        );
    }

    #[test]
    fn settings_and_timelines_are_found() {
        let table = ShortcutTable::default();
        let timelines = [(SequenceId(4), "Main sequence".to_owned())];
        let find = |q: &str| results(q, &table, None, &timelines, |_| String::new(), None);
        let proxy = find("proxy");
        assert!(proxy.contains(&Hit::Setting(Section::Proxies)));
        assert!(proxy.contains(&Hit::Setting(Section::Cache)));
        assert!(find("main").contains(&Hit::Timeline {
            id: SequenceId(4),
            name: "Main sequence".into()
        }));
        assert!(
            find("timeline").iter().any(|h| matches!(h, Hit::Timeline { .. })),
            "\"timeline\" lists them all"
        );
    }
}
