//! The search palette (Ctrl+K): commands, timeline labels, clips and
//! settings, matched by any part of their name.

use project::{ClipId, Sequence};
use ui_kit::{Action, ShortcutTable};

/// One thing search found.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Command { action: Action, keys: String },
    Label { name: String, at: f64 },
    Clip { id: ClipId, name: String, at: f64 },
    Settings,
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
            Hit::Settings => "Settings: theme, editing, resources".into(),
            Hit::TimelineSettings => "Timeline settings: picture size, frame rate, sound rate".into(),
            Hit::Shortcuts => "Keyboard shortcuts: every key, drawn; change them here".into(),
        }
    }
}

/// Most lines shown at once.
pub const SHOWN: usize = 30;

/// Everything matching `query` (any case), commands first. `clip_name`
/// names a clip by its file.
pub fn results(
    query: &str,
    shortcuts: &ShortcutTable,
    seq: Option<&Sequence>,
    clip_name: impl Fn(&project::Clip) -> String,
) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let matches = |text: &str| text.to_lowercase().contains(&q);
    let mut hits = Vec::new();
    for action in Action::ALL {
        if matches(action.label()) {
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
    let Some(seq) = seq else { return hits };
    for label in &seq.labels {
        if matches(&label.name) || matches("label") {
            hits.push(Hit::Label {
                name: label.name.clone(),
                at: label.at.as_secs_f64(),
            });
        }
    }
    // Picture clips only, so a file shows once rather than once per stream.
    let mut clips: Vec<&project::Clip> = seq.video_tracks.iter().flat_map(|t| t.clips.iter()).collect();
    if clips.is_empty() {
        clips = seq.audio_tracks.iter().flat_map(|t| t.clips.iter()).collect();
    }
    clips.sort_by_key(|c| c.start);
    for clip in clips {
        let name = clip_name(clip);
        if !q.is_empty() && matches(&name) {
            hits.push(Hit::Clip {
                id: clip.id,
                name,
                at: clip.start.as_secs_f64(),
            });
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, LabelColor, LabelId, Project, Time, TimelineLabel};

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
        let find = |q: &str| results(q, &table, p.active(), |_| "match.mkv".into());

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
            find("stop playing").contains(&Hit::Command {
                action: Action::Stop,
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
    fn commands_show_the_keys_they_have_now() {
        let mut table = ShortcutTable::default();
        let keys_of = |table: &ShortcutTable, query: &str| {
            results(query, table, None, |_| String::new())
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
}
