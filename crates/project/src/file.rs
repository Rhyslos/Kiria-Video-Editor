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
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 47;

/// Turns the text of a version `n` file into the text of a version `n + 1` file.
pub type Migration = fn(&str) -> diag::Result<String>;

/// `MIGRATIONS[i]` upgrades version `i + 1` to `i + 2`.
pub const MIGRATIONS: &[Migration] = &[
    v1_to_v2, v2_to_v3, v3_to_v4, v4_to_v5, v5_to_v6, v6_to_v7, v7_to_v8, v8_to_v9, v9_to_v10, v10_to_v11,
    v11_to_v12, v12_to_v13, v13_to_v14, v14_to_v15, v15_to_v16, v16_to_v17, v17_to_v18, v18_to_v19,
    v19_to_v20, v20_to_v21, v21_to_v22, v22_to_v23, v23_to_v24, v24_to_v25, v25_to_v26, v26_to_v27,
    v27_to_v28, v28_to_v29, v29_to_v30, v30_to_v31, v31_to_v32, v32_to_v33, v33_to_v34, v34_to_v35,
    v35_to_v36, v36_to_v37, v37_to_v38, v38_to_v39, v39_to_v40, v40_to_v41, v41_to_v42, v42_to_v43,
    v43_to_v44, v44_to_v45, v45_to_v46, v46_to_v47,
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

/// Format 21 adds blur and pixelate, over the whole picture or a box.
/// Older files have neither and read as they are.
fn v20_to_v21(text: &str) -> diag::Result<String> {
    set_format_version(text, 20, 21)
}

/// Format 22 adds the motion effects: zoom to a point, shake and smooth
/// pan. Older files have none and read as they are.
fn v21_to_v22(text: &str) -> diag::Result<String> {
    set_format_version(text, 21, 22)
}

/// Format 23 gives pictures a blend mode, how they mix with what is below
/// them. It defaults to over, which is how every older picture was drawn.
fn v22_to_v23(text: &str) -> diag::Result<String> {
    set_format_version(text, 22, 23)
}

/// Format 24 lets an effect's settings change over the clip: `keys` on
/// each effect, none when missing, which is how every older effect was.
fn v23_to_v24(text: &str) -> diag::Result<String> {
    set_format_version(text, 23, 24)
}

/// Format 25 adds a transition into a picture from the clip before it:
/// `transition` on each clip, a plain cut when missing, as every older
/// clip was.
fn v24_to_v25(text: &str) -> diag::Result<String> {
    set_format_version(text, 24, 25)
}

/// Format 26 adds titles: a clip kind (`Title`) and `title` on each clip,
/// none when missing, as every older clip was.
fn v25_to_v26(text: &str) -> diag::Result<String> {
    set_format_version(text, 25, 26)
}

/// Format 27 adds the Stabilise effect. Older files have none.
fn v26_to_v27(text: &str) -> diag::Result<String> {
    set_format_version(text, 26, 27)
}

/// Format 28 adds a picture's grade from the Color page: `grade` on each
/// clip, none when missing, as every older clip was.
fn v27_to_v28(text: &str) -> diag::Result<String> {
    set_format_version(text, 27, 28)
}

/// Format 29 adds curves to a grade's layers: `curves`, straight when
/// missing, as every older layer was.
fn v28_to_v29(text: &str) -> diag::Result<String> {
    set_format_version(text, 28, 29)
}

/// Format 30 adds where a grade's layer applies: `range` (a colour range)
/// and `mask` (a shape), none when missing, as every older layer was.
fn v29_to_v30(text: &str) -> diag::Result<String> {
    set_format_version(text, 29, 30)
}

/// Format 31 lets a timeline label cover a stretch (a labelled region):
/// `until`, none when missing, as every older label was a moment.
fn v30_to_v31(text: &str) -> diag::Result<String> {
    set_format_version(text, 30, 31)
}

/// Format 32 adds fades to a picture (`fades`, none when missing) and a
/// curve to each of a sound's fades (`fade_in_curve`, `fade_out_curve`,
/// the plain quarter sine every older fade had when missing).
fn v31_to_v32(text: &str) -> diag::Result<String> {
    set_format_version(text, 31, 32)
}

/// Format 33 adds a colour to a track (`colour`, the timeline's own when
/// missing).
fn v32_to_v33(text: &str) -> diag::Result<String> {
    set_format_version(text, 32, 33)
}

/// Format 34 adds a track's sync lock (`unsynced`, every track
/// sync-locked when missing, as every edit treated them before).
fn v33_to_v34(text: &str) -> diag::Result<String> {
    set_format_version(text, 33, 34)
}

/// Format 35 adds the Mirror, Rounded corners, Outline and Drop shadow
/// effects. Older files have none.
fn v34_to_v35(text: &str) -> diag::Result<String> {
    set_format_version(text, 34, 35)
}

/// Format 36 adds the Chroma key and Letterbox effects. Older files have
/// none.
fn v35_to_v36(text: &str) -> diag::Result<String> {
    set_format_version(text, 35, 36)
}

/// Format 37 adds the Black and white, Sepia, Duotone, Invert and
/// Chromatic aberration effects. Older files have none.
fn v36_to_v37(text: &str) -> diag::Result<String> {
    set_format_version(text, 36, 37)
}

/// Format 38 adds the Directional blur, Radial blur and Glitch effects.
/// Older files have none.
fn v37_to_v38(text: &str) -> diag::Result<String> {
    set_format_version(text, 37, 38)
}

/// Format 39 adds the Lens distortion, Mosaic and Kaleidoscope effects.
/// Older files have none.
fn v38_to_v39(text: &str) -> diag::Result<String> {
    set_format_version(text, 38, 39)
}

/// Format 40 adds the Posterise, Halftone and Scanlines effects. Older
/// files have none.
fn v39_to_v40(text: &str) -> diag::Result<String> {
    set_format_version(text, 39, 40)
}

/// Format 41 adds the Light leak, Lens flare, Speed lines and Impact frame
/// effects. Older files have none.
fn v40_to_v41(text: &str) -> diag::Result<String> {
    set_format_version(text, 40, 41)
}

/// Format 42 adds the Slide in, Spin, Whip pan, Luma fade and Glitch
/// transitions. Older files have none.
fn v41_to_v42(text: &str) -> diag::Result<String> {
    set_format_version(text, 41, 42)
}

/// Format 43 gives a track's sound distortion, an echo and a reverb, all
/// off when missing.
fn v42_to_v43(text: &str) -> diag::Result<String> {
    set_format_version(text, 42, 43)
}

/// Format 44 adds the Motion blur effect and Zoom to a point's `sharpen`
/// (none when it is missing). Older files have neither.
fn v43_to_v44(text: &str) -> diag::Result<String> {
    set_format_version(text, 43, 44)
}

/// Format 45 adds a track's pitch shift (off when it is missing).
fn v44_to_v45(text: &str) -> diag::Result<String> {
    set_format_version(text, 44, 45)
}

/// Format 46 adds crossfades between sound clips (transitions of the
/// sound kinds on audio tracks). Older files have none.
fn v45_to_v46(text: &str) -> diag::Result<String> {
    set_format_version(text, 45, 46)
}

/// Format 47 adds a sound clip's own EQ and clean-up (`fx`, off when it
/// is missing). Older files have none.
fn v46_to_v47(text: &str) -> diag::Result<String> {
    set_format_version(text, 46, 47)
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
    /// `None` when the text read has no such field: a file from before
    /// format 2, or (in a head cut off at the project) one that keeps it
    /// after the project.
    #[serde(default, deserialize_with = "present")]
    journal_seq: Option<u64>,
}

/// A number written plainly (not as `Some(…)`), as a field that is there.
fn present<'de, D: serde::Deserializer<'de>>(number: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(number).map(Some)
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
    // The header is read once, from the text before the project, and the
    // project once.
    let header = parse_header(&text)?;
    let project = project_from(&text, header.format_version, FORMAT_VERSION, MIGRATIONS)?;
    Ok((project, header.journal_seq.unwrap_or(0)))
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
    let (head, _) = read_head(path)?;
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
/// its own, and whether it was cut off there. Files written by this app
/// always put the project last; for any other layout the whole text is
/// returned (not cut off).
fn read_head(path: &Path) -> diag::Result<(String, bool)> {
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
            return Ok((head, false));
        }
        if line.starts_with(PROJECT_FIELD) {
            head.push(')');
            return Ok((head, true));
        }
        head.push_str(&line);
    }
}

/// [`read_head`] for text already in memory: `None` when there is no
/// project field at the start of a line.
fn head_of(text: &str) -> Option<String> {
    let at = if text.starts_with(PROJECT_FIELD) {
        0
    } else {
        text.find(&format!("\n{PROJECT_FIELD}"))? + 1
    };
    Some(format!("{})", &text[..at]))
}

/// The header read from a head cut off at the project, when it has every
/// field the whole text would give (else the whole text must be read).
fn header_in_head(head: &str) -> Option<Header> {
    let header: Header = ron::from_str(head).ok()?;
    header.journal_seq.is_some().then_some(header)
}

/// How the project field starts in files this app writes (two-space indent).
const PROJECT_FIELD: &str = "  project: ";

/// The last autosave-journal entry included in the file, read from the
/// header only (the project is never read, unless the file keeps the
/// number after it).
pub fn read_journal_seq(path: &Path) -> diag::Result<u64> {
    let (head, cut) = read_head(path)?;
    if !cut {
        return Ok(parse_whole_header(&head)?.journal_seq.unwrap_or(0));
    }
    if let Some(header) = header_in_head(&head) {
        return Ok(header.journal_seq.unwrap_or(0));
    }
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be read").why(format!("{} could not be read.", path.display()))
    })?;
    Ok(parse_whole_header(&text)?.journal_seq.unwrap_or(0))
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

/// Copies a project file before an upgrade changes it, whole or not at all
/// (`disk::copy_atomic`). It is called while the file is still in format
/// `old_version` (before anything is written to it), so a whole copy of a
/// format `old_version` file already there is the true original and is
/// kept; one that isn't (cut short by a crash or a full disk when an
/// earlier version copied it in place) is made again from the file.
pub fn back_up_before_upgrade(path: &Path, old_version: u32) -> diag::Result<PathBuf> {
    let backup = backup_path(path, old_version);
    if !is_whole_backup(&backup, old_version) {
        crate::disk::copy_atomic(path, &backup).or_explain(|| {
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

/// Whether `backup` is there, says it is in format `old_version` and is a
/// whole file: its header reads (`read_info`) and its text closes every
/// bracket it opens and stops there ([`is_whole_ron`]). The project is not
/// parsed, migrated or checked, so this stays cheap for big projects, and a
/// sound copy is never thrown away because it fails a check added later.
fn is_whole_backup(backup: &Path, old_version: u32) -> bool {
    read_info(backup).is_ok_and(|info| info.format_version == old_version)
        && fs::read(backup).is_ok_and(|bytes| is_whole_ron(&bytes))
}

/// Whether `text` is one whole bracketed RON value: a `(` at its start,
/// whose matching `)` ends it (only white space after). Brackets inside
/// strings don't count. A copy cut short anywhere leaves a bracket open or
/// a string unclosed, so it never passes.
fn is_whole_ron(text: &[u8]) -> bool {
    let text = text.trim_ascii();
    if text.first() != Some(&b'(') {
        return false;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, &byte) in text.iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = match depth.checked_sub(1) {
                    Some(depth) => depth,
                    None => return false,
                };
                if depth == 0 {
                    return at + 1 == text.len();
                }
            }
            _ => {}
        }
    }
    false
}

/// Parses project file text, migrating it from an older format if needed.
pub fn from_str(text: &str) -> diag::Result<Project> {
    from_str_with(text, FORMAT_VERSION, MIGRATIONS)
}

/// Like [`from_str`] with an explicit target version and migration list (used by tests).
pub fn from_str_with(text: &str, current: u32, migrations: &[Migration]) -> diag::Result<Project> {
    project_from(text, parse_header(text)?.format_version, current, migrations)
}

/// The project in `text`, whose header says it is in format `version`.
fn project_from(text: &str, version: u32, current: u32, migrations: &[Migration]) -> diag::Result<Project> {
    let file: FileIn = versioned_from(text, version, current, migrations, &PROJECT)?;
    file.project.validate()?;
    Ok(file.project)
}

/// Runs migrations from `from` up to `to`, one version at a time.
pub fn migrate(text: &str, from: u32, to: u32, migrations: &[Migration]) -> diag::Result<String> {
    upgraded(text, from, to, migrations, &PROJECT).map(Cow::into_owned)
}

/// How one kind of versioned file (a project, a template, the labels for
/// every project) is named when it is refused: each keeps its own words.
pub(crate) struct FileKind {
    /// A file in format `version`, newer than `current`, the newest read.
    pub newer: fn(version: u32, current: u32) -> AppError,
    /// A file that can't be read as this kind.
    pub damaged: fn() -> AppError,
    /// No migration goes from format `version` to the next.
    pub no_step: fn(version: u32) -> AppError,
}

const PROJECT: FileKind = FileKind {
    newer: |version, current| {
        AppError::new("This project was saved by a newer version")
            .why(format!(
                "It uses file format {version}; this version reads up to format {current}."
            ))
            .fix("Update the app, then open the project again.")
    },
    damaged,
    no_step: |version| {
        AppError::new("This project can't be upgraded")
            .why(format!(
                "There is no upgrade from file format {version} to {}.",
                version + 1
            ))
            .fix("This is a bug in the app; please report it with the project file.")
    },
};

/// The format version every versioned file starts with.
#[derive(Deserialize)]
struct VersionHeader {
    format_version: u32,
}

/// Reads a versioned file of `kind` (a header with its `format_version`,
/// then its contents): refused when it is newer than `current`, upgraded
/// through `migrations` (`migrations[i]` takes format `i + 1` to `i + 2`)
/// when it is older, then read as `T`. Templates and the labels for every
/// project read their files with this; projects read their header their own
/// faster way, then go through [`versioned_from`] as well.
pub(crate) fn versioned_from_str<T: DeserializeOwned>(
    text: &str,
    current: u32,
    migrations: &[Migration],
    kind: &FileKind,
) -> diag::Result<T> {
    let version = ron::from_str::<VersionHeader>(text)
        .or_explain(kind.damaged)?
        .format_version;
    versioned_from(text, version, current, migrations, kind)
}

/// [`versioned_from_str`] for a file whose header says it is in format `version`.
fn versioned_from<T: DeserializeOwned>(
    text: &str,
    version: u32,
    current: u32,
    migrations: &[Migration],
    kind: &FileKind,
) -> diag::Result<T> {
    if version > current {
        return Err((kind.newer)(version, current));
    }
    let text = upgraded(text, version, current, migrations, kind)?;
    ron::from_str(&text).or_explain(kind.damaged)
}

/// `text` taken from format `from` up to `to`, one version at a time (as
/// it is when they are the same).
fn upgraded<'a>(
    text: &'a str,
    from: u32,
    to: u32,
    migrations: &[Migration],
    kind: &FileKind,
) -> diag::Result<Cow<'a, str>> {
    let mut text = Cow::Borrowed(text);
    for version in from..to {
        let step = version
            .checked_sub(1)
            .and_then(|i| migrations.get(i as usize))
            .ok_or_else(|| (kind.no_step)(version))?;
        text = Cow::Owned(step(&text)?);
    }
    Ok(text)
}

/// The header fields of a whole file's text, read from the text before the
/// project (see [`read_head`]) so a big project is not parsed for them. A
/// file laid out otherwise is read whole, as are files the head alone
/// doesn't answer for, so it gives what reading all of it gives.
fn parse_header(text: &str) -> diag::Result<Header> {
    if let Some(header) = head_of(text).as_deref().and_then(header_in_head) {
        return Ok(header);
    }
    parse_whole_header(text)
}

/// The header fields, reading all of `text`.
fn parse_whole_header(text: &str) -> diag::Result<Header> {
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
    fn the_journal_number_is_read_without_reading_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        save_with_journal_seq(&sample_project(), &path, 9).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let at = text.find(PROJECT_FIELD).unwrap();
        fs::write(&path, format!("{}{PROJECT_FIELD}%%% not read %%%\n", &text[..at])).unwrap();
        assert_eq!(read_journal_seq(&path).unwrap(), 9);
        assert_eq!(parse_header(&text).unwrap().format_version, FORMAT_VERSION);
    }

    #[test]
    fn header_fields_kept_after_the_project_are_still_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let project = sample_project();
        let body = ron::ser::to_string(&project).unwrap();
        let text =
            format!("(\n  format_version: {FORMAT_VERSION},\n{PROJECT_FIELD}{body},\n  journal_seq: 4,\n)\n");
        fs::write(&path, &text).unwrap();
        assert_eq!(read_journal_seq(&path).unwrap(), 4);
        assert_eq!(load_with_journal_seq(&path).unwrap(), (project.clone(), 4));
        let first =
            format!("(\n{PROJECT_FIELD}{body},\n  format_version: {FORMAT_VERSION},\n  journal_seq: 5,\n)\n");
        fs::write(&path, &first).unwrap();
        assert_eq!(read_journal_seq(&path).unwrap(), 5);
        assert_eq!(load_with_journal_seq(&path).unwrap(), (project, 5));
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

    /// `sample_project()` as a file of format `version`.
    fn text_of_format(version: u32) -> String {
        to_string(&sample_project()).unwrap().replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            &format!("format_version: {version}"),
            1,
        )
    }

    #[test]
    fn a_backup_is_made_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let original = text_of_format(FORMAT_VERSION - 1);
        fs::write(&path, &original).unwrap();
        let backup = back_up_before_upgrade(&path, FORMAT_VERSION - 1).unwrap();
        assert_eq!(
            backup,
            tmp.path()
                .join(format!("p.project.backup-v{}", FORMAT_VERSION - 1))
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        let names: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(names.len(), 2, "no temporary file is left");
    }

    #[test]
    fn a_good_backup_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let version = FORMAT_VERSION - 1;
        let original = text_of_format(version);
        fs::write(&path, &original).unwrap();
        let backup = back_up_before_upgrade(&path, version).unwrap();
        // The file changes but is still of that format (opened again by an
        // older version, say): the first copy is the true original.
        fs::write(&path, original.replace("Round trip", "Changed later")).unwrap();
        back_up_before_upgrade(&path, version).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    }

    #[test]
    fn a_backup_cut_short_is_made_again() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let version = FORMAT_VERSION - 1;
        let original = text_of_format(version);
        fs::write(&path, &original).unwrap();
        let backup = backup_path(&path, version);
        // A copy cut short by a crash or a full disk: the header is there,
        // the project is not whole.
        fs::write(&backup, &original[..original.len() - 20]).unwrap();
        back_up_before_upgrade(&path, version).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        // An empty one, and one of another format, too.
        for wrong in [String::new(), text_of_format(version - 1)] {
            fs::write(&backup, wrong).unwrap();
            back_up_before_upgrade(&path, version).unwrap();
            assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        }
    }

    #[test]
    fn a_whole_backup_failing_later_checks_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let version = FORMAT_VERSION - 1;
        let original = text_of_format(version);
        fs::write(&path, &original).unwrap();
        let backup = backup_path(&path, version);
        // A whole copy of that format whose project this version refuses
        // (as a stricter load check would): still the original, kept.
        let head = head_of(&original).unwrap();
        let refused = format!(
            "{}{PROJECT_FIELD}(not_a_project: \"a ) ] }} \\\" (\"),\n)\n",
            &head[..head.len() - 1]
        );
        assert!(from_str(&refused).is_err());
        fs::write(&backup, &refused).unwrap();
        back_up_before_upgrade(&path, version).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), refused);
    }

    #[test]
    fn whole_ron_is_told_from_text_cut_short() {
        for whole in [
            "(a: 1)",
            "(a: \"x)\\\"(\")\n",
            "  (a: [1, (2)], b: {\"k\": 3})\r\n",
        ] {
            assert!(is_whole_ron(whole.as_bytes()), "{whole}");
        }
        for cut in [
            "",
            "(",
            "(a: (1)",
            "(a: \"x)",
            "(a: 1))",
            "(a: 1) x",
            "a: 1)",
            "(a: \"\\\")",
        ] {
            assert!(!is_whole_ron(cut.as_bytes()), "{cut}");
        }
        // Every stored sample of an earlier format is whole; every way of
        // cutting one short is not.
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples");
        for version in 1..=FORMAT_VERSION {
            let text = fs::read(samples.join(format!("project-v{version}.ron"))).unwrap();
            assert!(is_whole_ron(&text), "format {version}");
            let end = text.trim_ascii_end().len();
            for len in (0..end).step_by(61).chain(end.saturating_sub(64)..end) {
                assert!(!is_whole_ron(&text[..len]), "format {version} cut at {len}");
            }
        }
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

    /// Projects, templates and the labels for every project read through
    /// one loader, and each is still refused in its own words.
    #[test]
    fn every_kind_of_versioned_file_keeps_its_own_refusals() {
        let newer = |kind: &FileKind| {
            versioned_from_str::<VersionHeader>("(format_version: 9)", 2, &[], kind)
                .err()
                .unwrap()
                .what()
                .to_owned()
        };
        let damaged = |kind: &FileKind| {
            versioned_from_str::<VersionHeader>("nonsense", 2, &[], kind)
                .err()
                .unwrap()
                .what()
                .to_owned()
        };
        let no_step = |kind: &FileKind| {
            versioned_from_str::<VersionHeader>("(format_version: 1)", 2, &[], kind)
                .err()
                .unwrap()
                .what()
                .to_owned()
        };
        assert_eq!(newer(&PROJECT), "This project was saved by a newer version");
        assert_eq!(damaged(&PROJECT), "This file is not a project, or it is damaged");
        assert_eq!(no_step(&PROJECT), "This project can't be upgraded");

        let template = crate::templates::from_str_with("(format_version: 9)", 2, &[]).unwrap_err();
        assert_eq!(template.what(), "This template was saved by a newer version");
        assert!(
            template.report().contains("template format 9"),
            "{}",
            template.report()
        );
        let template = crate::templates::from_str_with("(format_version: 1)", 2, &[]).unwrap_err();
        assert_eq!(template.what(), "This file is not a template, or it is damaged");

        let labels = crate::GlobalLabels::parse("(format_version: 2, labels: [])").unwrap_err();
        assert_eq!(
            labels.what(),
            "Labels for every project were saved by a newer version"
        );
        assert!(labels.report().contains("up to format 1"), "{}", labels.report());
        assert!(crate::GlobalLabels::parse("nonsense").is_err());
        assert!(crate::GlobalLabels::parse("(format_version: 1, labels: [])").is_ok());
    }

    #[test]
    fn a_version_that_needs_no_upgrade_is_read_as_it_is() {
        let read: VersionHeader = versioned_from_str("(format_version: 2)", 2, &[], &PROJECT).unwrap();
        assert_eq!(read.format_version, 2);
    }
}
