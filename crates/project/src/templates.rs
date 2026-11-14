//! Templates: a stretch of a timeline saved under a name, to drop into any
//! project later (an intro, an outro, a sting between parts).
//!
//! A template holds the clips of the stretch on every track, placed from
//! zero, and the media entries those clips use, exactly as a project stores
//! them. It lives in a small file of its own (`<name>.template`, RON with
//! its own `format_version`) in a folder the app picks; nothing about it is
//! kept in a project file. Dropping one in is an ordinary, undoable change
//! ([`crate::Command::InsertTemplate`]) that carries the whole template, so
//! the autosave journal can replay it even after the file is gone.

use crate::command::{insert_clip, missing, sequence_mut, sequence_ref, settle_automatic};
use crate::ids::{ClipId, LinkId, MediaId, SequenceId, TrackId};
use crate::model::{Clip, MediaKind, MediaRef, Project, Region, Sequence, Track, TrackKind};
use crate::time::Time;
use crate::track_sound::TrackSound;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The template format this build writes. It goes up whenever the file
/// changes shape, which includes a change to the model types it holds
/// (`Clip`, `Track`, `MediaRef`) that old text can't be read into.
pub const FORMAT_VERSION: u32 = 2;

/// File extension of a template, without the dot.
pub const FILE_EXTENSION: &str = "template";

/// Turns the text of a version `n` template into the text of version `n + 1`.
pub type Migration = fn(&str) -> diag::Result<String>;

/// `MIGRATIONS[i]` upgrades version `i + 1` to `i + 2`.
pub const MIGRATIONS: &[Migration] = &[v1_to_v2];

/// Template format 2 leaves values out when they are their default, as
/// project format 18 does (`crate::defaults`). Version 1 text has them all
/// and reads as it is; the version goes up so an older app says the
/// template is from a newer version instead of calling it damaged.
fn v1_to_v2(text: &str) -> diag::Result<String> {
    crate::file::set_format_version(text, 1, 2)
}

/// A stretch of a timeline, ready to be dropped into any project.
///
/// The ids inside are the template's own (they start at 1); inserting gives
/// everything new ids from the project it goes into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// How much room it takes on the timeline.
    pub length: Time,
    /// The files its clips use, as a project stores them.
    pub media: Vec<MediaRef>,
    /// Its video tracks from the first up, each with its clips placed from
    /// zero. A track's volume, and its name when someone chose it, are used
    /// when the project it goes into has no such track yet.
    pub video_tracks: Vec<Track>,
    pub audio_tracks: Vec<Track>,
}

impl Template {
    /// The clips of `range` on every track of a timeline (the whole timeline
    /// when `None`). A clip crossing an edge of the range is kept cut there.
    pub fn capture(project: &Project, sequence: SequenceId, range: Option<Region>) -> diag::Result<Template> {
        let seq = sequence_ref(project, sequence)?;
        let whole = range.is_none();
        let range = range.unwrap_or(Region {
            start: Time::ZERO,
            end: seq.length(),
        });
        let cut = |tracks: &imbl::Vector<Track>| {
            let mut kept: Vec<Track> = tracks
                .iter()
                .map(|t| Track {
                    // A nested timeline belongs to its project: it is left out.
                    clips: t
                        .clips
                        .iter()
                        .filter(|c| c.nested().is_none())
                        .filter_map(|c| cut_to(c, range))
                        .collect(),
                    // Switches are how the track was being worked on, not part of the stretch.
                    muted: false,
                    solo: false,
                    locked: false,
                    hidden: false,
                    ..t.clone()
                })
                .collect();
            // Tracks keep their place, so only empty ones after the last used one go.
            while kept.last().is_some_and(|t| t.clips.is_empty()) {
                kept.pop();
            }
            kept
        };
        let mut template = Template {
            length: range.length(),
            media: Vec::new(),
            video_tracks: cut(&seq.video_tracks),
            audio_tracks: cut(&seq.audio_tracks),
        };
        if template.clip_count() == 0 {
            return Err(AppError::new("There is nothing to save as a template")
                .why(if whole {
                    "The timeline has no clips yet."
                } else {
                    "The region has no clips in it."
                })
                .fix(if whole {
                    "Put the clips of your intro or outro on the timeline first."
                } else {
                    "Mark a region over the clips to save (Shift-drag in the ruler), or clear it to save the whole timeline."
                }));
        }
        template.media = project
            .media
            .values()
            .filter(|m| template.clips().any(|c| c.media == m.id))
            .cloned()
            .collect();
        template.renumber();
        template.validate()?;
        Ok(template)
    }

    /// Every clip, video tracks first.
    pub fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.tracks().flat_map(|t| t.clips.iter())
    }

    /// Video tracks, then audio tracks.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.video_tracks.iter().chain(self.audio_tracks.iter())
    }

    pub fn clip_count(&self) -> usize {
        self.clips().count()
    }

    /// The files it uses that are not where it says they are. Reads the
    /// disk: never call this on the interface thread.
    pub fn missing_media(&self) -> Vec<PathBuf> {
        self.media
            .iter()
            .filter(|m| !m.path.exists())
            .map(|m| m.path.clone())
            .collect()
    }

    /// Gives everything the template's own ids, counted from 1, so a
    /// template never depends on the project it was saved from. A link left
    /// with one clip (its partner was outside the stretch) is dropped.
    fn renumber(&mut self) {
        let mut next = 0u64;
        let mut fresh = || {
            next += 1;
            next
        };
        let media: Vec<(MediaId, MediaId)> = self
            .media
            .iter_mut()
            .map(|m| {
                let old = m.id;
                m.id = MediaId(fresh());
                (old, m.id)
            })
            .collect();
        let old_links: Vec<LinkId> = self.clips().filter_map(|c| c.link).collect();
        let mut links: Vec<(LinkId, LinkId)> = Vec::new();
        for track in self.video_tracks.iter_mut().chain(self.audio_tracks.iter_mut()) {
            track.id = TrackId(fresh());
            for clip in track.clips.iter_mut() {
                clip.id = ClipId(fresh());
                if let Some((_, new)) = media.iter().find(|(old, _)| *old == clip.media) {
                    clip.media = *new;
                }
                clip.link = clip
                    .link
                    .filter(|l| old_links.iter().filter(|o| *o == l).count() > 1)
                    .map(|old| mapped(&mut links, old, || LinkId(fresh())));
            }
        }
    }

    /// Everything that breaks a template's rules, in plain words. Empty means valid.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.length <= Time::ZERO {
            problems.push("it has no length".to_owned());
        }
        if self.clip_count() == 0 {
            problems.push("it has no clips".to_owned());
        }
        for (i, m) in self.media.iter().enumerate() {
            if self.media[..i].iter().any(|other| other.id == m.id) {
                problems.push(format!("{} is listed twice", m.id));
            }
        }
        for (tracks, kind) in [
            (&self.video_tracks, TrackKind::Video),
            (&self.audio_tracks, TrackKind::Audio),
        ] {
            for track in tracks {
                if track.kind != kind {
                    problems.push(format!("track {} is in the wrong area", track.name));
                }
                let mut previous_end = Time::ZERO;
                for clip in &track.clips {
                    let place = format!("{} › {}", track.name, clip.id);
                    if clip.is_file() && !self.media.iter().any(|m| m.id == clip.media) {
                        problems.push(format!("{place} uses missing media {}", clip.media));
                    }
                    if clip.nested().is_some() {
                        problems.push(format!("{place} plays a timeline of a project"));
                    }
                    if clip.duration <= Time::ZERO {
                        problems.push(format!("{place} has no length"));
                    }
                    if clip.start.is_negative() || clip.starts_before_its_source() {
                        problems.push(format!("{place} starts before zero"));
                    }
                    if clip.start < previous_end {
                        problems.push(format!("{place} overlaps the clip before it"));
                    }
                    if clip.end() > self.length {
                        problems.push(format!("{place} ends after the template does"));
                    }
                    if clip.look.check().is_err() {
                        problems.push(format!("{place} has a look out of range"));
                    }
                    problems.extend(clip.keyframes.problems(&place));
                    previous_end = clip.end();
                }
            }
        }
        problems
    }

    /// Checks the template's rules; the error lists what is wrong.
    pub fn validate(&self) -> diag::Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            return Ok(());
        }
        Err(AppError::new("This template is damaged")
            .why(problems.join("; "))
            .fix("Delete it and save the template again from a project that has it."))
    }
}

/// The part of `clip` inside `range`, placed from the range's start.
fn cut_to(clip: &Clip, range: Region) -> Option<Clip> {
    let start = clip.start.max(range.start);
    let end = clip.end().min(range.end);
    // A cut end loses its fade there, as a split does.
    let mut sound = clip.sound;
    if start > clip.start {
        sound = sound.without_fade_in();
    }
    if end < clip.end() {
        sound = sound.without_fade_out();
    }
    (start < end).then(|| Clip {
        start: start - range.start,
        // A cut start also moves where in the file the clip begins, by its speed.
        source_in: clip.source_at(start),
        duration: end - start,
        sound: sound.fitted(end - start),
        // Only the keyframes of the part kept, with its ends held.
        keyframes: clip.keyframes.cut_to(clip.source_at(start), clip.source_at(end)),
        // The ramp keeps its place on the picture.
        ramp: clip.ramp.shifted(clip.start - start),
        ..clip.clone()
    })
}

/// The new id standing in for `old`, made on first use.
fn mapped<T: Copy + PartialEq>(map: &mut Vec<(T, T)>, old: T, make: impl FnOnce() -> T) -> T {
    match map.iter().find(|(o, _)| *o == old) {
        Some((_, new)) => *new,
        None => {
            let new = make();
            map.push((old, new));
            new
        }
    }
}

/// Whether a project's media entry is the file a template's entry means:
/// the same contents (size and sample hash) of the same kind, wherever the
/// file is now. A file at the same place with other contents is another file.
fn same_file(in_project: &MediaRef, in_template: &MediaRef) -> bool {
    in_project.fingerprint == in_template.fingerprint && in_project.kind == in_template.kind
}

/// Puts `template` into a timeline at `at`. See [`crate::Command::InsertTemplate`].
pub(crate) fn insert(
    p: &mut Project,
    sequence: SequenceId,
    template: &Template,
    at: Time,
    name_tracks: bool,
) -> diag::Result<()> {
    template.validate()?;
    if at.is_negative() {
        return Err(AppError::new("The template can't go there").why("That is before the timeline starts."));
    }
    // The project's own entry is used for a file it already has (preferring
    // one at the same place); other files are added.
    let mut media: Vec<(MediaId, MediaId)> = Vec::new();
    for entry in &template.media {
        let known = p
            .media
            .values()
            .filter(|m| same_file(m, entry))
            .max_by_key(|m| m.path == entry.path)
            .map(|m| m.id);
        let id = match known {
            Some(id) => id,
            None => {
                let id: MediaId = p.allocate_id();
                p.media.insert(id, MediaRef { id, ..entry.clone() });
                id
            }
        };
        media.push((entry.id, id));
    }
    // A timeline still waiting for its first video takes its settings from the template's.
    let first_video = template
        .video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| template.media.iter().find(|m| m.id == c.media))
        .find(|m| m.kind == MediaKind::Video && m.info.video.is_some());
    if let Some(video) = first_video {
        settle_automatic(p, sequence, video)?;
    }
    crate::edits::open_space(p, sequence, at, template.length)?;

    let mut links: Vec<(LinkId, LinkId)> = Vec::new();
    for (kind, tracks) in [
        (TrackKind::Video, &template.video_tracks),
        (TrackKind::Audio, &template.audio_tracks),
    ] {
        for (index, from) in tracks.iter().enumerate() {
            if from.clips.is_empty() {
                continue;
            }
            make_tracks(p, sequence, kind, &tracks[..=index])?;
            if name_tracks {
                name_unused_track(p, sequence, kind, index, &from.name)?;
            }
            for clip in &from.clips {
                let placed = Clip {
                    id: p.allocate_id(),
                    media: match clip.is_file() {
                        true => media
                            .iter()
                            .find(|(old, _)| *old == clip.media)
                            .map(|(_, new)| *new)
                            .ok_or_else(|| missing("media file"))?,
                        false => clip.media,
                    },
                    start: clip.start + at,
                    link: clip.link.map(|old| mapped(&mut links, old, || p.allocate_id())),
                    ..clip.clone()
                };
                let track = tracks_of(sequence_mut(p, sequence)?, kind)
                    .get_mut(index)
                    .ok_or_else(|| missing("track"))?;
                insert_clip(track, placed)?;
            }
        }
    }
    Ok(())
}

/// A track of the timeline nobody has used yet (empty, still with its
/// usual name, "A1") takes the name of the template's track that lands on
/// it when someone chose that name ("Game"), as a file placed on it does.
fn name_unused_track(
    p: &mut Project,
    sequence: SequenceId,
    kind: TrackKind,
    index: usize,
    wanted: &str,
) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let name = new_track_name(seq, kind, wanted);
    let letter = match kind {
        TrackKind::Video => 'V',
        TrackKind::Audio => 'A',
    };
    if let Some(track) = tracks_of(seq, kind).get_mut(index)
        && track.clips.is_empty()
        && track.name == format!("{letter}{}", index + 1)
        && name == wanted.trim()
    {
        track.name = name;
    }
    Ok(())
}

/// The timeline's tracks of one kind.
fn tracks_of(seq: &mut Sequence, kind: TrackKind) -> &mut imbl::Vector<Track> {
    match kind {
        TrackKind::Video => &mut seq.video_tracks,
        TrackKind::Audio => &mut seq.audio_tracks,
    }
}

/// Makes sure the timeline has as many tracks of `kind` as `wanted`. A new
/// track goes after the others of its kind, as any added track does, and is
/// as loud as the template's track in that place, with its role, pan and
/// effects (not its lines).
pub(crate) fn make_tracks(
    p: &mut Project,
    sequence: SequenceId,
    kind: TrackKind,
    wanted: &[Track],
) -> diag::Result<()> {
    loop {
        let seq = sequence_mut(p, sequence)?;
        let have = tracks_of(seq, kind).len();
        let Some(model) = wanted.get(have) else {
            return Ok(());
        };
        let (name, gain_db) = (new_track_name(seq, kind, &model.name), model.gain_db);
        // Lines are in the template's timeline's time, which means nothing
        // where it is dropped: they stay behind.
        let sound = TrackSound {
            volume_line: Default::default(),
            pan_line: Default::default(),
            ..model.sound.clone()
        };
        crate::tracks::add(p, sequence, kind, &name)?;
        if let Some(made) = tracks_of(sequence_mut(p, sequence)?, kind).back_mut() {
            made.gain_db = gain_db;
            made.sound = sound;
        }
    }
}

/// The name of a track made for a template: the template track's own name
/// when someone chose it ("Music", "Titles") and no track of that kind has
/// it yet; otherwise the next usual name ("V2", "A3"), so a template's
/// "V3" never ends up as the second track or beside another "V3".
fn new_track_name(seq: &Sequence, kind: TrackKind, wanted: &str) -> String {
    let wanted = wanted.trim();
    let letter = match kind {
        TrackKind::Video => 'V',
        TrackKind::Audio => 'A',
    };
    let usual = wanted
        .strip_prefix(letter)
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()));
    let taken = seq.tracks().any(|t| t.kind == kind && t.name == wanted);
    let fits = !wanted.is_empty() && wanted.chars().count() <= crate::tracks::LONGEST_NAME;
    if usual || taken || !fits {
        seq.next_track_name(kind)
    } else {
        wanted.to_owned()
    }
}

// ---- Template files ----

#[derive(Serialize)]
struct FileOut<'a> {
    format_version: u32,
    template: &'a Template,
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    template: Template,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
}

/// One template file in the folder.
#[derive(Debug)]
pub struct Listed {
    /// The file's name without its extension.
    pub name: String,
    pub path: PathBuf,
    /// What it holds, or why it can't be used (damaged, or from a newer version).
    pub contents: diag::Result<Contents>,
}

/// A template that could be read.
#[derive(Debug, Clone, PartialEq)]
pub struct Contents {
    pub template: Template,
    /// The files it uses that can't be found where it says they are.
    pub missing_media: Vec<PathBuf>,
}

/// Where templates are kept: `templates/` in the app's data folder.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("templates")
}

/// The file text for `template` in the current format.
pub fn to_string(template: &Template) -> diag::Result<String> {
    let file = FileOut {
        format_version: FORMAT_VERSION,
        template,
    };
    let config = ron::ser::PrettyConfig::new().indentor("  ".to_owned());
    ron::ser::to_string_pretty(&file, config).or_explain(|| {
        AppError::new("The template can't be saved").why("It could not be turned into file text.")
    })
}

/// Parses template file text, upgrading it from an older format if needed.
pub fn from_str(text: &str) -> diag::Result<Template> {
    from_str_with(text, FORMAT_VERSION, MIGRATIONS)
}

/// Like [`from_str`] with an explicit target version and migration list (used by tests).
pub fn from_str_with(text: &str, current: u32, migrations: &[Migration]) -> diag::Result<Template> {
    let version = ron::from_str::<Header>(text).or_explain(damaged)?.format_version;
    if version > current {
        return Err(AppError::new("This template was saved by a newer version")
            .why(format!(
                "It uses template format {version}; this version reads up to format {current}."
            ))
            .fix("Update the app, then use the template again."));
    }
    let mut text = std::borrow::Cow::Borrowed(text);
    for from in version..current {
        let step = from
            .checked_sub(1)
            .and_then(|i| migrations.get(i as usize))
            .ok_or_else(damaged)?;
        text = std::borrow::Cow::Owned(step(&text)?);
    }
    let file: FileIn = ron::from_str(&text).or_explain(damaged)?;
    file.template.validate()?;
    Ok(file.template)
}

fn damaged() -> AppError {
    AppError::new("This file is not a template, or it is damaged")
        .why("Its contents could not be read as a template.")
        .fix("Delete it and save the template again from a project that has it.")
}

/// Reads one template file.
pub fn load(path: &Path) -> diag::Result<Template> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The template can't be read")
            .why(format!("{} could not be read.", path.display()))
            .fix("Check that the file still exists and that you can open it.")
    })?;
    from_str(&text)
}

/// The templates in `dir` by name, each read (and its media looked for).
/// Files that can't be read are listed too, with why.
pub fn list(dir: &Path) -> Vec<Listed> {
    let mut found: Vec<Listed> = files(dir)
        .into_iter()
        .map(|(name, path)| {
            let contents = load(&path).map(|template| Contents {
                missing_media: template.missing_media(),
                template,
            });
            Listed { name, path, contents }
        })
        .collect();
    found.sort_by_key(|t| t.name.to_lowercase());
    found
}

/// Every template file in `dir`: its name and where it is.
fn files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some(FILE_EXTENSION) {
                return None;
            }
            Some((path.file_stem()?.to_string_lossy().into_owned(), path))
        })
        .collect()
}

/// Saves `template` in `dir` under `name`; returns the name as kept (made
/// safe for a file name). Refused when a template already has that name.
pub fn save(template: &Template, dir: &Path, name: &str) -> diag::Result<String> {
    template.validate()?;
    let name = usable_name(name)?;
    refuse_if_taken(dir, &name, None)?;
    let explain = || {
        AppError::new("The template can't be saved")
            .why(format!("Writing to {} failed.", dir.display()))
            .fix("Check that the disk is not full and that the app's data folder can be written to.")
    };
    fs::create_dir_all(dir).or_explain(explain)?;
    let text = to_string(template)?;
    crate::disk::write_atomic(&path_of(dir, &name), text.as_bytes()).or_explain(explain)?;
    Ok(name)
}

/// Gives the template called `from` the name `to`; returns the name as kept.
pub fn rename(dir: &Path, from: &str, to: &str) -> diag::Result<String> {
    let to = usable_name(to)?;
    if to == from {
        return Ok(to);
    }
    refuse_if_taken(dir, &to, Some(from))?;
    fs::rename(path_of(dir, from), path_of(dir, &to)).or_explain(|| {
        AppError::new("The template can't be renamed")
            .why(format!("\"{from}\" could not be renamed in {}.", dir.display()))
            .fix("Check that the template is still there and that the app's data folder can be written to.")
    })?;
    Ok(to)
}

/// Removes the template called `name` for good.
pub fn delete(dir: &Path, name: &str) -> diag::Result<()> {
    fs::remove_file(path_of(dir, name)).or_explain(|| {
        AppError::new("The template can't be deleted")
            .why(format!("\"{name}\" could not be removed from {}.", dir.display()))
            .fix("Check that the template is still there and that the app's data folder can be written to.")
    })
}

fn path_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{FILE_EXTENSION}"))
}

/// `name` made safe for a file name, or why it can't be one.
fn usable_name(name: &str) -> diag::Result<String> {
    let name = crate::versions::clean_name(name);
    let name = name.trim_start_matches('.').trim_end_matches('.').trim();
    if name.is_empty() {
        return Err(AppError::new("A template needs a name").fix("Type a few words, like \"Channel intro\"."));
    }
    // Names some systems keep for devices can't be file names there.
    const KEPT: [&str; 4] = ["con", "prn", "aux", "nul"];
    let lower = name.to_lowercase();
    let numbered = |prefix: &str| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit()))
    };
    if KEPT.contains(&lower.as_str()) || numbered("com") || numbered("lpt") {
        return Err(AppError::new("That name can't be used for a template")
            .why(format!("The operating system keeps \"{name}\" for itself."))
            .fix("Add a word, like \"Channel intro\"."));
    }
    Ok(name.to_owned())
}

/// Names differing only in capitals count as the same, as they are the
/// same file on some systems. `except` is the template being renamed.
fn refuse_if_taken(dir: &Path, name: &str, except: Option<&str>) -> diag::Result<()> {
    let taken = files(dir)
        .into_iter()
        .any(|(other, _)| Some(other.as_str()) != except && other.to_lowercase() == name.to_lowercase());
    if taken {
        return Err(AppError::new("A template already has that name")
            .why(format!("There is already a template called \"{name}\"."))
            .fix("Pick another name, or delete the old template first."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{Look, ONE};
    use crate::model::{AudioStream, Fingerprint, MediaInfo, SequenceSettings, VideoStream};
    use crate::time::{Rate, Speed};
    use crate::{Command, Document, Recovery};

    fn recording(path: &str, hash: &str) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: path.into(),
            fingerprint: Fingerprint {
                size: 5_000,
                sample_hash: hash.into(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(100),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "h264".into(),
                    width: 1280,
                    height: 720,
                    frame_rate: Rate::FPS_30,
                    variable_frame_rate: false,
                    rotation: 0,
                }),
                audio: vec![AudioStream {
                    index: 1,
                    codec: "aac".into(),
                    channels: 2,
                    sample_rate: 44_100,
                    name: "Voice".into(),
                }],
                stated_length: None,
            },
        }
    }

    fn music() -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: "sounds/jingle.flac".into(),
            fingerprint: Fingerprint {
                size: 900,
                sample_hash: "jingle".into(),
            },
            kind: MediaKind::Audio,
            duration: Time::seconds(60),
            info: MediaInfo::default(),
        }
    }

    /// A timeline with a title card and its voice (linked), a sped-up part
    /// and its voice (linked), and music on a second, quieter audio track:
    ///
    /// ```text
    /// V1     [title 0..10][fast 10..20, twice as fast, from 30 s]
    /// A1     [title 0..10][fast 10..20]
    /// Music    [jingle 2..18]
    /// ```
    fn intro_project() -> Project {
        let mut p = Project::new("Channel", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        for media in [recording("clips/title.mkv", "title"), music()] {
            p = Command::AddMedia { media }.apply(&p).unwrap();
        }
        let ids: Vec<MediaId> = p.media.keys().copied().collect();
        let (video, jingle) = (ids[0], ids[1]);
        p = Command::AddTrack {
            sequence,
            kind: TrackKind::Audio,
            name: "Music".into(),
        }
        .apply(&p)
        .unwrap();
        let (title_link, fast_link): (LinkId, LinkId) = (p.allocate_id(), p.allocate_id());
        let seq = p.active().unwrap();
        let (v1, a1, a2) = (
            seq.video_tracks[0].id,
            seq.audio_tracks[0].id,
            seq.audio_tracks[1].id,
        );
        let twice = Speed::new(2, 1).unwrap();
        let clip = |media, stream, start, source_in, len, link, speed| Clip {
            lines: Default::default(),
            effects: Default::default(),
            id: ClipId(0),
            media,
            stream,
            start: Time::seconds(start),
            source_in: Time::seconds(source_in),
            duration: Time::seconds(len),
            link,
            speed,
            look: Look::PLAIN,
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
        };
        for (track, clip) in [
            (v1, clip(video, 0, 0, 0, 10, Some(title_link), Speed::NORMAL)),
            (v1, clip(video, 0, 10, 30, 10, Some(fast_link), twice)),
            (a1, clip(video, 1, 0, 0, 10, Some(title_link), Speed::NORMAL)),
            (a1, clip(video, 1, 10, 30, 10, Some(fast_link), twice)),
            (a2, clip(jingle, 0, 2, 0, 16, None, Speed::NORMAL)),
        ] {
            p = Command::PlaceClip {
                sequence,
                track,
                clip,
            }
            .apply(&p)
            .unwrap();
        }
        let seq = p.sequences.front_mut().unwrap();
        seq.audio_tracks[1].gain_db = -6.0;
        seq.audio_tracks[1].muted = true;
        p
    }

    fn intro() -> Template {
        let p = intro_project();
        Template::capture(&p, p.active_sequence, None).unwrap()
    }

    fn region(start: i64, end: i64) -> Option<Region> {
        Region::between(Time::seconds(start), Time::seconds(end))
    }

    /// (start, length, where in the file) of each clip, in whole seconds.
    fn spans<'a>(clips: impl IntoIterator<Item = &'a Clip>) -> Vec<(i64, i64, i64)> {
        let whole = |t: Time| {
            assert_eq!(t.den(), 1, "{t:?} is not a whole second");
            t.num()
        };
        clips
            .into_iter()
            .map(|c| (whole(c.start), whole(c.duration), whole(c.source_in)))
            .collect()
    }

    fn insert_at(p: &Project, template: &Template, at: i64) -> diag::Result<Project> {
        Command::InsertTemplate {
            sequence: p.active_sequence,
            template: Box::new(template.clone()),
            at: Time::seconds(at),
            name_tracks: false,
        }
        .apply(p)
    }

    #[test]
    fn the_whole_timeline_is_saved_when_no_region_is_marked() {
        let t = intro();
        assert_eq!(t.length, Time::seconds(20));
        assert_eq!(t.clip_count(), 5);
        assert_eq!(spans(&t.video_tracks[0].clips), [(0, 10, 0), (10, 10, 30)]);
        assert_eq!(spans(&t.audio_tracks[1].clips), [(2, 16, 0)]);
        assert_eq!(t.media.len(), 2);
        let music = &t.audio_tracks[1];
        assert_eq!((music.name.as_str(), music.gain_db), ("Music", -6.0));
        assert!(!music.muted, "switches are not part of a template");
        assert_eq!(t.video_tracks[0].clips[1].speed, Speed::new(2, 1).unwrap());
    }

    #[test]
    fn a_template_has_its_own_ids() {
        let t = intro();
        let mut ids: Vec<u64> = t.media.iter().map(|m| m.id.0).collect();
        ids.extend(t.tracks().map(|t| t.id.0));
        ids.extend(t.clips().map(|c| c.id.0));
        ids.extend(t.clips().filter_map(|c| c.link).map(|l| l.0));
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids,
            (1..=12).collect::<Vec<u64>>(),
            "2 files, 3 tracks, 5 clips, 2 links"
        );
    }

    #[test]
    fn what_goes_in_comes_out_the_same() {
        let t = intro();
        // Dropped in after itself, the copy has all new ids; saved again, it is the same template.
        let p = insert_at(&intro_project(), &t, 20).unwrap();
        let again = Template::capture(&p, p.active_sequence, region(20, 40)).unwrap();
        assert_eq!(again, t);
    }

    #[test]
    fn saving_a_region_cuts_clips_at_its_edges() {
        let p = intro_project();
        let t = Template::capture(&p, p.active_sequence, region(5, 15)).unwrap();
        assert_eq!(t.length, Time::seconds(10));
        // The title card loses its first 5 s; the fast part its last 5 s.
        assert_eq!(spans(&t.video_tracks[0].clips), [(0, 5, 5), (5, 5, 30)]);
        assert_eq!(spans(&t.audio_tracks[0].clips), [(0, 5, 5), (5, 5, 30)]);
        // The jingle started at 2 s, so 3 s of it are before the region.
        assert_eq!(spans(&t.audio_tracks[1].clips), [(0, 10, 3)]);
        t.validate().unwrap();
    }

    #[test]
    fn a_cut_start_moves_into_the_file_by_the_clips_speed() {
        let p = intro_project();
        let t = Template::capture(&p, p.active_sequence, region(12, 20)).unwrap();
        // 2 s of timeline at twice the speed are 4 s of the file.
        assert_eq!(spans(&t.video_tracks[0].clips), [(0, 8, 34)]);
        assert_eq!(t.video_tracks[0].clips[0].speed, Speed::new(2, 1).unwrap());
        assert_eq!(t.video_tracks[0].clips[0].source_len(), Time::seconds(16));
    }

    #[test]
    fn each_clips_look_is_kept() {
        let mut p = intro_project();
        let look = Look {
            x: ONE / 4,
            scale: ONE / 2,
            opacity: ONE / 2,
            ..Look::PLAIN
        };
        p.sequences.front_mut().unwrap().video_tracks[0].clips[0].look = look;
        // Cut by the region, through the file, and into another project.
        let t = Template::capture(&p, p.active_sequence, region(5, 15)).unwrap();
        assert_eq!(t.video_tracks[0].clips[0].look, look);
        let t = from_str(&to_string(&t).unwrap()).unwrap();
        let done = insert_at(&other_project(), &t, 0).unwrap();
        let clips = &done.active().unwrap().video_tracks[0].clips;
        assert_eq!(clips[0].look, look);
        assert_eq!(clips[1].look, Look::PLAIN, "and nobody else gets it");
    }

    #[test]
    fn a_region_keeps_its_own_length_and_only_the_tracks_it_uses() {
        let p = intro_project();
        // Past the jingle's end and past the timeline's end.
        let t = Template::capture(&p, p.active_sequence, region(18, 25)).unwrap();
        assert_eq!(t.length, Time::seconds(7));
        assert_eq!(
            t.audio_tracks.len(),
            1,
            "the empty Music track after it is dropped"
        );
        assert_eq!(t.media.len(), 1, "the jingle isn't used");
        assert_eq!(spans(&t.video_tracks[0].clips), [(0, 2, 46)]);
    }

    #[test]
    fn a_link_left_with_one_clip_is_dropped() {
        let mut p = intro_project();
        // Remove the title's voice: the title card's link now has no partner.
        let seq = p.sequences.front_mut().unwrap();
        seq.audio_tracks[0].clips.remove(0);
        let t = Template::capture(&p, p.active_sequence, None).unwrap();
        assert_eq!(t.video_tracks[0].clips[0].link, None);
        assert!(t.video_tracks[0].clips[1].link.is_some());
        assert_eq!(t.video_tracks[0].clips[1].link, t.audio_tracks[0].clips[0].link);
    }

    #[test]
    fn nothing_to_save_is_explained() {
        let empty = Project::new("Empty", SequenceSettings::GAME_CLIP);
        let err = Template::capture(&empty, empty.active_sequence, None).unwrap_err();
        assert!(err.what().contains("nothing to save"), "{}", err.report());
        assert!(err.why_text().unwrap().contains("no clips yet"));
        let p = intro_project();
        let err = Template::capture(&p, p.active_sequence, region(40, 50)).unwrap_err();
        assert!(err.why_text().unwrap().contains("region"), "{}", err.report());
    }

    /// A project with one long clip (and its voice) from 0 to 60 s.
    fn other_project() -> Project {
        let p = Project::new("Episode", SequenceSettings::GAME_CLIP);
        let mut media = recording("episodes/part-one.mkv", "part-one");
        media.duration = Time::seconds(60);
        Command::Import {
            sequence: p.active_sequence,
            media,
        }
        .apply(&p)
        .unwrap()
    }

    #[test]
    fn inserting_at_the_start_moves_everything_later() {
        let p = other_project();
        let done = insert_at(&p, &intro(), 0).unwrap();
        let seq = done.active().unwrap();
        assert_eq!(
            spans(&seq.video_tracks[0].clips),
            [(0, 10, 0), (10, 10, 30), (20, 60, 0)]
        );
        assert_eq!(
            spans(&seq.audio_tracks[0].clips),
            [(0, 10, 0), (10, 10, 30), (20, 60, 0)]
        );
        assert_eq!(spans(&seq.audio_tracks[1].clips), [(2, 16, 0)]);
        assert_eq!(seq.length(), Time::seconds(80));
        done.validate().unwrap();
    }

    #[test]
    fn inserting_at_the_needle_splits_what_is_under_it() {
        let p = other_project();
        let with_label = Command::AddLabel {
            sequence: p.active_sequence,
            label: crate::TimelineLabel {
                id: crate::LabelId(0),
                name: "After the break".into(),
                at: Time::seconds(45),
                color: crate::LabelColor::Green,
            },
        }
        .apply(&p)
        .unwrap();
        let done = insert_at(&with_label, &intro(), 25).unwrap();
        let seq = done.active().unwrap();
        assert_eq!(
            spans(&seq.video_tracks[0].clips),
            [(0, 25, 0), (25, 10, 0), (35, 10, 30), (45, 35, 25)]
        );
        assert_eq!(spans(&seq.audio_tracks[1].clips), [(27, 16, 0)]);
        assert_eq!(seq.labels[0].at, Time::seconds(65), "labels after it move along");
        done.validate().unwrap();
    }

    #[test]
    fn inserting_at_the_end_moves_nothing() {
        let p = other_project();
        let end = p.active().unwrap().length();
        assert_eq!(end, Time::seconds(60));
        let done = insert_at(&p, &intro(), 60).unwrap();
        let seq = done.active().unwrap();
        assert_eq!(
            spans(&seq.video_tracks[0].clips),
            [(0, 60, 0), (60, 10, 0), (70, 10, 30)]
        );
        assert_eq!(seq.length(), Time::seconds(80));
        assert!(insert_at(&p, &intro(), -1).is_err(), "not before the start");
    }

    #[test]
    fn missing_tracks_are_made_like_the_templates() {
        let p = other_project();
        assert_eq!(p.active().unwrap().audio_tracks.len(), 1);
        let done = insert_at(&p, &intro(), 0).unwrap();
        let seq = done.active().unwrap();
        assert_eq!(seq.audio_tracks.len(), 2);
        let made = &seq.audio_tracks[1];
        assert_eq!(
            (made.name.as_str(), made.gain_db, made.kind),
            ("Music", -6.0, TrackKind::Audio)
        );
        assert_eq!(
            seq.audio_tracks[0].name, "Voice",
            "tracks already there keep their name"
        );
        // A second time, the track is there already.
        let again = insert_at(&done, &intro(), 0).unwrap();
        assert_eq!(again.active().unwrap().audio_tracks.len(), 2);
    }

    #[test]
    fn an_unused_track_takes_the_name_of_the_templates_track() {
        let p = Project::new("Empty", SequenceSettings::GAME_CLIP);
        let mut template = intro();
        template.audio_tracks[0].name = "Commentary".into();
        let wanted = template.audio_tracks[0].name.clone();
        let insert = |name_tracks| Command::InsertTemplate {
            sequence: p.active_sequence,
            template: Box::new(template.clone()),
            at: Time::ZERO,
            name_tracks,
        };
        let named = insert(true).apply(&p).unwrap();
        assert_eq!(named.active().unwrap().audio_tracks[0].name, wanted);
        let kept = insert(false).apply(&p).unwrap();
        assert_eq!(
            kept.active().unwrap().audio_tracks[0].name,
            "A1",
            "as before the option came"
        );
    }

    #[test]
    fn a_template_insert_written_before_naming_tracks_replays_without_it() {
        let p = Project::new("Empty", SequenceSettings::GAME_CLIP);
        let written = Command::InsertTemplate {
            sequence: p.active_sequence,
            template: Box::new(intro()),
            at: Time::ZERO,
            name_tracks: true,
        };
        let text = ron::to_string(&written).unwrap().replace(",name_tracks:true", "");
        assert!(!text.contains("name_tracks"));
        let read: Command = ron::from_str(&text).unwrap();
        assert!(matches!(
            read,
            Command::InsertTemplate {
                name_tracks: false,
                ..
            }
        ));
    }

    /// The intro with a caption over the title card on a second video
    /// track (4 s to 9 s), and the template of all of it.
    fn captioned() -> (Project, Template) {
        let p = intro_project();
        let sequence = p.active_sequence;
        let name = p.active().unwrap().next_track_name(TrackKind::Video);
        assert_eq!(name, "V2");
        let p = Command::AddTrack {
            sequence,
            kind: TrackKind::Video,
            name,
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let caption = Clip {
            id: ClipId(0),
            start: Time::seconds(4),
            source_in: Time::seconds(50),
            duration: Time::seconds(5),
            link: None,
            ..seq.video_tracks[0].clips[0].clone()
        };
        let p = Command::PlaceClip {
            sequence,
            track: seq.video_tracks[1].id,
            clip: caption,
        }
        .apply(&p)
        .unwrap();
        let template = Template::capture(&p, sequence, None).unwrap();
        (p, template)
    }

    #[test]
    fn a_template_with_more_video_tracks_than_the_project_makes_them() {
        let (_, template) = captioned();
        assert_eq!(template.video_tracks.len(), 2);
        let p = other_project();
        assert_eq!(p.active().unwrap().video_tracks.len(), 1);
        let done = insert_at(&p, &template, 25).unwrap();
        done.validate().unwrap();
        let seq = done.active().unwrap();
        assert_eq!(seq.video_tracks.len(), 2);
        assert_eq!(
            spans(&seq.video_tracks[0].clips),
            [(0, 25, 0), (25, 10, 0), (35, 10, 30), (45, 35, 25)]
        );
        let v2 = &seq.video_tracks[1];
        assert_eq!(
            (v2.name.as_str(), v2.kind, v2.hidden),
            ("V2", TrackKind::Video, false)
        );
        assert_eq!(
            spans(&v2.clips),
            [(29, 5, 50)],
            "the caption, over the title card"
        );
        // The same again: V2 is there now, and what is on it moves along like everything else.
        let again = insert_at(&done, &template, 0).unwrap();
        let seq = again.active().unwrap();
        assert_eq!(seq.video_tracks.len(), 2);
        assert_eq!(spans(&seq.video_tracks[1].clips), [(4, 5, 50), (49, 5, 50)]);
        again.validate().unwrap();
    }

    #[test]
    fn a_template_with_fewer_video_tracks_leaves_room_on_the_others() {
        // The project has two video tracks with clips; the template only one.
        let (p, _) = captioned();
        let one_track = intro();
        assert_eq!(one_track.video_tracks.len(), 1);
        let done = insert_at(&p, &one_track, 6).unwrap();
        done.validate().unwrap();
        let seq = done.active().unwrap();
        assert_eq!(seq.video_tracks.len(), 2, "no track is made or removed");
        assert_eq!(
            spans(&seq.video_tracks[0].clips),
            [(0, 6, 0), (6, 10, 0), (16, 10, 30), (26, 4, 6), (30, 10, 30)]
        );
        // The caption crossed 6 s: it is split there and its second part waits behind the template.
        assert_eq!(spans(&seq.video_tracks[1].clips), [(4, 2, 50), (26, 3, 52)]);
        assert_eq!(seq.length(), Time::seconds(40));
    }

    #[test]
    fn tracks_made_for_a_template_get_names_that_fit_the_timeline() {
        let (_, mut template) = captioned();
        let names = |p: &Project, kind: TrackKind| -> Vec<String> {
            let seq = p.active().unwrap();
            let tracks = seq.tracks().filter(|t| t.kind == kind);
            tracks.map(|t| t.name.clone()).collect()
        };
        // A usual name from another timeline ("V7") becomes the next usual name here.
        template.video_tracks[1].name = "V7".into();
        let done = insert_at(&other_project(), &template, 0).unwrap();
        assert_eq!(names(&done, TrackKind::Video), ["V1", "V2"]);
        assert_eq!(
            names(&done, TrackKind::Audio),
            ["Voice", "Music"],
            "a chosen name is kept"
        );

        // A chosen name is kept, unless a track of that kind has it already.
        template.video_tracks[1].name = " Captions ".into();
        let done = insert_at(&other_project(), &template, 0).unwrap();
        assert_eq!(names(&done, TrackKind::Video), ["V1", "Captions"]);
        let p = other_project();
        let (sequence, v1, a1) = {
            let seq = p.active().unwrap();
            (seq.id, seq.video_tracks[0].id, seq.audio_tracks[0].id)
        };
        let mut renamed = p;
        for (track, name) in [(v1, "Captions"), (a1, "Music")] {
            renamed = Command::RenameTrack {
                sequence,
                track,
                name: name.into(),
            }
            .apply(&renamed)
            .unwrap();
        }
        let done = insert_at(&renamed, &template, 0).unwrap();
        assert_eq!(names(&done, TrackKind::Video), ["Captions", "V2"]);
        assert_eq!(names(&done, TrackKind::Audio), ["Music", "A2"]);

        // A name no track may have (a file changed by hand) is replaced too.
        template.video_tracks[1].name = "  ".into();
        template.audio_tracks[1].name = "x".repeat(crate::tracks::LONGEST_NAME + 1);
        let done = insert_at(&other_project(), &template, 0).unwrap();
        assert_eq!(names(&done, TrackKind::Video), ["V1", "V2"]);
        assert_eq!(names(&done, TrackKind::Audio), ["Voice", "A2"]);
    }

    #[test]
    fn clips_on_a_hidden_track_are_saved_and_hiding_is_not() {
        let (p, shown) = captioned();
        let seq = p.active().unwrap();
        let hidden = Command::SetTrackHidden {
            sequence: seq.id,
            track: seq.video_tracks[1].id,
            hidden: true,
        }
        .apply(&p)
        .unwrap();
        let template = Template::capture(&hidden, hidden.active_sequence, None).unwrap();
        assert_eq!(template, shown, "the same template as with the track shown");
        assert_eq!(template.video_tracks[1].clips.len(), 1);
        assert!(!template.video_tracks[1].hidden);
        // A track made for it is shown; a track that is already there stays as it was set.
        let made = insert_at(&other_project(), &template, 0).unwrap();
        assert!(!made.active().unwrap().video_tracks[1].hidden);
        let into_hidden = insert_at(&hidden, &template, 20).unwrap();
        let v2 = &into_hidden.active().unwrap().video_tracks[1];
        assert!(v2.hidden);
        assert_eq!(spans(&v2.clips), [(4, 5, 50), (24, 5, 50)]);
    }

    #[test]
    fn media_the_project_has_is_reused_and_the_rest_is_added() {
        let template = intro();
        // The project the template came from has both files already.
        let home = intro_project();
        let done = insert_at(&home, &template, 20).unwrap();
        assert_eq!(done.media.len(), 2);

        // Another project gets both, once, however often the template goes in.
        let p = other_project();
        let once = insert_at(&p, &template, 0).unwrap();
        assert_eq!(once.media.len(), 3);
        let twice = insert_at(&once, &template, 0).unwrap();
        assert_eq!(twice.media.len(), 3);
        twice.validate().unwrap();
        let jingle = twice.media.values().find(|m| m.kind == MediaKind::Audio).unwrap();
        assert_eq!(jingle.path, PathBuf::from("sounds/jingle.flac"));
        let uses = |p: &Project| {
            let seq = p.active().unwrap();
            seq.tracks()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.media == jingle.id)
                .count()
        };
        assert_eq!((uses(&once), uses(&twice)), (1, 2));
    }

    #[test]
    fn a_moved_file_is_recognised_and_a_changed_one_is_not() {
        let template = intro();
        // The same contents somewhere else: the project's own entry is used.
        let added = Command::AddMedia {
            media: MediaRef {
                path: "elsewhere/title-copy.mkv".into(),
                ..recording("clips/title.mkv", "title")
            },
        }
        .apply(&other_project())
        .unwrap();
        let moved = insert_at(&added, &template, 0).unwrap();
        assert_eq!(moved.media.len(), 3, "only the jingle is new");
        let first = &moved.active().unwrap().video_tracks[0].clips[0];
        assert_eq!(
            moved.media[&first.media].path,
            PathBuf::from("elsewhere/title-copy.mkv")
        );

        // Other contents at the same place: a different file, so it is added.
        let changed = Command::AddMedia {
            media: recording("clips/title.mkv", "recorded again"),
        }
        .apply(&other_project())
        .unwrap();
        let done = insert_at(&changed, &template, 0).unwrap();
        assert_eq!(done.media.len(), 4);
    }

    #[test]
    fn links_are_kept_with_new_ids() {
        let template = intro();
        let p = other_project();
        let done = insert_at(&insert_at(&p, &template, 0).unwrap(), &template, 0).unwrap();
        let seq = done.active().unwrap();
        let (v, a) = (&seq.video_tracks[0].clips, &seq.audio_tracks[0].clips);
        let mut links = Vec::new();
        for i in 0..4 {
            assert!(v[i].link.is_some());
            assert_eq!(v[i].link, a[i].link, "picture and voice stay linked");
            links.push(v[i].link);
        }
        links.push(v[4].link);
        links.sort();
        links.dedup();
        assert_eq!(links.len(), 5, "each pair has its own link, in both copies");
        assert_eq!(seq.audio_tracks[1].clips[0].link, None);
        // Every clip id in the project is its own.
        let mut ids: Vec<ClipId> = seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn a_new_automatic_timeline_takes_its_settings_from_the_template() {
        let p = Project::new_automatic("Fresh");
        let done = insert_at(&p, &intro(), 0).unwrap();
        let seq = done.active().unwrap();
        assert!(!seq.automatic);
        assert_eq!(
            (seq.settings.width, seq.settings.height, seq.settings.frame_rate),
            (1280, 720, Rate::FPS_30)
        );
        assert_eq!(seq.settings.sample_rate, 44_100);
        assert_eq!(seq.length(), Time::seconds(20));
    }

    #[test]
    fn a_template_whose_media_is_missing_still_inserts() {
        let template = intro();
        assert_eq!(template.missing_media().len(), 2, "these files are made up");
        let done = insert_at(&other_project(), &template, 0).unwrap();
        done.validate().unwrap();
        assert_eq!(done.active().unwrap().video_tracks[0].clips.len(), 3);
    }

    #[test]
    fn undo_restores_exactly_and_the_journal_replays_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("episode.{}", crate::FILE_EXTENSION));
        let before = other_project();
        let mut doc = Document::create(&path, before.clone()).unwrap();
        doc.apply(Command::InsertTemplate {
            sequence: before.active_sequence,
            template: Box::new(intro()),
            at: Time::seconds(25),
            name_tracks: false,
        })
        .unwrap();
        let after = doc.project().clone();
        assert_ne!(after, before);
        assert!(doc.undo().unwrap());
        assert_eq!(doc.project(), &before, "one step back is exactly how it was");
        assert!(doc.redo().unwrap());
        assert_eq!(doc.project(), &after);
        drop(doc); // a crash before any save
        let recovered = Document::open(&path, Recovery::Recover).unwrap();
        // Opening makes the saved project's relative media paths full ones
        // (from the project's folder); the replayed step is the same.
        let mut expected = before.clone();
        crate::file::resolve_media_paths(&mut expected, dir.path());
        let expected = Command::InsertTemplate {
            sequence: before.active_sequence,
            template: Box::new(intro()),
            at: Time::seconds(25),
            name_tracks: false,
        }
        .apply(&expected)
        .unwrap();
        assert_eq!(recovered.project(), &expected);
    }

    #[test]
    fn a_damaged_template_is_refused_and_nothing_changes() {
        let mut template = intro();
        template.video_tracks[0].clips[1].start = Time::seconds(5);
        let err = insert_at(&other_project(), &template, 0).unwrap_err();
        assert!(err.what().contains("damaged"), "{}", err.report());
        assert!(err.why_text().unwrap().contains("overlaps"));
        let mut template = intro();
        template.length = Time::seconds(15);
        assert!(
            insert_at(&other_project(), &template, 0).is_err(),
            "clips past its end"
        );
        let mut template = intro();
        template.media.pop();
        assert!(
            insert_at(&other_project(), &template, 0).is_err(),
            "a clip without its file"
        );
        let mut template = intro();
        template.video_tracks[0].clips[0].look.opacity = 2 * ONE;
        let err = insert_at(&other_project(), &template, 0).unwrap_err();
        assert!(
            err.why_text().unwrap().contains("look out of range"),
            "{}",
            err.report()
        );
    }

    #[test]
    fn save_list_rename_and_delete() {
        let data = tempfile::tempdir().unwrap();
        let dir = folder(data.path());
        assert!(list(&dir).is_empty(), "no folder yet is no templates");
        let template = intro();
        assert_eq!(
            save(&template, &dir, "  Outro: thanks/bye ").unwrap(),
            "Outro- thanks-bye"
        );
        assert_eq!(save(&template, &dir, "Channel intro").unwrap(), "Channel intro");
        let all = list(&dir);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Channel intro", "Outro- thanks-bye"], "by name");
        let contents = all[0].contents.as_ref().unwrap();
        assert_eq!(contents.template, template);
        assert_eq!(contents.missing_media.len(), 2);
        assert_eq!(load(&all[0].path).unwrap(), template);

        assert_eq!(rename(&dir, "Channel intro", "Intro 2026").unwrap(), "Intro 2026");
        assert_eq!(
            rename(&dir, "Intro 2026", "intro 2026").unwrap(),
            "intro 2026",
            "capitals only"
        );
        let err = rename(&dir, "intro 2026", "outro- THANKS-bye").unwrap_err();
        assert!(err.what().contains("already has that name"), "{}", err.report());
        assert!(rename(&dir, "intro 2026", " ").is_err());
        assert!(rename(&dir, "no such template", "Something").is_err());

        delete(&dir, "intro 2026").unwrap();
        assert!(delete(&dir, "intro 2026").is_err(), "already gone");
        assert_eq!(list(&dir).len(), 1);
    }

    #[test]
    fn a_taken_or_unusable_name_is_explained() {
        let data = tempfile::tempdir().unwrap();
        let dir = folder(data.path());
        let template = intro();
        save(&template, &dir, "Intro").unwrap();
        let err = save(&template, &dir, "intro").unwrap_err();
        assert!(err.what().contains("already has that name"), "{}", err.report());
        assert!(
            save(&template, &dir, "")
                .unwrap_err()
                .what()
                .contains("needs a name")
        );
        assert!(save(&template, &dir, "...").is_err());
        assert!(
            save(&template, &dir, "NUL")
                .unwrap_err()
                .what()
                .contains("can't be used")
        );
        assert!(save(&template, &dir, "com1").is_err());
        assert_eq!(save(&template, &dir, "Common").unwrap(), "Common");
        assert_eq!(list(&dir).len(), 2);
    }

    #[test]
    fn a_newer_or_damaged_file_is_listed_with_why() {
        let data = tempfile::tempdir().unwrap();
        let dir = folder(data.path());
        save(&intro(), &dir, "Good").unwrap();
        let text = to_string(&intro()).unwrap();
        let newer = text.replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            "format_version: 99",
            1,
        );
        assert_ne!(newer, text);
        fs::write(path_of(&dir, "From the future"), newer).unwrap();
        fs::write(path_of(&dir, "Broken"), "(format_version: 1, template: (").unwrap();
        fs::write(path_of(&dir, "Not even text"), [0u8, 159, 146, 150]).unwrap();
        fs::write(dir.join("notes.txt"), "not a template").unwrap();

        let all = list(&dir);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Broken", "From the future", "Good", "Not even text"]);
        let what = |i: usize| all[i].contents.as_ref().unwrap_err().what().to_owned();
        assert!(what(0).contains("damaged"), "{}", what(0));
        assert!(what(1).contains("newer version"), "{}", what(1));
        assert!(all[2].contents.is_ok());
        assert!(what(3).contains("can't be read"), "{}", what(3));
    }

    #[test]
    fn older_formats_are_upgraded_step_by_step() {
        let text = to_string(&intro()).unwrap();
        let one = text.replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            "format_version: 1",
            1,
        );
        assert_eq!(from_str(&one).unwrap(), intro(), "version 1 text, upgraded");
        assert!(
            from_str_with(&one, 2, &[]).is_err(),
            "no way up is an error, not a guess"
        );
        let zero = text.replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            "format_version: 0",
            1,
        );
        assert!(from_str(&zero).is_err());
    }
}
