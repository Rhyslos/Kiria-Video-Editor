//! Reading and writing project files.
//!
//! A project file is RON text: a `format_version`, a small `summary` the
//! launcher can read without loading everything, then the project itself.
//! Files are written atomically (to a temporary file, then renamed over the
//! old one), so a crash mid-save never leaves a half-written project.
//!
//! When the format changes, [`FORMAT_VERSION`] goes up by one and a step is
//! added to [`MIGRATIONS`] that turns file text of the previous version into
//! the new one. So far every change has only added things: new fields have
//! defaults (`#[serde(default)]`), so each step just writes the new version
//! number and the old text reads as it is. Since format 18 values that are
//! their default are left out of the file (`crate::defaults`), which makes
//! each default part of the format. The first change that renames or
//! reshapes something will need a frozen copy of the old types to read the
//! old text into, convert and write again. Old files are migrated step by
//! step when opened; a sample of every released version lives in
//! `tests/samples/` and must keep opening.

use crate::model::{Project, SequenceSettings};
use crate::time::Time;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 20;

/// Turns the text of a version `n` file into the text of a version `n + 1` file.
pub type Migration = fn(&str) -> diag::Result<String>;

/// `MIGRATIONS[i]` upgrades version `i + 1` to `i + 2`.
pub const MIGRATIONS: &[Migration] = &[
    v1_to_v2, v2_to_v3, v3_to_v4, v4_to_v5, v5_to_v6, v6_to_v7, v7_to_v8, v8_to_v9, v9_to_v10, v10_to_v11,
    v11_to_v12, v12_to_v13, v13_to_v14, v14_to_v15, v15_to_v16, v16_to_v17, v17_to_v18, v18_to_v19,
    v19_to_v20,
];

/// Format 2 adds `info` (the file's streams) to each media entry. The field
/// has a default, so version 1 text is already valid version 2 text apart
/// from its version number.
fn v1_to_v2(text: &str) -> diag::Result<String> {
    set_format_version(text, 1, 2)
}

/// Format 3 adds `automatic` to each timeline. It defaults to off, which is
/// what every older timeline was (its settings were chosen up front).
fn v2_to_v3(text: &str) -> diag::Result<String> {
    set_format_version(text, 2, 3)
}

/// Format 4 adds `variable_frame_rate` to video streams. It defaults to off;
/// files imported before were treated as constant.
fn v3_to_v4(text: &str) -> diag::Result<String> {
    set_format_version(text, 3, 4)
}

/// Format 5 lists the media files in the summary, so project lists can say
/// which files are missing without reading the whole project. Empty until
/// the project is saved again.
fn v4_to_v5(text: &str) -> diag::Result<String> {
    set_format_version(text, 4, 5)
}

/// Format 6 adds `master_gain_db` to each timeline: the volume of the whole
/// mix. It defaults to 0 dB, which is how older timelines sounded.
fn v5_to_v6(text: &str) -> diag::Result<String> {
    set_format_version(text, 5, 6)
}

/// Format 7 adds `speed` to each clip. It defaults to normal speed, which
/// is how every older clip played.
fn v6_to_v7(text: &str) -> diag::Result<String> {
    set_format_version(text, 6, 7)
}

/// Format 8 adds `look` to each clip: where its picture sits, which part
/// shows, how strong, and its quick colour. It defaults to the picture as it
/// is, which is how every older clip was shown.
fn v7_to_v8(text: &str) -> diag::Result<String> {
    set_format_version(text, 7, 8)
}

/// Format 9 adds `sound` to each clip: its volume, pan, fade in and fade
/// out. It defaults to the sound as it is, which is how every older clip
/// sounded.
fn v8_to_v9(text: &str) -> diag::Result<String> {
    set_format_version(text, 8, 9)
}

/// Format 10 adds `keyframes` to each clip: how numbers of its look change
/// over time. It defaults to none, so every older clip looks the same all
/// along, as it did.
fn v9_to_v10(text: &str) -> diag::Result<String> {
    set_format_version(text, 9, 10)
}

/// Format 11 adds `ramp` (speed keyframes) and `blend` (frame blending) to
/// each clip. They default to no ramp and no blending, so every older clip
/// plays at its one speed and shows its recorded frames, as it did.
fn v10_to_v11(text: &str) -> diag::Result<String> {
    set_format_version(text, 10, 11)
}

/// Format 12 adds `kind` to each clip: a part of a file, another timeline
/// of the project (a nested timeline) or an adjustment layer. It defaults to
/// a part of a file, which every older clip was; several timelines were
/// already stored, so nothing else changes.
fn v11_to_v12(text: &str) -> diag::Result<String> {
    set_format_version(text, 11, 12)
}

/// Format 13 adds `library` to the project: labels on media files, custom
/// lists and notes. It defaults to none of them, which is all an older
/// project could have.
fn v12_to_v13(text: &str) -> diag::Result<String> {
    set_format_version(text, 12, 13)
}

/// Format 14 adds `stated_length` to a media file's streams: the length a
/// file whose end is missing says it has, when it was imported anyway. It
/// defaults to none: every older file was imported whole.
fn v13_to_v14(text: &str) -> diag::Result<String> {
    set_format_version(text, 13, 14)
}

/// Format 15 adds `uses` to the library: how each file is to be added from
/// the Library page (the part marked with in and out, a level per sound
/// stream), kept when the Library's marks are saved in the project. It
/// defaults to none: marks were never kept before.
fn v14_to_v15(text: &str) -> diag::Result<String> {
    set_format_version(text, 14, 15)
}

/// Format 16 adds `sound` to each track (its role, pan, effects and volume
/// and pan lines) and `master_sound` to each timeline (the mix's
/// compressor and limiter). Both default to every effect off, which is how
/// older projects sounded.
fn v15_to_v16(text: &str) -> diag::Result<String> {
    set_format_version(text, 15, 16)
}

/// Format 17 adds `lines` to each clip: its own volume and pan lines, at
/// moments of its file. They default to none, which is how every older
/// clip sounded.
fn v16_to_v17(text: &str) -> diag::Result<String> {
    set_format_version(text, 16, 17)
}

/// Format 18 leaves values out of the file when they are their default
/// (`crate::defaults`): an untouched clip is written as its place and its
/// file. Every value that may be left out reads as its default when
/// missing, so version 17 text, which has them all, reads as it is. The
/// version goes up so that an older app says the file is from a newer
/// version instead of calling it damaged.
fn v17_to_v18(text: &str) -> diag::Result<String> {
    set_format_version(text, 17, 18)
}

/// Format 19 gives picture clips `effects`, an ordered list. It defaults
/// to none, which is how every older clip looked.
fn v18_to_v19(text: &str) -> diag::Result<String> {
    set_format_version(text, 18, 19)
}

/// Format 20 adds three effects (film grain, sharpen, glow). Older files
/// have none of them and read as they are; the version goes up so that an
/// older app calls a file with one newer, not damaged.
fn v19_to_v20(text: &str) -> diag::Result<String> {
    set_format_version(text, 19, 20)
}

/// Rewrites the `format_version` number at the top of the file.
pub(crate) fn set_format_version(text: &str, from: u32, to: u32) -> diag::Result<String> {
    let key = "format_version";
    let start = text.find(key).ok_or_else(damaged)?;
    let after_key = start + key.len();
    let rest = &text[after_key..];
    let colon = rest.find(':').ok_or_else(damaged)?;
    let digits_start =
        after_key + colon + 1 + (rest[colon + 1..].len() - rest[colon + 1..].trim_start().len());
    let digits_len = text[digits_start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .count();
    let found: u32 = text[digits_start..digits_start + digits_len]
        .parse()
        .map_err(|_| damaged())?;
    if found != from {
        return Err(damaged());
    }
    Ok(format!(
        "{}{to}{}",
        &text[..digits_start],
        &text[digits_start + digits_len..]
    ))
}

/// The part of a project file the launcher reads to show a project card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub name: String,
    pub settings: SequenceSettings,
    /// Length of the open timeline.
    pub length: Time,
    /// The settings still wait for the first video (they are placeholders).
    #[serde(default)]
    pub automatic: bool,
    /// Where the project's media files are (format 5).
    #[serde(default)]
    pub media: Vec<PathBuf>,
}

impl Summary {
    pub fn of(project: &Project) -> Option<Summary> {
        let sequence = project.active()?;
        Some(Summary {
            name: project.name.clone(),
            settings: sequence.settings,
            length: sequence.length(),
            automatic: sequence.automatic,
            media: project.media.values().map(|m| m.path.clone()).collect(),
        })
    }
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    #[allow(dead_code)]
    summary: Option<Summary>,
    #[allow(dead_code)]
    #[serde(default)]
    journal_seq: u64,
    project: Project,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
    #[serde(default)]
    summary: Option<Summary>,
    #[serde(default)]
    journal_seq: u64,
}

/// Saves `project` to `path`, replacing any existing file only once the new one is complete.
pub fn save(project: &Project, path: &Path) -> diag::Result<()> {
    save_with_journal_seq(project, path, 0)
}

/// Like [`save`], also recording that autosave-journal entries up to
/// `journal_seq` are included, so recovery never applies them twice.
pub fn save_with_journal_seq(project: &Project, path: &Path, journal_seq: u64) -> diag::Result<()> {
    project.validate()?;
    let text = to_string_with_journal_seq(project, journal_seq)?;
    let explain_write = || {
        AppError::new("The project can't be saved")
            .why(format!("Writing to {} failed.", path.display()))
            .fix("Check that the disk is not full and that you can write to this folder.")
    };
    crate::disk::write_atomic(path, text.as_bytes()).or_explain(explain_write)
}

/// The file text for `project` in the current format.
pub fn to_string(project: &Project) -> diag::Result<String> {
    to_string_with_journal_seq(project, 0)
}

/// The file is a header of a line per field (what the launcher reads
/// without reading the rest: see [`read_head`]), then the project on one
/// line, without the spaces and line breaks of indenting: a third of the
/// size, quicker to write and read, and the same to every RON reader.
fn to_string_with_journal_seq(project: &Project, journal_seq: u64) -> diag::Result<String> {
    let explain =
        || AppError::new("The project can't be saved").why("It could not be turned into file text.");
    let summary = ron::ser::to_string(&Summary::of(project)).or_explain(explain)?;
    let body = ron::ser::to_string(project).or_explain(explain)?;
    Ok(format!(
        "(\n  format_version: {FORMAT_VERSION},\n  summary: {summary},\n  journal_seq: {journal_seq},\n{PROJECT_FIELD}{body},\n)\n"
    ))
}

/// Opens a project file, migrating it from an older format if needed.
pub fn load(path: &Path) -> diag::Result<Project> {
    Ok(load_with_journal_seq(path)?.0)
}

/// Like [`load`], also returning the last autosave-journal entry the file includes.
pub fn load_with_journal_seq(path: &Path) -> diag::Result<(Project, u64)> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be opened")
            .why(format!("{} could not be read.", path.display()))
            .fix("Check that the file still exists and that you can open it.")
    })?;
    let journal_seq = parse_header(&text)?.journal_seq;
    Ok((from_str(&text)?, journal_seq))
}

/// Makes the project's relative media paths full ones, from `folder` (the
/// project file's folder). A relative media path means "from the project
/// file's folder": a packaged project (`crate::package`) is written that
/// way, so it opens wherever its folder is moved. Everything that opens a
/// project to use its media goes through this ([`crate::Document::open`],
/// [`read_info`]); the model in memory always holds full paths, so a
/// project saved again writes full paths.
pub fn resolve_media_paths(project: &mut Project, folder: &Path) {
    let folder = full_folder(folder);
    let relative: Vec<_> = project
        .media
        .values()
        .filter(|m| m.path.is_relative())
        .map(|m| m.id)
        .collect();
    for id in relative {
        if let Some(media) = project.media.get_mut(&id) {
            media.path = folder.join(&media.path);
        }
    }
}

/// `folder` made absolute (the folder the app started in for an empty one),
/// without touching the disk.
fn full_folder(folder: &Path) -> PathBuf {
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf())
}

/// The folder a project file is in.
pub fn folder_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// What a project list needs to know about a file, read without loading the project.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    /// The format the file was written in. Higher than [`FORMAT_VERSION`] means a newer app saved it.
    pub format_version: u32,
    pub summary: Option<Summary>,
}

/// Reads only the start of the file (format version and summary), for project
/// lists. The project itself is never read, so this stays cheap for big projects.
pub fn read_info(path: &Path) -> diag::Result<FileInfo> {
    let head = read_head(path)?;
    let header: Header = ron::from_str(&head).or_explain(damaged)?;
    let mut summary = header.summary;
    if let Some(summary) = &mut summary {
        let folder = full_folder(folder_of(path));
        for media in &mut summary.media {
            if media.is_relative() {
                *media = folder.join(&*media);
            }
        }
    }
    Ok(FileInfo {
        format_version: header.format_version,
        summary,
    })
}

/// Reads only the summary block, for project lists. Cheap even for big projects.
pub fn read_summary(path: &Path) -> diag::Result<Option<Summary>> {
    Ok(read_info(path)?.summary)
}

/// The text before the `project:` field, closed off so it parses as a file on
/// its own. Files written by this app always put the project last; for any
/// other layout the whole text is returned.
fn read_head(path: &Path) -> diag::Result<String> {
    let explain = || {
        AppError::new("The project can't be read")
            .why(format!("{} could not be read.", path.display()))
            .fix("Check that the file still exists and that you can open it.")
    };
    let mut reader = BufReader::new(File::open(path).or_explain(explain)?);
    let mut head = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).or_explain(explain)? == 0 {
            return Ok(head);
        }
        if line.starts_with(PROJECT_FIELD) {
            head.push(')');
            return Ok(head);
        }
        head.push_str(&line);
    }
}

/// How the project field starts in files this app writes (two-space indent).
const PROJECT_FIELD: &str = "  project: ";

/// The last autosave-journal entry included in the file, read from the header only.
pub fn read_journal_seq(path: &Path) -> diag::Result<u64> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be read").why(format!("{} could not be read.", path.display()))
    })?;
    Ok(parse_header(&text)?.journal_seq)
}

/// Reads a project saved by a newer version, for looking at only: fields this
/// version doesn't know are skipped. Fails (explained) if the newer format
/// changed something this version can't skip.
pub fn load_newer(path: &Path) -> diag::Result<Project> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be opened")
            .why(format!("{} could not be read.", path.display()))
            .fix("Check that the file still exists and that you can open it.")
    })?;
    let version = parse_header(&text)?.format_version;
    let file: FileIn = ron::from_str(&text).or_explain(|| {
        AppError::new("This project was saved by a newer version")
            .why(format!(
                "It uses file format {version}, which changed too much for this version to show it."
            ))
            .fix("Update the app, then open the project again.")
    })?;
    file.project.validate()?;
    Ok(file.project)
}

/// Where the copy of a project is kept before it is upgraded from format
/// `old_version`: `<file>.backup-v<old_version>` beside it.
pub fn backup_path(path: &Path, old_version: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".backup-v{old_version}"));
    PathBuf::from(name)
}

/// Copies a project file before an upgrade changes it, unless a copy for that
/// format is already there (the first one is the true original).
pub fn back_up_before_upgrade(path: &Path, old_version: u32) -> diag::Result<PathBuf> {
    let backup = backup_path(path, old_version);
    if !backup.exists() {
        fs::copy(path, &backup).or_explain(|| {
            AppError::new("The project can't be upgraded safely")
                .why(format!(
                    "A copy of the original could not be made at {}.",
                    backup.display()
                ))
                .fix("Check that the disk is not full and that you can write to the project's folder.")
        })?;
    }
    Ok(backup)
}

/// Parses project file text, migrating it from an older format if needed.
pub fn from_str(text: &str) -> diag::Result<Project> {
    from_str_with(text, FORMAT_VERSION, MIGRATIONS)
}

/// Like [`from_str`] with an explicit target version and migration list (used by tests).
pub fn from_str_with(text: &str, current: u32, migrations: &[Migration]) -> diag::Result<Project> {
    let version = parse_header(text)?.format_version;
    if version > current {
        return Err(AppError::new("This project was saved by a newer version")
            .why(format!(
                "It uses file format {version}; this version reads up to format {current}."
            ))
            .fix("Update the app, then open the project again."));
    }
    let file: FileIn = if version == current {
        ron::from_str(text).or_explain(damaged)?
    } else {
        let upgraded = migrate(text, version, current, migrations)?;
        ron::from_str(&upgraded).or_explain(damaged)?
    };
    file.project.validate()?;
    Ok(file.project)
}

/// Runs migrations from `from` up to `to`, one version at a time.
pub fn migrate(text: &str, from: u32, to: u32, migrations: &[Migration]) -> diag::Result<String> {
    let mut text = text.to_owned();
    for version in from..to {
        let step = version
            .checked_sub(1)
            .and_then(|i| migrations.get(i as usize))
            .ok_or_else(|| {
                AppError::new("This project can't be upgraded")
                    .why(format!(
                        "There is no upgrade from file format {version} to {}.",
                        version + 1
                    ))
                    .fix("This is a bug in the app; please report it with the project file.")
            })?;
        text = step(&text)?;
    }
    Ok(text)
}

fn parse_header(text: &str) -> diag::Result<Header> {
    ron::from_str(text).or_explain(damaged)
}

fn damaged() -> AppError {
    AppError::new("This file is not a project, or it is damaged")
        .why("Its contents could not be read as a project.")
        .fix("Open the latest autosave or the backup copy next to the file.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> Project {
        let mut p = Project::new("Test", SequenceSettings::GAME_CLIP);
        p.name = "Round trip".into();
        p
    }

    #[test]
    fn save_then_load_gives_the_same_project() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let project = sample_project();
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap(), project);
        let names: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "the temporary file is renamed away");
    }

    #[test]
    fn the_project_is_written_on_one_line_and_files_written_indented_still_open() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let project = sample_project();
        save_with_journal_seq(&project, &path, 7).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().count(),
            6,
            "a line per header field, the project on one"
        );
        assert_eq!(load_with_journal_seq(&path).unwrap(), (project.clone(), 7));
        assert_eq!(read_journal_seq(&path).unwrap(), 7);
        // As files were written before: indented, the project over many lines.
        #[derive(Serialize)]
        struct Indented<'a> {
            format_version: u32,
            summary: Option<Summary>,
            journal_seq: u64,
            project: &'a Project,
        }
        let indented = ron::ser::to_string_pretty(
            &Indented {
                format_version: FORMAT_VERSION,
                summary: Summary::of(&project),
                journal_seq: 3,
                project: &project,
            },
            ron::ser::PrettyConfig::new().indentor("  ".to_owned()),
        )
        .unwrap();
        assert!(indented.len() > text.len(), "the one-line file is smaller");
        fs::write(&path, indented).unwrap();
        assert_eq!(load_with_journal_seq(&path).unwrap(), (project, 3));
        assert_eq!(read_summary(&path).unwrap().unwrap().name, "Round trip");
    }

    #[test]
    fn saving_replaces_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let mut project = sample_project();
        save(&project, &path).unwrap();
        project.name = "Second save".into();
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap().name, "Second save");
    }

    #[test]
    fn the_summary_is_readable_on_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        save(&sample_project(), &path).unwrap();
        let summary = read_summary(&path).unwrap().unwrap();
        assert_eq!(summary.name, "Round trip");
        assert_eq!(summary.settings, SequenceSettings::GAME_CLIP);
    }

    #[test]
    fn the_summary_is_read_without_reading_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let text = to_string(&sample_project()).unwrap();
        let at = text
            .find(PROJECT_FIELD)
            .expect("the project field is written last");
        // Everything after the summary is garbage: it must never be looked at.
        fs::write(&path, format!("{}{PROJECT_FIELD}%%% not read %%%\n", &text[..at])).unwrap();
        let info = read_info(&path).unwrap();
        assert_eq!(info.format_version, FORMAT_VERSION);
        assert_eq!(info.summary.unwrap().name, "Round trip");
    }

    #[test]
    fn the_version_number_is_rewritten_in_place() {
        let text = "(\n  format_version: 1,\n  summary: None,\n)";
        assert_eq!(
            set_format_version(text, 1, 2).unwrap(),
            "(\n  format_version: 2,\n  summary: None,\n)"
        );
        assert!(
            set_format_version(text, 3, 4).is_err(),
            "only from the expected version"
        );
        assert!(
            set_format_version("(format_version:1)", 1, 2)
                .unwrap()
                .contains("format_version:2")
        );
    }

    #[test]
    fn a_damaged_head_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        fs::write(&path, "not a project at all").unwrap();
        assert!(read_info(&path).unwrap_err().what().contains("damaged"));
    }

    #[test]
    fn a_backup_is_made_once_per_old_format() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        fs::write(&path, "original").unwrap();
        let backup = back_up_before_upgrade(&path, 1).unwrap();
        assert_eq!(backup, tmp.path().join("p.project.backup-v1"));
        fs::write(&path, "upgraded").unwrap();
        back_up_before_upgrade(&path, 1).unwrap();
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "original",
            "the first copy is kept"
        );
    }

    #[test]
    fn a_newer_file_with_unknown_fields_still_shows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let text = to_string(&sample_project())
            .unwrap()
            .replacen(
                &format!("format_version: {FORMAT_VERSION}"),
                &format!("format_version: {}", FORMAT_VERSION + 1),
                1,
            )
            .replacen("project: (", "future_field: 42,\n  project: (", 1);
        fs::write(&path, text).unwrap();
        assert!(load(&path).is_err(), "a normal open refuses");
        assert_eq!(load_newer(&path).unwrap().name, "Round trip");
    }

    #[test]
    fn newer_files_are_refused_with_an_explanation() {
        let text = to_string(&sample_project()).unwrap().replace(
            &format!("format_version: {FORMAT_VERSION}"),
            &format!("format_version: {}", FORMAT_VERSION + 1),
        );
        let err = from_str(&text).unwrap_err();
        assert!(err.what().contains("newer version"), "{}", err.report());
    }

    #[test]
    fn garbage_is_reported_as_damaged() {
        let err = from_str("this is not a project").unwrap_err();
        assert!(err.what().contains("damaged"));
    }

    #[test]
    fn migrations_run_in_order_from_the_file_version() {
        // Pretend the current format is 3 and this file is version 1. Each
        // step bumps the version and renames the project, so the result shows
        // both steps ran, in order.
        fn v1_to_v2(text: &str) -> diag::Result<String> {
            Ok(text
                .replace("format_version: 1", "format_version: 2")
                .replace("\"Round trip\"", "\"after step 1\""))
        }
        fn v2_to_v3(text: &str) -> diag::Result<String> {
            Ok(text
                .replace("format_version: 2", "format_version: 3")
                .replace("\"after step 1\"", "\"after step 2\""))
        }
        let text = to_string(&sample_project()).unwrap().replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            "format_version: 1",
            1,
        );
        let project = from_str_with(&text, 3, &[v1_to_v2, v2_to_v3]).unwrap();
        assert_eq!(project.name, "after step 2");

        let missing = from_str_with(&text, 3, &[v1_to_v2]).unwrap_err();
        assert!(missing.what().contains("can't be upgraded"));
    }
}
