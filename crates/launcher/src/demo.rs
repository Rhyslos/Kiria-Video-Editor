//! Sample projects for screenshots and scroll tests (`--demo <count>`).
//!
//! Everything goes into a fresh temporary folder, never the person's own
//! data: a project folder full of sample projects, a media folder, one
//! project whose file has gone missing and one damaged file. The first
//! sample has every kind of file a project keeps beside it. Deleting a
//! sample moves it to a folder there ([`bin`]), not to the recycle bin.

use diag::{AppError, Explain};
use project::SequenceSettings;
use project_index::{FolderKind, Index};
use std::path::{Path, PathBuf};

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

/// Where deleted sample projects go, for the data folder [`prepare`] returned.
pub fn bin(data_dir: &Path) -> crate::delete::Bin {
    let root = data_dir.parent().unwrap_or(data_dir);
    crate::delete::Bin::Folder(root.join("Deleted samples"))
}

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
        let path = crate::files::create_project(&projects, &name, Some(settings))?;
        // Give the sample a length, as if it had clips on its timeline.
        let seconds = 45 + (i * 97) % 1500;
        let text = std::fs::read_to_string(&path).or_explain(explain)?;
        let text = text.replacen("length: (0, 1)", &format!("length: ({seconds}, 1)"), 1);
        std::fs::write(&path, text).or_explain(explain)?;
        index.record(&path)?;
        index.mark_opened(&path, now - (i as i64 * i as i64 * 1800 + 120))?;
        // Most samples have a picture, as if saved in the editor; some don't yet.
        if i % 3 != 2 {
            sample_picture(&project::thumbnail_path(&path), i)?;
        }
        if i == 0 {
            sample_side_files(&path)?;
        }
    }
    if count > 2 {
        let gone =
            crate::files::create_project(&projects, "Old tournament", Some(SequenceSettings::GAME_CLIP))?;
        index.record(&gone)?;
        index.mark_opened(&gone, now - 40 * 86_400)?;
        std::fs::remove_file(&gone).or_explain(explain)?;
        std::fs::write(
            projects.join(format!("Broken.{}", project::FILE_EXTENSION)),
            "not a project",
        )
        .or_explain(explain)?;
        // One project with three changes a crash left unsaved.
        let unsaved = crate::files::create_project(&projects, "Unsaved edit", None)?;
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
            // An editor's marker whose process is gone: a crash.
            let markers = data.join("sessions");
            std::fs::create_dir_all(&markers).or_explain(explain)?;
            let pid = u32::MAX - 7;
            std::fs::write(
                markers.join(format!("{pid}.running")),
                format!("pid {pid}\nproject {}\n", unsaved.display()),
            )
            .or_explain(explain)?;
        }
    }
    Ok(data)
}

/// Gives a sample what a project gathers beside it over time: a backup from
/// before an upgrade, a timeline layout, two saved versions and the
/// autosave file.
fn sample_side_files(path: &Path) -> diag::Result<()> {
    let older = project::file::FORMAT_VERSION.saturating_sub(1);
    project::file::back_up_before_upgrade(path, older)?;
    std::fs::write(project::view_path(path), "(video_share: Some(0.5))").or_explain(|| {
        AppError::new("The sample projects can't be made")
            .why(format!("Writing beside {} failed.", path.display()))
            .fix("Check that the temporary folder has space.")
    })?;
    let project = project::file::load(path)?;
    for name in ["First cut", "Before the colour pass"] {
        project::versions::save(&project, path, name)?;
    }
    drop(project::Document::open(path, project::Recovery::Recover)?);
    Ok(())
}

/// A soft two-colour gradient standing in for a frame of the timeline.
fn sample_picture(out: &std::path::Path, seed: usize) -> diag::Result<()> {
    const HUES: [[u8; 3]; 6] = [
        [0x2E, 0x5C, 0x8A],
        [0x8A, 0x3E, 0x5C],
        [0x3E, 0x7A, 0x52],
        [0x9A, 0x6A, 0x2E],
        [0x5A, 0x3E, 0x8A],
        [0x2E, 0x7A, 0x7A],
    ];
    let (a, b) = (HUES[seed % HUES.len()], HUES[(seed + 2) % HUES.len()]);
    let (w, h) = (480u32, 270u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = (x + y) as f32 / (w + h) as f32;
            for c in 0..3 {
                rgba.push((f32::from(a[c]) * (1.0 - t) + f32::from(b[c]) * t) as u8);
            }
            rgba.push(255);
        }
    }
    image::save_buffer(out, &rgba, w, h, image::ExtendedColorType::Rgba8).map_err(|e| {
        AppError::new("The sample projects can't be made")
            .why(format!("Writing {} failed: {e}.", out.display()))
            .fix("Check that the temporary folder has space.")
    })
}
