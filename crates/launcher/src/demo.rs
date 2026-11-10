//! Sample projects for screenshots and scroll tests (`--demo <count>`).
//!
//! Everything goes into a fresh temporary folder, never the person's own
//! data: a project folder full of sample projects, a media folder, one
//! project whose file has gone missing and one damaged file.

use diag::{AppError, Explain};
use project::SequenceSettings;
use project_index::{FolderKind, Index};
use std::path::PathBuf;

const NAMES: [&str; 12] = [
    "Lofoten trip",
    "Ranked night",
    "Clutch moments",
    "Weekly highlights",
    "Settings tutorial",
    "Speedrun attempt 41",
    "LAN party",
    "Patch review",
    "Montage draft",
    "Final boss",
    "Stream cuts",
    "One-tap short",
];

const PRESETS: [SequenceSettings; 3] = [
    SequenceSettings::GAME_CLIP,
    SequenceSettings::HIGH_FRAME_RATE,
    SequenceSettings::VERTICAL_SHORT,
];

/// Makes the sample folder and returns the data folder to use.
/// With `crash`, the editor is made to look as if it crashed with unsaved
/// work in one of them, so the recovery window opens at start-up.
pub fn prepare(count: usize, crash: bool) -> diag::Result<PathBuf> {
    let root = std::env::temp_dir().join(format!(
        "{}-launcher-demo-{}",
        brand::DATA_DIR_NAME,
        std::process::id()
    ));
    let explain = || {
        AppError::new("The sample projects can't be made")
            .why(format!("Writing to {} failed.", root.display()))
            .fix("Check that the temporary folder has space.")
    };
    if root.exists() {
        std::fs::remove_dir_all(&root).or_explain(explain)?;
    }
    let data = root.join("data");
    let projects = root.join("Projects");
    let media = root.join("Recordings");
    for dir in [&data, &projects, &media] {
        std::fs::create_dir_all(dir).or_explain(explain)?;
    }
    let index = Index::open(&data.join(project_index::INDEX_FILE_NAME))?;
    index.add_folder(&projects, FolderKind::Projects)?;
    index.add_folder(&media, FolderKind::Media)?;
    crate::prefs::LauncherPrefs {
        default_project_folder: Some(projects.clone()),
    }
    .save(&data)?;

    let now = project_index::now();
    for i in 0..count {
        let name = match i / NAMES.len() {
            0 => NAMES[i % NAMES.len()].to_owned(),
            round => format!("{} {}", NAMES[i % NAMES.len()], round + 1),
        };
        let settings = PRESETS[i % PRESETS.len()];
        let path = crate::files::create_project(&projects, &name, settings)?;
        // Give the sample a length, as if it had clips on its timeline.
        let seconds = 45 + (i * 97) % 1500;
        let text = std::fs::read_to_string(&path).or_explain(explain)?;
        let text = text.replacen("length: (0, 1)", &format!("length: ({seconds}, 1)"), 1);
        std::fs::write(&path, text).or_explain(explain)?;
        index.record(&path)?;
        index.mark_opened(&path, now - (i as i64 * i as i64 * 1800 + 120))?;
    }
    if count > 2 {
        let gone = crate::files::create_project(&projects, "Old tournament", SequenceSettings::GAME_CLIP)?;
        index.record(&gone)?;
        index.mark_opened(&gone, now - 40 * 86_400)?;
        std::fs::remove_file(&gone).or_explain(explain)?;
        std::fs::write(
            projects.join(format!("Broken.{}", project::FILE_EXTENSION)),
            "not a project",
        )
        .or_explain(explain)?;
        // One project with three changes a crash left unsaved.
        let unsaved = crate::files::create_project(&projects, "Unsaved edit", SequenceSettings::GAME_CLIP)?;
        index.record(&unsaved)?;
        index.mark_opened(&unsaved, now - 60)?;
        let mut doc = project::Document::open(&unsaved, project::Recovery::Recover)?;
        let sequence = doc.project().active_sequence;
        for frame in [10, 20, 30] {
            doc.apply(project::Command::SetEditNeedle {
                sequence,
                at: project::Time::from_ratio(frame, 60).unwrap_or(project::Time::ZERO),
            })?;
        }
        drop(doc);
        if crash {
            std::fs::write(
                data.join("session.running"),
                format!("pid {}\nproject {}\n", u32::MAX - 7, unsaved.display()),
            )
            .or_explain(explain)?;
        }
    }
    Ok(data)
}
