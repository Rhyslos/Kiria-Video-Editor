//! Every change to a project is a [`Command`].
//!
//! A command takes the current snapshot and returns a new one, or an
//! explained error and no change. Commands are plain data, so they can be
//! written to the autosave journal and replayed after a crash.

use crate::animate::{KeyframeChange, KeyframesChange};
use crate::ids::{ClipId, LabelId, LinkId, MediaId, MediaLabelId, MediaListId, SequenceId, TrackId};
use crate::keyframes::LookField;
use crate::library::{MediaLabel, MediaUse};
use crate::look::Look;
use crate::model::{
    Clip, FrameBlend, LabelColor, MediaKind, MediaRef, Project, Region, Sequence, SequenceSettings,
    TimelineLabel, Track, TrackKind,
};
use crate::ramp::Ramp;
use crate::sound::{ClipLines, Sound};
use crate::templates::Template;
use crate::time::{Speed, Time};
use crate::track_sound::{MasterSound, TrackSound};
use diag::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One change to a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    RenameProject {
        name: String,
    },
    AddMedia {
        media: MediaRef,
    },
    /// Places a file already in the project at the end of the timeline, as
    /// [`Command::Import`] does.
    PlaceMedia {
        sequence: SequenceId,
        media: MediaId,
    },
    /// Places a file of the project at `at`, pushing everything from there
    /// on later to make room (clips under `at` are split).
    InsertMedia {
        sequence: SequenceId,
        media: MediaId,
        at: Time,
    },
    /// Places a file of the project at `at` over whatever is there: the
    /// clips it covers are cut away, nothing else moves.
    OverwriteMedia {
        sequence: SequenceId,
        media: MediaId,
        at: Time,
    },
    /// Plays `clip` (and its linked clips) at `speed`: the same part of the
    /// file, so the clip gets longer (slower) or shorter (faster), and
    /// everything after it on the timeline moves along.
    SetSpeed {
        sequence: SequenceId,
        clip: ClipId,
        speed: Speed,
    },
    /// Gives `clip` (and its linked clips, at the same place on the
    /// timeline) a speed that changes over time: `ramp`'s keyframes are
    /// from `clip`'s start. It plays the same part of its file from the
    /// same start, so its length follows the ramp (to the nearest frame)
    /// and everything after it on the timeline moves along. An empty ramp
    /// puts it back at its one speed.
    SetRamp {
        sequence: SequenceId,
        clip: ClipId,
        ramp: Ramp,
    },
    /// Whether `clip`'s picture mixes the two recorded frames around each
    /// moment when slowed down ([`FrameBlend`]). Only clips on video tracks.
    SetFrameBlend {
        sequence: SequenceId,
        clip: ClipId,
        blend: FrameBlend,
    },
    /// Sets where `clip`'s picture sits in the frame, which part of it
    /// shows, how strong it is and its quick colour. Only that clip: its
    /// linked sound has no picture. Numbers that have keyframes keep them
    /// (and follow them); [`Command::SetLookAt`] is what the inspector uses.
    SetLook {
        sequence: SequenceId,
        clip: ClipId,
        look: Look,
    },
    /// Sets `clip`'s look as it is seen at `at` on the timeline (the
    /// playhead, brought onto the clip). A number that changes over time
    /// gets a keyframe there (the one already in that frame changes, or a
    /// new one is added); a number that doesn't is set for the whole clip.
    /// Only numbers that differ from what is seen there are written, and a
    /// place's X and Y always go together (format 10).
    SetLookAt {
        sequence: SequenceId,
        clip: ClipId,
        at: Time,
        look: Look,
    },
    /// Adds, removes or sets the ease of the keyframes of `fields` in the
    /// frame at `at` on the timeline (the playhead, brought onto the clip).
    Keyframe {
        sequence: SequenceId,
        clip: ClipId,
        at: Time,
        fields: Vec<LookField>,
        change: KeyframeChange,
    },
    /// Moves, removes or sets the ease of every keyframe in each of the
    /// timeline frames `frames` (the frames of the keyframe marks on the
    /// timeline: a mark is all the keyframes of one frame, whatever numbers
    /// they are for). One step for all of them. A move goes by whole frames,
    /// stays on the clip, and is refused when a number would get a second
    /// keyframe in a frame; keyframes of other numbers already there are
    /// joined (their marks become one).
    ChangeKeyframes {
        sequence: SequenceId,
        clip: ClipId,
        frames: Vec<Time>,
        change: KeyframesChange,
    },
    /// Pastes copied keyframe marks onto picture clip `clip`: the first on
    /// the frame at `at` on the timeline (the playhead, brought onto the
    /// clip), the others as far after it as they were copied. One step for
    /// all of them. A pasted number's keyframe replaces one already in its
    /// frame. Marks that would land off the clip are left out; refused
    /// when none fits. See [`crate::clipboard`].
    PasteKeyframes {
        sequence: SequenceId,
        clip: ClipId,
        at: Time,
        keyframes: crate::clipboard::CopiedKeyframes,
    },
    /// Puts `fields` of `clip`'s look back as they come and takes their
    /// keyframes away: a section's Reset.
    ResetLook {
        sequence: SequenceId,
        clip: ClipId,
        fields: Vec<LookField>,
    },
    /// Sets how each of `sounds`' clips sounds: its volume, pan and fades.
    /// One step for all of them, so a picture's linked sound streams (game,
    /// microphone) change together and are undone together. Only clips on
    /// audio tracks: a picture's sound is its linked sound clips.
    SetSound {
        sequence: SequenceId,
        sounds: Vec<(ClipId, Sound)>,
    },
    /// Puts back a whole earlier state of the project (a saved version).
    /// Undo returns to how it was just before.
    Restore {
        project: Box<Project>,
    },
    /// Drops a template in at `at`: everything from there on, on every
    /// track, moves later by the template's length (clips under `at` are
    /// split), and the template's clips fill the room. Tracks it needs are
    /// made, media the project doesn't have yet is added, and every id is new.
    InsertTemplate {
        sequence: SequenceId,
        template: Box<Template>,
        at: Time,
    },
    /// Takes files that no timeline uses out of the project (prune), with
    /// their labels, places in lists and notes, as one step. Refused when
    /// any of them is used. Files on disk are never touched.
    RemoveMedia {
        media: Vec<MediaId>,
    },
    /// Points a media file at where it is now (it was moved or renamed).
    Relink {
        media: MediaId,
        path: PathBuf,
    },
    /// Points several media files at where they are now, as one step: what
    /// finding moved files does, by itself when a project opens or in a
    /// folder the person picks. Refused, with nothing changed, for no files
    /// or a file the project doesn't have.
    RelinkFiles {
        files: Vec<(MediaId, PathBuf)>,
    },
    /// Adds a media file and places it at the end of the timeline as one step:
    /// its picture on the first video track and each sound stream on its own
    /// audio track (made when missing, named after the stream), all linked.
    /// The media's `id` is ignored and a new one is given.
    Import {
        sequence: SequenceId,
        media: MediaRef,
    },
    /// Places a file of the project at `at` with its picture on the video
    /// track `track` (the first one when `None`); its sound goes on the
    /// audio tracks as for [`Command::Import`]. With `insert` everything
    /// from `at` on moves later on every track to make room, as for
    /// [`Command::InsertMedia`]. Without it the file lies over what is
    /// there: what it covers on the tracks it lands on is cut away, and
    /// every other track keeps what it has, so a clip dropped on a higher
    /// track leaves the picture under it in place.
    DropMedia {
        sequence: SequenceId,
        media: MediaId,
        at: Time,
        track: Option<TrackId>,
        insert: bool,
    },
    /// Lays a file of the project over the timeline at `at`, its picture
    /// on the video track `track` (the first one when `None`). What the
    /// picture covers on that track is replaced: those stretches of the
    /// clips there go and, when the file has sound of its own, so does the
    /// sound linked to them. Each of the file's sound streams then takes the
    /// first audio track where the stretch is free, counting from the one
    /// [`Command::Import`] would give it; a track is made when none is.
    /// Nothing else is cut or moved: the picture on the other video tracks,
    /// and sound that belongs to none of the covered clips, stay as they are.
    LayMedia {
        sequence: SequenceId,
        media: MediaId,
        at: Time,
        track: Option<TrackId>,
    },
    /// Adds an empty track after the others of its kind: a video track on
    /// top of the picture, an audio track at the bottom.
    AddTrack {
        sequence: SequenceId,
        kind: TrackKind,
        name: String,
    },
    /// Removes an empty track. Refused while it has clips, and for the
    /// last track of its kind.
    RemoveTrack {
        sequence: SequenceId,
        track: TrackId,
    },
    RenameTrack {
        sequence: SequenceId,
        track: TrackId,
        name: String,
    },
    /// Hides a video track, so its clips are not seen in the picture, or
    /// shows it again.
    SetTrackHidden {
        sequence: SequenceId,
        track: TrackId,
        hidden: bool,
    },
    /// Places a clip; its `id` is ignored and a new one is given.
    PlaceClip {
        sequence: SequenceId,
        track: TrackId,
        clip: Clip,
    },
    /// Removes a clip and the clips linked to it. With `ripple`, later clips
    /// on the same tracks move back to close the gap.
    RemoveClip {
        sequence: SequenceId,
        clip: ClipId,
        ripple: bool,
    },
    /// Removes several clips and the clips linked to them, as one step: every
    /// clip is checked first, and if any no longer exists nothing changes.
    /// With `ripple`, later clips on the same tracks move back to close the
    /// gaps, as removing them one after another would.
    RemoveClips {
        sequence: SequenceId,
        clips: Vec<ClipId>,
        ripple: bool,
    },
    /// Moves a clip to a new start time, and optionally to another track of the same kind.
    MoveClip {
        sequence: SequenceId,
        clip: ClipId,
        start: Time,
        track: Option<TrackId>,
    },
    /// Moves a clip and every clip linked to it by `by` (later is positive),
    /// keeping them in step. Refused if any would overlap or start before zero.
    MoveLinked {
        sequence: SequenceId,
        clip: ClipId,
        by: Time,
    },
    /// Moves a clip to another track of its kind and, with every clip
    /// linked to it, by `by`: the linked clips stay on their own tracks.
    /// Refused if any would overlap or start before zero.
    MoveToTrack {
        sequence: SequenceId,
        clip: ClipId,
        track: TrackId,
        by: Time,
    },
    /// Moves a clip to the track `track` of its kind, or with `None` to a
    /// new track made after the others of its kind (a video track on top, an
    /// audio track at the bottom), and, with every clip linked to it, by
    /// `by`. Linked sound stays on its own track where that stretch is free
    /// and otherwise goes to the first audio track where it is (a track is
    /// made when none is), so a picture moved over another keeps its sound
    /// and nothing under it is cut. Linked pictures stay on their own
    /// tracks. Refused if the clip itself or a linked picture would overlap
    /// a clip, or any of them would start before zero.
    MoveBetweenTracks {
        sequence: SequenceId,
        clip: ClipId,
        track: Option<TrackId>,
        by: Time,
    },
    /// Lines clips up where sync by sound found them: each clip (with its
    /// linked clips) moves to start at its time, as one step. A clip with
    /// a picture goes to the nearest frame; sound alone lines up exactly
    /// (its edges on frames, where in its file it starts moved by what is
    /// left over). What would start before zero is cut off. Sound in the
    /// way of something takes the first free audio track, as in
    /// [`Command::MoveBetweenTracks`]; refused if a picture would overlap a
    /// clip that isn't moving, or two of them are linked.
    LineUp {
        sequence: SequenceId,
        moves: Vec<(ClipId, Time)>,
    },
    /// Moves one edge of a clip (and the same edge of its linked clips) to
    /// `to` on the timeline. The start edge also moves where in the file the
    /// clip begins, so the picture stays in place.
    Trim {
        sequence: SequenceId,
        clip: ClipId,
        edge: Edge,
        to: Time,
    },
    /// Like [`Command::Trim`], but the rest of the timeline follows: every
    /// clip after the trimmed one moves by the same amount, so no gap opens
    /// and nothing is covered. The start edge keeps the clip where it is.
    RippleTrim {
        sequence: SequenceId,
        clip: ClipId,
        edge: Edge,
        to: Time,
    },
    /// Moves the cut between `clip` and the clip right after it to `to`:
    /// one gets longer by what the other loses. Linked clips follow.
    Roll {
        sequence: SequenceId,
        clip: ClipId,
        to: Time,
    },
    /// Shows a different part of the file in the same place: the clip stays
    /// put and its contents shift by `by`. Linked clips follow.
    Slip {
        sequence: SequenceId,
        clip: ClipId,
        by: Time,
    },
    /// Moves a clip by `by` between its neighbours: the one before gets
    /// longer (or shorter) and the one after shorter (or longer), so nothing
    /// else on the timeline moves. Linked clips follow.
    Slide {
        sequence: SequenceId,
        clip: ClipId,
        by: Time,
    },
    /// Cuts these stretches out of the whole timeline (every track) and
    /// closes the gaps, as one step. Labels after a stretch move with it.
    RemoveRanges {
        sequence: SequenceId,
        ranges: Vec<Region>,
    },
    /// Splits at `at`: only `clip` and its linked clips, or every clip under `at` when `None`.
    Split {
        sequence: SequenceId,
        at: Time,
        clip: Option<ClipId>,
    },
    /// Adds a timeline label; its `id` is ignored and a new one is given.
    AddLabel {
        sequence: SequenceId,
        label: TimelineLabel,
    },
    RemoveLabel {
        sequence: SequenceId,
        label: LabelId,
    },
    /// Renames a timeline label and sets its colour.
    EditLabel {
        sequence: SequenceId,
        label: LabelId,
        name: String,
        color: LabelColor,
    },
    SetRegion {
        sequence: SequenceId,
        region: Option<Region>,
    },
    SetEditNeedle {
        sequence: SequenceId,
        at: Time,
    },
    /// The volume of the whole mix (decibels, -60 to +12).
    SetMasterGain {
        sequence: SequenceId,
        gain_db: f32,
    },
    /// A track's mute, solo and volume (decibels; 0 leaves the sound as it is).
    SetTrackMix {
        sequence: SequenceId,
        track: TrackId,
        muted: bool,
        solo: bool,
        gain_db: f32,
    },
    /// An audio track's role, pan, effects and volume and pan lines
    /// (format 16), all at once: one undo step per change on the Audio page.
    SetTrackSound {
        sequence: SequenceId,
        track: TrackId,
        sound: Box<TrackSound>,
    },
    /// A sound clip's own volume and pan lines (format 17), points at
    /// moments of its file: one undo step per change on the Audio page.
    SetClipLines {
        sequence: SequenceId,
        clip: ClipId,
        lines: Box<ClipLines>,
    },
    /// The mix's compressor and true-peak limiter (format 16).
    SetMasterSound {
        sequence: SequenceId,
        sound: MasterSound,
    },
    /// Fixes a timeline's picture size, frame rate and sound rate (it stops being automatic).
    SetSettings {
        sequence: SequenceId,
        settings: SequenceSettings,
    },
    /// Adds an empty timeline (one video and one audio track) with these
    /// settings, and opens it (format 12). Names are 1 to 40 characters and
    /// no two timelines share one, whatever their capitals.
    AddSequence {
        name: String,
        settings: SequenceSettings,
    },
    /// Adds a copy of a timeline under a new name, and opens it. Every track,
    /// clip, link and label of the copy has an id of its own; nested
    /// timelines in it stay the same timelines.
    DuplicateSequence {
        sequence: SequenceId,
        name: String,
    },
    RenameSequence {
        sequence: SequenceId,
        name: String,
    },
    /// Removes a timeline. Refused for the last one, and for one that clips
    /// in another timeline play. When it was open, the first one left opens.
    RemoveSequence {
        sequence: SequenceId,
    },
    /// Moves `clips` (with their linked clips) into a new timeline called
    /// `name`, with the same settings, each on the track of the same kind
    /// and place, from its start; one clip playing that timeline takes their
    /// place on the lowest video track they left, and one for its sound on
    /// the lowest audio track they left, linked. Refused when a clip that
    /// isn't moved lies between them on one of those two tracks.
    NestClips {
        sequence: SequenceId,
        clips: Vec<ClipId>,
        name: String,
    },
    /// Places the timeline `nested` in `sequence` at `at` (its end when
    /// `None`) as one piece: its picture on the video track `track` (the
    /// first when `None`), refused when that stretch isn't free there, and
    /// its sound on the first audio track free for its length (one is made
    /// when none is), linked. Refused when `nested` is `sequence` or plays
    /// it, directly or through other timelines.
    PlaceSequence {
        sequence: SequenceId,
        nested: SequenceId,
        at: Option<Time>,
        track: Option<TrackId>,
    },
    /// Lays an adjustment layer from `at` for `length` on the first video
    /// track from `track` up (from the first when `None`) that is free for
    /// that stretch, or on a new video track on top.
    AddAdjustment {
        sequence: SequenceId,
        at: Time,
        length: Time,
        track: Option<TrackId>,
    },
    /// Makes a media label (its `id` is ignored and a new one given) and
    /// puts it on `media` (format 13). Names are 1 to 40 characters and no
    /// two labels share one, whatever their capitals.
    AddMediaLabel {
        label: MediaLabel,
        media: Vec<MediaId>,
    },
    /// Renames a media label and sets its colour.
    EditMediaLabel {
        label: MediaLabelId,
        name: String,
        color: LabelColor,
    },
    /// Removes a media label, from every file that had it too.
    RemoveMediaLabel {
        label: MediaLabelId,
    },
    /// Puts a media label on files (`on`) or takes it off them.
    SetMediaLabel {
        media: Vec<MediaId>,
        label: MediaLabelId,
        on: bool,
    },
    /// Sets a file's note (trimmed); an empty one removes it.
    SetMediaNote {
        media: MediaId,
        note: String,
    },
    /// Makes a custom list called `name` holding `media`. Names follow the
    /// rules for labels.
    AddMediaList {
        name: String,
        media: Vec<MediaId>,
    },
    RenameMediaList {
        list: MediaListId,
        name: String,
    },
    /// Removes a custom list; its files stay in the project.
    RemoveMediaList {
        list: MediaListId,
    },
    /// Puts files at the end of a list (`member`) or takes them out.
    SetListMember {
        list: MediaListId,
        media: Vec<MediaId>,
        member: bool,
    },
    /// Sets how files are to be added from the Library (format 15): the
    /// part marked and the level of each sound stream, all as one step. An
    /// empty use removes what was kept.
    SetMediaUses {
        uses: Vec<(MediaId, MediaUse)>,
    },
}

impl Command {
    /// The new snapshot after this command, or why it could not be done.
    pub fn apply(&self, before: &Project) -> diag::Result<Project> {
        let mut p = before.clone();
        match self {
            Command::RenameProject { name } => {
                if name.trim().is_empty() {
                    return Err(AppError::new("A project needs a name").fix("Type at least one letter."));
                }
                p.name = name.clone();
            }
            Command::AddMedia { media } => {
                let id: MediaId = p.allocate_id();
                p.media.insert(id, MediaRef { id, ..media.clone() });
            }
            Command::SetSpeed {
                sequence,
                clip,
                speed,
            } => {
                crate::edits::set_speed(&mut p, *sequence, *clip, *speed)?;
            }
            Command::SetRamp { sequence, clip, ramp } => {
                crate::edits::set_ramp(&mut p, *sequence, *clip, ramp)?;
            }
            Command::SetFrameBlend {
                sequence,
                clip,
                blend,
            } => {
                let seq = sequence_mut(&mut p, *sequence)?;
                if seq
                    .audio_tracks
                    .iter()
                    .any(|t| t.clips.iter().any(|c| c.id == *clip))
                {
                    return Err(AppError::new("Sound has no frames to blend")
                        .why("Frame blending mixes pictures, and belongs to clips on video tracks.")
                        .fix("Select the clip's picture on a video track."));
                }
                crate::animate::picture_clip(&mut p, *sequence, *clip)?.0.blend = *blend;
            }
            Command::SetLook { sequence, clip, look } => {
                look.check()?;
                crate::animate::picture_clip(&mut p, *sequence, *clip)?.0.look = *look;
            }
            Command::SetLookAt {
                sequence,
                clip,
                at,
                look,
            } => crate::animate::set_look_at(&mut p, *sequence, *clip, *at, look)?,
            Command::Keyframe {
                sequence,
                clip,
                at,
                fields,
                change,
            } => crate::animate::keyframe(&mut p, *sequence, *clip, *at, fields, *change)?,
            Command::ChangeKeyframes {
                sequence,
                clip,
                frames,
                change,
            } => crate::animate::change_keyframes(&mut p, *sequence, *clip, frames, *change)?,
            Command::PasteKeyframes {
                sequence,
                clip,
                at,
                keyframes,
            } => crate::clipboard::paste_keyframes(&mut p, *sequence, *clip, *at, keyframes)?,
            Command::ResetLook {
                sequence,
                clip,
                fields,
            } => crate::animate::reset_look(&mut p, *sequence, *clip, fields)?,
            Command::SetSound { sequence, sounds } => set_sound(&mut p, *sequence, sounds)?,
            Command::Restore { project } => {
                if project.active().is_none() {
                    return Err(AppError::new("That version can't be restored")
                        .why("It has no timeline open.")
                        .fix("Pick another version."));
                }
                p = (**project).clone();
            }
            Command::InsertTemplate {
                sequence,
                template,
                at,
            } => crate::templates::insert(&mut p, *sequence, template, *at)?,
            Command::RemoveMedia { media } => crate::prune::remove_media(&mut p, media)?,
            Command::Relink { media, path } => {
                p.media.get_mut(media).ok_or_else(|| missing("media file"))?.path = path.clone();
            }
            Command::RelinkFiles { files } => {
                if files.is_empty() {
                    return Err(AppError::new("No files to relink")
                        .why("None of the missing files were found.")
                        .fix("Look for them in another folder."));
                }
                for (media, path) in files {
                    p.media.get_mut(media).ok_or_else(|| missing("media file"))?.path = path.clone();
                }
            }
            Command::Import { sequence, media } => import(&mut p, *sequence, media)?,
            Command::AddMediaLabel { label, media } => crate::library::add_label(&mut p, label, media)?,
            Command::EditMediaLabel { label, name, color } => {
                crate::library::edit_label(&mut p, *label, name, *color)?
            }
            Command::RemoveMediaLabel { label } => crate::library::remove_label(&mut p, *label)?,
            Command::SetMediaLabel { media, label, on } => {
                crate::library::set_label(&mut p, media, *label, *on)?
            }
            Command::SetMediaNote { media, note } => crate::library::set_note(&mut p, *media, note)?,
            Command::AddMediaList { name, media } => crate::library::add_list(&mut p, name, media)?,
            Command::RenameMediaList { list, name } => crate::library::rename_list(&mut p, *list, name)?,
            Command::RemoveMediaList { list } => crate::library::remove_list(&mut p, *list)?,
            Command::SetMediaUses { uses } => crate::library::set_uses(&mut p, uses)?,
            Command::SetListMember { list, media, member } => {
                crate::library::set_member(&mut p, *list, media, *member)?
            }
            Command::PlaceMedia { sequence, media } => place_media(&mut p, *sequence, *media, None, None)?,
            Command::InsertMedia { sequence, media, at } => {
                let length = placed_length(p.media.get(media).ok_or_else(|| missing("media file"))?);
                crate::edits::open_space(&mut p, *sequence, *at, length)?;
                place_media(&mut p, *sequence, *media, Some(*at), None)?;
            }
            Command::OverwriteMedia { sequence, media, at } => {
                let length = placed_length(p.media.get(media).ok_or_else(|| missing("media file"))?);
                crate::edits::clear_space(&mut p, *sequence, *at, length)?;
                place_media(&mut p, *sequence, *media, Some(*at), None)?;
            }
            Command::DropMedia {
                sequence,
                media,
                at,
                track,
                insert,
            } => drop_media(&mut p, *sequence, *media, *at, *track, *insert)?,
            Command::LayMedia {
                sequence,
                media,
                at,
                track,
            } => crate::layers::lay(&mut p, *sequence, *media, *at, *track)?,
            Command::AddTrack { sequence, kind, name } => {
                crate::tracks::add(&mut p, *sequence, *kind, name)?;
            }
            Command::RemoveTrack { sequence, track } => crate::tracks::remove(&mut p, *sequence, *track)?,
            Command::RenameTrack {
                sequence,
                track,
                name,
            } => crate::tracks::rename(&mut p, *sequence, *track, name)?,
            Command::SetTrackHidden {
                sequence,
                track,
                hidden,
            } => crate::tracks::set_hidden(&mut p, *sequence, *track, *hidden)?,
            Command::PlaceClip {
                sequence,
                track,
                clip,
            } => {
                if clip.is_file() && !p.media.contains_key(&clip.media) {
                    return Err(missing("media file"));
                }
                if clip.duration <= Time::ZERO || clip.start.is_negative() || clip.starts_before_its_source()
                {
                    return Err(AppError::new("The clip can't be placed there")
                        .why("It would start before the timeline or have no length."));
                }
                let id: ClipId = p.allocate_id();
                let track = track_mut(sequence_mut(&mut p, *sequence)?, *track)?;
                insert_clip(track, Clip { id, ..clip.clone() })?;
            }
            Command::RemoveClip {
                sequence,
                clip,
                ripple,
            } => remove_clips(&mut p, *sequence, &[*clip], *ripple)?,
            Command::RemoveClips {
                sequence,
                clips,
                ripple,
            } => {
                if clips.is_empty() {
                    return Err(AppError::new("No clips to remove")
                        .why("None were selected.")
                        .fix("Click a clip on the timeline first."));
                }
                remove_clips(&mut p, *sequence, clips, *ripple)?
            }
            Command::MoveClip {
                sequence,
                clip,
                start,
                track,
            } => {
                if start.is_negative() {
                    return Err(
                        AppError::new("The clip can't move there").why("That is before the timeline starts.")
                    );
                }
                let seq = sequence_mut(&mut p, *sequence)?;
                let (from, mut moved) = take_clip(seq, *clip)?;
                moved.start = *start;
                let to = track.unwrap_or(from);
                let from_kind = track_mut(seq, from)?.kind;
                let target = track_mut(seq, to)?;
                if target.kind != from_kind {
                    return Err(wrong_kind_of_track());
                }
                insert_clip(target, moved)?;
            }
            Command::MoveLinked { sequence, clip, by } => move_linked(&mut p, *sequence, *clip, *by, None)?,
            Command::MoveToTrack {
                sequence,
                clip,
                track,
                by,
            } => move_linked(&mut p, *sequence, *clip, *by, Some(*track))?,
            Command::MoveBetweenTracks {
                sequence,
                clip,
                track,
                by,
            } => crate::layers::move_between(&mut p, *sequence, *clip, *track, *by)?,
            Command::LineUp { sequence, moves } => crate::line_up::line_up(&mut p, *sequence, moves)?,
            Command::Trim {
                sequence,
                clip,
                edge,
                to,
            } => trim(&mut p, *sequence, *clip, *edge, *to)?,
            Command::RippleTrim {
                sequence,
                clip,
                edge,
                to,
            } => crate::edits::ripple_trim(&mut p, *sequence, *clip, *edge, *to)?,
            Command::Roll { sequence, clip, to } => crate::edits::roll(&mut p, *sequence, *clip, *to)?,
            Command::Slip { sequence, clip, by } => crate::edits::slip(&mut p, *sequence, *clip, *by)?,
            Command::Slide { sequence, clip, by } => crate::edits::slide(&mut p, *sequence, *clip, *by)?,
            Command::RemoveRanges { sequence, ranges } => {
                crate::edits::remove_ranges(&mut p, *sequence, ranges)?
            }
            Command::Split { sequence, at, clip } => split(&mut p, *sequence, *at, *clip)?,
            Command::AddLabel { sequence, label } => {
                if label.at.is_negative() {
                    return Err(
                        AppError::new("A label can't go there").why("That is before the timeline starts.")
                    );
                }
                let id: LabelId = p.allocate_id();
                let seq = sequence_mut(&mut p, *sequence)?;
                let index = seq
                    .labels
                    .iter()
                    .position(|l| l.at > label.at)
                    .unwrap_or(seq.labels.len());
                seq.labels.insert(index, TimelineLabel { id, ..label.clone() });
            }
            Command::EditLabel {
                sequence,
                label,
                name,
                color,
            } => {
                if name.trim().is_empty() {
                    return Err(AppError::new("A label needs a name").fix("Type at least one letter."));
                }
                let seq = sequence_mut(&mut p, *sequence)?;
                let l = seq
                    .labels
                    .iter_mut()
                    .find(|l| l.id == *label)
                    .ok_or_else(|| missing("label"))?;
                l.name = name.trim().to_owned();
                l.color = *color;
            }
            Command::RemoveLabel { sequence, label } => {
                let seq = sequence_mut(&mut p, *sequence)?;
                let before_len = seq.labels.len();
                seq.labels.retain(|l| l.id != *label);
                if seq.labels.len() == before_len {
                    return Err(missing("label"));
                }
            }
            Command::SetRegion { sequence, region } => {
                if region.is_some_and(|r| r.start >= r.end || r.start.is_negative()) {
                    return Err(AppError::new("That region can't be set")
                        .why("A region must start at or after zero and end after it starts."));
                }
                sequence_mut(&mut p, *sequence)?.region = *region;
            }
            Command::SetMasterGain { sequence, gain_db } => {
                if !(-60.0..=12.0).contains(gain_db) {
                    return Err(AppError::new("That volume is out of range")
                        .why("The mix's volume goes from -60 dB to +12 dB."));
                }
                sequence_mut(&mut p, *sequence)?.master_gain_db = *gain_db;
            }
            Command::SetTrackMix {
                sequence,
                track,
                muted,
                solo,
                gain_db,
            } => {
                if !gain_db.is_finite() || !(-60.0..=12.0).contains(gain_db) {
                    return Err(AppError::new("That volume can't be set")
                        .why("Track volume goes from -60 dB (nearly silent) to +12 dB."));
                }
                let t = track_mut(sequence_mut(&mut p, *sequence)?, *track)?;
                t.muted = *muted;
                t.solo = *solo;
                t.gain_db = *gain_db;
            }
            Command::SetTrackSound {
                sequence,
                track,
                sound,
            } => {
                sound.check()?;
                let t = track_mut(sequence_mut(&mut p, *sequence)?, *track)?;
                if t.kind != TrackKind::Audio {
                    return Err(AppError::new("That sound setting can't be used")
                        .why("Effects, pan and lines are set on audio tracks; a picture's sound is on its linked audio track."));
                }
                t.sound = (**sound).clone();
            }
            Command::SetClipLines {
                sequence,
                clip,
                lines,
            } => {
                lines.check()?;
                let seq = sequence_mut(&mut p, *sequence)?;
                if seq
                    .video_tracks
                    .iter()
                    .any(|t| t.clips.iter().any(|c| c.id == *clip))
                {
                    return Err(AppError::new("A picture has no sound of its own")
                        .why("Volume and pan lines are drawn on sound clips, on audio tracks.")
                        .fix("Draw the line on the picture's linked sound clip."));
                }
                let found = seq
                    .audio_tracks
                    .iter_mut()
                    .flat_map(|track| track.clips.iter_mut())
                    .find(|c| c.id == *clip)
                    .ok_or_else(|| missing("clip"))?;
                found.lines = (**lines).clone();
            }
            Command::SetMasterSound { sequence, sound } => {
                sound.check()?;
                sequence_mut(&mut p, *sequence)?.master_sound = *sound;
            }
            Command::SetSettings { sequence, settings } => {
                crate::timelines::check_settings(settings)?;
                let seq = sequence_mut(&mut p, *sequence)?;
                seq.settings = *settings;
                seq.automatic = false;
            }
            Command::SetEditNeedle { sequence, at } => {
                sequence_mut(&mut p, *sequence)?.edit_needle = (*at).max(Time::ZERO);
            }
            Command::AddSequence { name, settings } => crate::timelines::add(&mut p, name, *settings)?,
            Command::DuplicateSequence { sequence, name } => {
                crate::timelines::duplicate(&mut p, *sequence, name)?
            }
            Command::RenameSequence { sequence, name } => crate::timelines::rename(&mut p, *sequence, name)?,
            Command::RemoveSequence { sequence } => crate::timelines::remove(&mut p, *sequence)?,
            Command::NestClips {
                sequence,
                clips,
                name,
            } => crate::timelines::nest(&mut p, *sequence, clips, name)?,
            Command::PlaceSequence {
                sequence,
                nested,
                at,
                track,
            } => crate::timelines::place(&mut p, *sequence, *nested, *at, *track)?,
            Command::AddAdjustment {
                sequence,
                at,
                length,
                track,
            } => crate::timelines::add_adjustment(&mut p, *sequence, *at, *length, *track)?,
        }
        // A clip made shorter keeps its fades within it.
        p.fit_fades();
        p.validate()?;
        Ok(p)
    }
}

fn set_sound(p: &mut Project, sequence: SequenceId, sounds: &[(ClipId, Sound)]) -> diag::Result<()> {
    if sounds.is_empty() {
        return Err(AppError::new("No sound to set").why("No clips were given."));
    }
    let seq = sequence_mut(p, sequence)?;
    for (clip, sound) in sounds {
        if seq
            .video_tracks
            .iter()
            .any(|t| t.clips.iter().any(|c| c.id == *clip))
        {
            return Err(AppError::new("A picture has no sound of its own")
                .why("A recording's sound is on audio tracks, in the sound clips linked to its picture.")
                .fix("Select the picture or its sound clip: the inspector's Audio section sets the linked sound."));
        }
        let found = seq
            .audio_tracks
            .iter_mut()
            .flat_map(|track| track.clips.iter_mut())
            .find(|c| c.id == *clip)
            .ok_or_else(|| missing("clip"))?;
        sound.check(found.duration)?;
        found.sound = *sound;
    }
    Ok(())
}

/// One end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Start,
    End,
}

/// Moves `clip` and the clips linked to it by `by`. With `to`, `clip`
/// itself lands on that track (of its own kind); the linked ones stay on theirs.
fn move_linked(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    by: Time,
    to: Option<TrackId>,
) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let group = with_linked(seq, clip)?;
    let mut moved = Vec::new();
    for track in tracks_mut(seq) {
        let (ours, rest): (Vec<Clip>, Vec<Clip>) =
            track.clips.iter().cloned().partition(|c| group.contains(&c.id));
        if ours.is_empty() {
            continue;
        }
        track.clips = rest.into_iter().collect();
        moved.push((track.id, track.kind, ours));
    }
    for (track, kind, clips) in moved {
        for mut c in clips {
            c.start = c.start + by;
            if c.start.is_negative() {
                return Err(
                    AppError::new("The clip can't move there").why("That is before the timeline starts.")
                );
            }
            let target = track_mut(seq, to.filter(|_| c.id == clip).unwrap_or(track))?;
            if target.kind != kind {
                return Err(wrong_kind_of_track());
            }
            insert_clip(target, c)?;
        }
    }
    Ok(())
}

pub(crate) fn wrong_kind_of_track() -> AppError {
    AppError::new("The clip can't move to that track")
        .why("Video clips go on video tracks and audio clips on audio tracks.")
}

fn trim(p: &mut Project, sequence: SequenceId, clip: ClipId, edge: Edge, to: Time) -> diag::Result<()> {
    let lengths: Vec<(ClipId, Option<Time>)> = sequence_ref(p, sequence)?
        .tracks()
        .flat_map(|t| t.clips.iter())
        .map(|c| (c.id, p.source_length(c)))
        .collect();
    let seq = sequence_mut(p, sequence)?;
    let reference = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .cloned()
        .ok_or_else(|| missing("clip"))?;
    let group = with_linked(seq, clip)?;
    let delta = match edge {
        Edge::Start => to - reference.start,
        Edge::End => to - reference.end(),
    };
    for track in tracks_mut(seq) {
        let mut changed = Vec::new();
        for c in track.clips.iter().filter(|c| group.contains(&c.id)) {
            let mut t = c.clone();
            match edge {
                Edge::Start => {
                    t = c.start_cut_by(delta);
                    t.start = c.start + delta;
                    t.duration = c.duration - delta;
                }
                Edge::End => t.duration = c.duration + delta,
            }
            if t.duration <= Time::ZERO {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("It would have no length left.")
                    .fix("Delete the clip instead."));
            }
            if t.starts_before_its_source() || t.start.is_negative() {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("Its file has nothing earlier to show."));
            }
            let length = lengths.iter().find(|(id, _)| *id == c.id).and_then(|(_, l)| *l);
            if let Some(length) = length
                && t.source_in + t.source_len() > length
            {
                return Err(
                    AppError::new("The clip can't be trimmed that far").why("Its file ends before that.")
                );
            }
            changed.push(t);
        }
        for t in changed {
            track.clips.retain(|c| c.id != t.id);
            insert_clip(track, t)?;
        }
    }
    Ok(())
}

/// How long a still image lasts when it is placed.
pub const STILL_LENGTH: Time = Time::seconds(5);

fn import(p: &mut Project, sequence: SequenceId, media: &MediaRef) -> diag::Result<()> {
    settle_automatic(p, sequence, media)?;
    let id: MediaId = p.allocate_id();
    p.media.insert(id, MediaRef { id, ..media.clone() });
    place_media(p, sequence, id, None, None)
}

/// An automatic timeline takes its settings from the first video it gets.
pub(crate) fn settle_automatic(p: &mut Project, sequence: SequenceId, media: &MediaRef) -> diag::Result<()> {
    if let Some(v) = &media.info.video
        && media.kind == MediaKind::Video
    {
        let seq = sequence_mut(p, sequence)?;
        if seq.automatic {
            seq.settings = SequenceSettings::matching(v, &media.info.audio);
            seq.automatic = false;
        }
    }
    Ok(())
}

/// How long a file's clips are when placed.
pub(crate) fn placed_length(media: &MediaRef) -> Time {
    match media.kind {
        MediaKind::Image => STILL_LENGTH,
        MediaKind::Video | MediaKind::Audio => media.duration,
    }
}

/// How long a file's clips are when placed, or why it can't be placed: its
/// length is not known, or it has neither picture nor sound.
pub(crate) fn placeable_length(media: &MediaRef) -> diag::Result<Time> {
    let length = placed_length(media);
    if length <= Time::ZERO {
        return Err(AppError::new("This file can't be placed")
            .why("Its length could not be read, so the app doesn't know how long the clip is.")
            .fix("Try re-saving the file with a recording or conversion tool, then import it again."));
    }
    let (video, audio) = placed_streams(media);
    if video.is_none() && audio.is_empty() {
        return Err(
            AppError::new("This file has nothing to place").why("No picture or sound was found in it.")
        );
    }
    Ok(length)
}

/// What of a file is placed: its picture stream (none for a sound file) and
/// its sound streams (none for a still).
pub(crate) fn placed_streams(
    media: &MediaRef,
) -> (Option<&crate::model::VideoStream>, &[crate::model::AudioStream]) {
    let video = media
        .info
        .video
        .as_ref()
        .filter(|_| media.kind != MediaKind::Audio);
    let audio = if media.kind == MediaKind::Image {
        &[][..]
    } else {
        &media.info.audio[..]
    };
    (video, audio)
}

/// The video track a file's picture goes on: `chosen`, or the first one
/// (`None` when the timeline has no video track yet: one is made).
pub(crate) fn picture_track(seq: &Sequence, chosen: Option<TrackId>) -> diag::Result<Option<TrackId>> {
    match chosen {
        Some(id) => match seq.track(id) {
            Some(track) if track.kind == TrackKind::Video => Ok(Some(id)),
            Some(track) => Err(AppError::new("The picture can't go on that track")
                .why(format!("{} is an audio track.", track.name))
                .fix("Drop the file on a video track.")),
            None => Err(missing("track")),
        },
        None => Ok(seq.video_tracks.front().map(|t| t.id)),
    }
}

/// Places a file of the project at `at` on the track under the pointer:
/// pushing everything later, or over what is on the tracks it lands on.
fn drop_media(
    p: &mut Project,
    sequence: SequenceId,
    id: MediaId,
    at: Time,
    track: Option<TrackId>,
    insert: bool,
) -> diag::Result<()> {
    if at.is_negative() {
        return Err(
            AppError::new("The file can't be placed there").why("That is before the timeline starts.")
        );
    }
    let media = p.media.get(&id).cloned().ok_or_else(|| missing("media file"))?;
    let length = placed_length(&media);
    let seq = sequence_ref(p, sequence)?;
    let track = picture_track(seq, track)?;
    if insert {
        crate::edits::open_space(p, sequence, at, length)?;
    } else {
        // The tracks it lands on: its picture's, and one audio track per
        // sound stream, from the first.
        let (video, audio) = placed_streams(&media);
        let landing: Vec<TrackId> = track
            .filter(|_| video.is_some())
            .into_iter()
            .chain(seq.audio_tracks.iter().take(audio.len()).map(|t| t.id))
            .collect();
        crate::edits::clear_space_on(p, sequence, at, length, &landing)?;
    }
    place_media(p, sequence, id, Some(at), track)
}

/// Places a file of the project at `at` (the end of the timeline when
/// `None`): its picture on the video track `picture_on` (the first when
/// `None`) and each sound stream on its own audio track (made when missing,
/// named after the stream), all linked.
fn place_media(
    p: &mut Project,
    sequence: SequenceId,
    id: MediaId,
    at: Option<Time>,
    picture_on: Option<TrackId>,
) -> diag::Result<()> {
    let media = p.media.get(&id).cloned().ok_or_else(|| missing("media file"))?;
    let media = &media;
    let length = placeable_length(media)?;
    let (video, audio) = placed_streams(media);
    let link: Option<LinkId> = (usize::from(video.is_some()) + audio.len() > 1).then(|| p.allocate_id());
    let at = match at {
        Some(at) => at,
        None => sequence_ref(p, sequence)?.length(),
    };
    let clip = |p: &mut Project, stream: u32| Clip {
        lines: Default::default(),
        id: p.allocate_id(),
        media: id,
        stream,
        start: at,
        source_in: Time::ZERO,
        duration: length,
        link,
        speed: Speed::NORMAL,
        look: Look::PLAIN,
        sound: Default::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
        kind: Default::default(),
    };

    if let Some(v) = video {
        let c = clip(p, v.index);
        if sequence_ref(p, sequence)?.video_tracks.is_empty() {
            let track = p.new_track(TrackKind::Video, "V1");
            sequence_mut(p, sequence)?.video_tracks.push_back(track);
        }
        let seq = sequence_mut(p, sequence)?;
        let track = match picture_track(seq, picture_on)? {
            Some(id) => track_mut(seq, id)?,
            None => return Err(missing("track")),
        };
        insert_clip(track, c)?;
    }
    for (i, stream) in audio.iter().enumerate() {
        let c = clip(p, stream.index);
        if sequence_ref(p, sequence)?.audio_tracks.len() <= i {
            let track = p.new_track(TrackKind::Audio, &stream.name);
            sequence_mut(p, sequence)?.audio_tracks.push_back(track);
        }
        let track = &mut sequence_mut(p, sequence)?.audio_tracks[i];
        // A track nobody has used yet takes the stream's name.
        if track.clips.is_empty() && track.name == format!("A{}", i + 1) {
            track.name = stream.name.clone();
        }
        insert_clip(track, c)?;
    }
    Ok(())
}

pub(crate) fn split(
    p: &mut Project,
    sequence: SequenceId,
    at: Time,
    only: Option<ClipId>,
) -> diag::Result<()> {
    let targets = match only {
        Some(clip) => Some(with_linked(sequence_ref(p, sequence)?, clip)?),
        None => None,
    };
    // Clips under `at` to cut, with a new link per original link so the right halves stay linked together.
    let mut cuts: Vec<(ClipId, Option<LinkId>)> = Vec::new();
    for track in sequence_ref(p, sequence)?.tracks() {
        for clip in &track.clips {
            let chosen = targets.as_ref().is_none_or(|t| t.contains(&clip.id));
            if chosen && clip.start < at && at < clip.end() {
                cuts.push((clip.id, clip.link));
            }
        }
    }
    if cuts.is_empty() {
        return Err(AppError::new("Nothing to split here")
            .why("No clip crosses the edit needle at this point.")
            .fix("Move the edit needle over a clip, then split."));
    }
    let mut new_links: Vec<(LinkId, LinkId)> = Vec::new();
    let mut right_ids: Vec<(ClipId, ClipId, Option<LinkId>)> = Vec::new();
    for (id, link) in cuts {
        let new_link = link.map(|old| match new_links.iter().find(|(o, _)| *o == old) {
            Some((_, n)) => *n,
            None => {
                let n = p.allocate_id();
                new_links.push((old, n));
                n
            }
        });
        right_ids.push((id, p.allocate_id(), new_link));
    }
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        for (left_id, right_id, right_link) in &right_ids {
            if let Some(i) = track.clips.iter().position(|c| c.id == *left_id) {
                let left = track.clips[i].clone();
                let first = at - left.start;
                // Volume and pan stay on both halves; the fade in stays at
                // the start, on the left half, and the fade out at the end,
                // on the right (each cut to fit its half after the change).
                // Each half keeps the keyframes of its own part of the
                // file, and one at the cut holding the value there.
                let cut = left.source_at(at);
                let right = Clip {
                    id: *right_id,
                    start: at,
                    source_in: cut,
                    duration: left.duration - first,
                    link: *right_link,
                    sound: left.sound.without_fade_in(),
                    keyframes: left.keyframes.cut_to(cut, left.source_at(left.end())),
                    // Both halves keep the whole ramp, in its place.
                    ramp: left.ramp.shifted(Time::ZERO - first),
                    ..left.clone()
                };
                track.clips[i] = Clip {
                    duration: first,
                    sound: left.sound.without_fade_out(),
                    keyframes: left.keyframes.cut_to(left.source_in, cut),
                    ..left
                };
                track.clips.insert(i + 1, right);
            }
        }
    }
    Ok(())
}

pub(crate) fn sequence_ref(p: &Project, id: SequenceId) -> diag::Result<&Sequence> {
    p.sequences
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| missing("timeline"))
}

pub(crate) fn sequence_mut(p: &mut Project, id: SequenceId) -> diag::Result<&mut Sequence> {
    p.sequences
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| missing("timeline"))
}

pub(crate) fn tracks_mut(seq: &mut Sequence) -> impl Iterator<Item = &mut Track> {
    seq.video_tracks.iter_mut().chain(seq.audio_tracks.iter_mut())
}

pub(crate) fn track_mut(seq: &mut Sequence, id: TrackId) -> diag::Result<&mut Track> {
    tracks_mut(seq)
        .find(|t| t.id == id)
        .ok_or_else(|| missing("track"))
}

/// `clip` and every clip linked to it, on any track.
pub(crate) fn with_linked(seq: &Sequence, clip: ClipId) -> diag::Result<Vec<ClipId>> {
    let link = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .ok_or_else(|| missing("clip"))?
        .link;
    Ok(seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.id == clip || (link.is_some() && c.link == link))
        .map(|c| c.id)
        .collect())
}

/// Removes `clips` and every clip linked to them. Each is looked up before
/// anything changes, so a missing one leaves the project as it was. With
/// `ripple`, each track closes its gaps from the last one back, which is
/// what removing the clips one at a time would do.
fn remove_clips(p: &mut Project, sequence: SequenceId, clips: &[ClipId], ripple: bool) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let mut targets = Vec::new();
    for clip in clips {
        targets.extend(with_linked(seq, *clip)?);
    }
    for track in tracks_mut(seq) {
        let removed: Vec<Clip> = track
            .clips
            .iter()
            .filter(|c| targets.contains(&c.id))
            .cloned()
            .collect();
        track.clips.retain(|c| !targets.contains(&c.id));
        if ripple {
            for gone in removed.iter().rev() {
                close_gap(track, gone.start, gone.duration);
            }
        }
    }
    Ok(())
}

/// Removes a clip from whichever track holds it.
fn take_clip(seq: &mut Sequence, clip: ClipId) -> diag::Result<(TrackId, Clip)> {
    for track in tracks_mut(seq) {
        if let Some(i) = track.clips.iter().position(|c| c.id == clip) {
            return Ok((track.id, track.clips.remove(i)));
        }
    }
    Err(missing("clip"))
}

/// Inserts in start order; refuses an overlap.
pub(crate) fn insert_clip(track: &mut Track, clip: Clip) -> diag::Result<()> {
    if track
        .clips
        .iter()
        .any(|c| c.start < clip.end() && clip.start < c.end())
    {
        return Err(AppError::new("There is already a clip there")
            .why(format!("It would overlap another clip on {}.", track.name))
            .fix("Move it to an empty spot or another track."));
    }
    let index = track
        .clips
        .iter()
        .position(|c| c.start > clip.start)
        .unwrap_or(track.clips.len());
    track.clips.insert(index, clip);
    Ok(())
}

/// Moves every clip that starts at or after `from` back by `by`.
fn close_gap(track: &mut Track, from: Time, by: Time) {
    for clip in track.clips.iter_mut() {
        if clip.start >= from {
            clip.start = clip.start - by;
        }
    }
}

pub(crate) fn missing(thing: &str) -> AppError {
    AppError::new(format!("That {thing} no longer exists"))
        .why("It may have been removed by an earlier change.")
        .fix("Undo the last change if this was unexpected.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioStream, Fingerprint, MediaInfo, SequenceSettings, VideoStream};
    use crate::time::Rate;

    struct Fixture {
        p: Project,
        seq: SequenceId,
        v1: TrackId,
        a1: TrackId,
        media: MediaId,
    }

    fn fixture() -> Fixture {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let p = Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: "rec.mkv".into(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: "x".into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(100),
                info: MediaInfo::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        Fixture {
            seq: seq.id,
            v1: seq.video_tracks[0].id,
            a1: seq.audio_tracks[0].id,
            media: *p.media.keys().next().unwrap(),
            p,
        }
    }

    fn clip(f: &Fixture, start: i64, len: i64, link: Option<LinkId>) -> Clip {
        Clip {
            lines: Default::default(),
            id: ClipId(0),
            media: f.media,
            stream: 0,
            start: Time::seconds(start),
            source_in: Time::ZERO,
            duration: Time::seconds(len),
            link,
            speed: Default::default(),
            look: Default::default(),
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
        }
    }

    fn place(f: &Fixture, p: &Project, track: TrackId, c: Clip) -> diag::Result<Project> {
        Command::PlaceClip {
            sequence: f.seq,
            track,
            clip: c,
        }
        .apply(p)
    }

    fn clips(p: &Project, track: usize, audio: bool) -> Vec<(i64, i64)> {
        let seq = p.active().unwrap();
        let t = if audio {
            &seq.audio_tracks[track]
        } else {
            &seq.video_tracks[track]
        };
        t.clips
            .iter()
            .map(|c| (c.start.as_secs_f64() as i64, c.duration.as_secs_f64() as i64))
            .collect()
    }

    #[test]
    fn overlapping_placement_is_refused_and_nothing_changes() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let err = place(&f, &p, f.v1, clip(&f, 5, 10, None)).unwrap_err();
        assert!(err.what().contains("already a clip"));
        assert_eq!(clips(&p, 0, false), vec![(0, 10)]);
    }

    #[test]
    fn split_with_a_clip_selected_cuts_it_and_its_linked_audio_only() {
        let mut f = fixture();
        let link = Some(f.p.allocate_id());
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 20, 10, None)).unwrap();
        let v = p.active().unwrap().video_tracks[0].clips[0].id;
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(4),
            clip: Some(v),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&p, 0, false), vec![(0, 4), (4, 6), (20, 10)]);
        assert_eq!(clips(&p, 0, true), vec![(0, 4), (4, 6)]);
        let seq = p.active().unwrap();
        let (rv, ra) = (&seq.video_tracks[0].clips[1], &seq.audio_tracks[0].clips[1]);
        assert_eq!(rv.link, ra.link, "the right halves stay linked to each other");
        assert_ne!(rv.link, link, "but not to the left halves");
        assert_eq!(rv.source_in, Time::seconds(4));
    }

    #[test]
    fn split_with_nothing_selected_cuts_every_track() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 2, 10, None)).unwrap();
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(5),
            clip: None,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&p, 0, false), vec![(0, 5), (5, 5)]);
        assert_eq!(clips(&p, 0, true), vec![(2, 3), (5, 7)]);
        assert!(
            Command::Split {
                sequence: f.seq,
                at: Time::seconds(50),
                clip: None
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn delete_leaves_a_gap_and_ripple_delete_closes_it() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 20, 5, None)).unwrap();
        let middle = p.active().unwrap().video_tracks[0].clips[1].id;
        let gap = Command::RemoveClip {
            sequence: f.seq,
            clip: middle,
            ripple: false,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&gap, 0, false), vec![(0, 10), (20, 5)]);
        let closed = Command::RemoveClip {
            sequence: f.seq,
            clip: middle,
            ripple: true,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&closed, 0, false), vec![(0, 10), (15, 5)]);
    }

    #[test]
    fn removing_several_clips_is_one_step_and_matches_removing_them_one_by_one() {
        let mut f = fixture();
        let link = Some(f.p.allocate_id());
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 20, 5, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 30, 5, None)).unwrap();
        let v = &p.active().unwrap().video_tracks[0];
        // The linked pair's audio goes with its picture, even when both are picked.
        let a = p.active().unwrap().audio_tracks[0].clips[0].id;
        let picked = vec![v.clips[0].id, a, v.clips[2].id];
        for ripple in [false, true] {
            let together = Command::RemoveClips {
                sequence: f.seq,
                clips: picked.clone(),
                ripple,
            }
            .apply(&p)
            .unwrap();
            let mut one_by_one = p.clone();
            for id in [picked[0], picked[2]] {
                one_by_one = Command::RemoveClip {
                    sequence: f.seq,
                    clip: id,
                    ripple,
                }
                .apply(&one_by_one)
                .unwrap();
            }
            assert_eq!(clips(&together, 0, false), clips(&one_by_one, 0, false));
            assert_eq!(clips(&together, 0, true), clips(&one_by_one, 0, true));
            assert!(clips(&together, 0, true).is_empty());
        }
        let gaps = Command::RemoveClips {
            sequence: f.seq,
            clips: picked,
            ripple: true,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&gaps, 0, false), vec![(0, 5), (15, 5)]);
    }

    #[test]
    fn removing_several_clips_changes_nothing_when_one_is_gone() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
        let first = p.active().unwrap().video_tracks[0].clips[0].id;
        let err = Command::RemoveClips {
            sequence: f.seq,
            clips: vec![first, ClipId(9999)],
            ripple: false,
        }
        .apply(&p)
        .unwrap_err();
        assert!(err.what().contains("no longer exists"));
        let none = Command::RemoveClips {
            sequence: f.seq,
            clips: Vec::new(),
            ripple: false,
        }
        .apply(&p);
        assert!(none.is_err());
    }

    #[test]
    fn moving_keeps_order_and_refuses_the_wrong_kind_of_track() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 5, None)).unwrap();
        let id = p.active().unwrap().video_tracks[0].clips[0].id;
        let moved = Command::MoveClip {
            sequence: f.seq,
            clip: id,
            start: Time::seconds(30),
            track: None,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&moved, 0, false), vec![(30, 5)]);
        let wrong = Command::MoveClip {
            sequence: f.seq,
            clip: id,
            start: Time::ZERO,
            track: Some(f.a1),
        }
        .apply(&p);
        assert!(wrong.is_err());
    }

    #[test]
    fn labels_stay_in_time_order() {
        let f = fixture();
        let label = |at| TimelineLabel {
            id: LabelId(0),
            name: "x".into(),
            at: Time::seconds(at),
            color: crate::model::LabelColor::Blue,
        };
        let p = Command::AddLabel {
            sequence: f.seq,
            label: label(30),
        }
        .apply(&f.p)
        .unwrap();
        let p = Command::AddLabel {
            sequence: f.seq,
            label: label(10),
        }
        .apply(&p)
        .unwrap();
        let times: Vec<_> = p.active().unwrap().labels.iter().map(|l| l.at).collect();
        assert_eq!(times, vec![Time::seconds(10), Time::seconds(30)]);
    }

    fn recording(audio: &[&str], seconds: i64) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: "match.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: "y".into(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(seconds),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 2560,
                    height: 1440,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: audio
                    .iter()
                    .enumerate()
                    .map(|(i, name)| AudioStream {
                        index: i as u32 + 1,
                        codec: "aac".into(),
                        channels: 2,
                        sample_rate: 48_000,
                        name: (*name).into(),
                    })
                    .collect(),
                stated_length: None,
            },
        }
    }

    #[test]
    fn importing_puts_each_sound_stream_on_its_own_named_track() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game", "Mic", "Voice chat"], 30),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game", "Mic", "Voice chat"]);
        assert_eq!(s.video_tracks[0].clips.len(), 1);
        let all: Vec<&Clip> = s.tracks().flat_map(|t| t.clips.iter()).collect();
        assert_eq!(all.len(), 4);
        assert!(
            all.iter().all(|c| c.link.is_some() && c.link == all[0].link),
            "all linked"
        );
        let streams: Vec<u32> = s.audio_tracks.iter().map(|t| t.clips[0].stream).collect();
        assert_eq!(streams, [1, 2, 3]);
        assert_eq!(s.length(), Time::seconds(30));
    }

    #[test]
    fn a_second_import_goes_after_the_first_and_keeps_track_names() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 30),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Other", "Mic"], 10),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game", "Mic"], "a used track keeps its name");
        assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(30));
        assert_eq!(s.length(), Time::seconds(40));
        assert_eq!(p.media.len(), 2);
    }

    #[test]
    fn images_get_a_fixed_length_and_files_without_length_are_refused() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let mut image = recording(&[], 0);
        image.kind = MediaKind::Image;
        let q = Command::Import {
            sequence: seq,
            media: image,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(q.active().unwrap().length(), STILL_LENGTH);
        assert!(q.active().unwrap().video_tracks[0].clips[0].link.is_none());

        let err = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 0),
        }
        .apply(&p)
        .unwrap_err();
        assert!(err.what().contains("can't be placed"));
    }

    #[test]
    fn an_automatic_timeline_takes_the_first_videos_settings() {
        let p = Project::new_automatic("Test");
        let seq = p.active_sequence;
        assert!(p.active().unwrap().automatic);

        // Sound alone doesn't settle it.
        let mut voice = recording(&["Voice"], 5);
        voice.kind = MediaKind::Audio;
        voice.info.video = None;
        let p = Command::Import {
            sequence: seq,
            media: voice,
        }
        .apply(&p)
        .unwrap();
        assert!(p.active().unwrap().automatic);

        let mut phone = recording(&["Audio 1"], 10);
        if let Some(v) = phone.info.video.as_mut() {
            (v.width, v.height, v.rotation, v.frame_rate) = (1920, 1080, 90, Rate::NTSC_30);
        }
        phone.info.audio[0].sample_rate = 44_100;
        let p = Command::Import {
            sequence: seq,
            media: phone,
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        assert!(!s.automatic);
        assert_eq!(
            s.settings,
            SequenceSettings {
                width: 1080,
                height: 1920,
                frame_rate: Rate::NTSC_30,
                sample_rate: 44_100
            }
        );

        // Later videos don't change it.
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.active().unwrap().settings.width, 1080);
    }

    #[test]
    fn a_project_with_fixed_settings_keeps_them() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.active().unwrap().settings, SequenceSettings::GAME_CLIP);
    }

    #[test]
    fn settings_can_be_set_by_hand_and_must_make_sense() {
        let p = Project::new_automatic("Test");
        let seq = p.active_sequence;
        let q = Command::SetSettings {
            sequence: seq,
            settings: SequenceSettings::VERTICAL_SHORT,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(q.active().unwrap().settings, SequenceSettings::VERTICAL_SHORT);
        assert!(!q.active().unwrap().automatic);
        let bad = SequenceSettings {
            width: 0,
            ..SequenceSettings::GAME_CLIP
        };
        assert!(
            Command::SetSettings {
                sequence: seq,
                settings: bad
            }
            .apply(&p)
            .is_err()
        );
    }

    fn imported() -> (Project, SequenceId) {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game", "Mic"], 30),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        (p, seq)
    }

    fn first_video(p: &Project, i: usize) -> Clip {
        p.active().unwrap().video_tracks[0].clips[i].clone()
    }

    #[test]
    fn moving_takes_linked_clips_along() {
        let (p, seq) = imported();
        let second = first_video(&p, 1).id;
        let q = Command::MoveLinked {
            sequence: seq,
            clip: second,
            by: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(35));
        assert_eq!(
            s.audio_tracks[0].clips[1].start,
            Time::seconds(35),
            "its sound came along"
        );
        let back = Command::MoveLinked {
            sequence: seq,
            clip: second,
            by: Time::seconds(-5),
        }
        .apply(&p);
        assert!(
            back.unwrap_err().what().contains("already a clip"),
            "it would overlap the first"
        );
        let first = first_video(&p, 0).id;
        assert!(
            Command::MoveLinked {
                sequence: seq,
                clip: first,
                by: Time::seconds(-1)
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn trimming_either_edge_keeps_the_picture_in_place() {
        let (p, seq) = imported();
        let first = first_video(&p, 0).id;
        let q = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(4),
        }
        .apply(&p)
        .unwrap();
        let c = first_video(&q, 0);
        assert_eq!(
            (c.start, c.source_in, c.duration),
            (Time::seconds(4), Time::seconds(4), Time::seconds(26))
        );
        let mic = &q.active().unwrap().audio_tracks[1].clips[0];
        assert_eq!(mic.start, Time::seconds(4), "linked sound trimmed too");
        // Back out again, but not before the file starts.
        let r = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(1),
        }
        .apply(&q)
        .unwrap();
        assert_eq!(first_video(&r, 0).source_in, Time::seconds(1));
        assert!(
            Command::Trim {
                sequence: seq,
                clip: first,
                edge: Edge::Start,
                to: Time::seconds(-1)
            }
            .apply(&q)
            .is_err()
        );

        let e = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::seconds(20),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_video(&e, 0).duration, Time::seconds(20));
        let long = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::seconds(31),
        }
        .apply(&e);
        assert!(
            long.unwrap_err()
                .why_text()
                .unwrap_or_default()
                .contains("ends before")
        );
        let gone = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::ZERO,
        }
        .apply(&p);
        assert!(gone.is_err());
    }

    #[test]
    fn track_mix_is_set_and_limited() {
        let f = fixture();
        let p = Command::SetTrackMix {
            sequence: f.seq,
            track: f.a1,
            muted: true,
            solo: false,
            gain_db: -6.0,
        }
        .apply(&f.p)
        .unwrap();
        let t = &p.active().unwrap().audio_tracks[0];
        assert!(t.muted && !t.solo);
        assert_eq!(t.gain_db, -6.0);
        let loud = Command::SetTrackMix {
            sequence: f.seq,
            track: f.a1,
            muted: false,
            solo: false,
            gain_db: 40.0,
        };
        assert!(loud.apply(&f.p).is_err());
    }

    #[test]
    fn track_and_master_sound_are_set_checked_and_undone_by_snapshot() {
        let f = fixture();
        let mut sound = TrackSound {
            role: crate::TrackRole::Dialogue,
            pan: -250_000,
            ..TrackSound::default()
        };
        sound.compressor.on = true;
        sound.eq.on = true;
        sound.eq.bands[1].gain_mdb = -3_000;
        let set = Command::SetTrackSound {
            sequence: f.seq,
            track: f.a1,
            sound: Box::new(sound.clone()),
        };
        let p = set.apply(&f.p).unwrap();
        assert_eq!(p.active().unwrap().audio_tracks[0].sound, sound);
        assert!(
            f.p.active().unwrap().audio_tracks[0].sound.is_plain(),
            "the old snapshot is untouched"
        );
        sound.eq.bands[1].gain_mdb = -40_000;
        let wild = Command::SetTrackSound {
            sequence: f.seq,
            track: f.a1,
            sound: Box::new(sound),
        };
        assert!(wild.apply(&f.p).is_err());
        let picture = Command::SetTrackSound {
            sequence: f.seq,
            track: f.v1,
            sound: Box::default(),
        };
        assert!(picture.apply(&f.p).is_err(), "only audio tracks");
        let mut master = MasterSound::default();
        master.limiter.on = true;
        let p = Command::SetMasterSound {
            sequence: f.seq,
            sound: master,
        }
        .apply(&f.p)
        .unwrap();
        assert!(p.active().unwrap().master_sound.limiter.on);
        master.limiter.ceiling_mdb = 3_000;
        let over = Command::SetMasterSound {
            sequence: f.seq,
            sound: master,
        };
        assert!(over.apply(&f.p).is_err());
    }

    #[test]
    fn clip_lines_are_set_on_sound_clips_only_and_checked() {
        let f = fixture();
        let p = place(&f, &f.p, f.a1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let seq = p.active().unwrap();
        let (sound, picture) = (seq.audio_tracks[0].clips[0].id, seq.video_tracks[0].clips[0].id);
        let point = |s: i64, value: i32| crate::track_sound::LinePoint {
            at: Time::seconds(s),
            value,
        };
        let lines = ClipLines {
            volume: [point(1, -6_000), point(3, 0)].into_iter().collect(),
            pan: [point(2, 500_000)].into_iter().collect(),
        };
        let set = |clip, lines: &ClipLines| Command::SetClipLines {
            sequence: f.seq,
            clip,
            lines: Box::new(lines.clone()),
        };
        let after = set(sound, &lines).apply(&p).unwrap();
        assert_eq!(after.active().unwrap().audio_tracks[0].clips[0].lines, lines);
        assert!(
            p.active().unwrap().audio_tracks[0].clips[0].lines.is_empty(),
            "undo by snapshot"
        );
        assert!(
            set(picture, &lines).apply(&p).is_err(),
            "a picture has no sound of its own"
        );
        let mut wild = lines.clone();
        wild.volume.push_back(point(4, 20_000));
        assert!(set(sound, &wild).apply(&p).is_err());
    }

    #[test]
    fn relinking_points_the_media_at_its_new_place() {
        let (p, _) = imported();
        let id = *p.media.keys().next().unwrap();
        let moved = PathBuf::from("/recordings/moved.mkv");
        let after = Command::Relink {
            media: id,
            path: moved.clone(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(after.media[&id].path, moved);
        let unknown = Command::Relink {
            media: MediaId(999_999),
            path: moved,
        };
        assert!(unknown.apply(&p).is_err());
    }

    #[test]
    fn several_files_are_relinked_in_one_step_or_none_are() {
        let (p, _) = imported();
        let p = Command::AddMedia {
            media: MediaRef {
                path: "/old/b.mkv".into(),
                ..p.media.values().next().unwrap().clone()
            },
        }
        .apply(&p)
        .unwrap();
        let ids: Vec<MediaId> = p.media.keys().copied().collect();
        let files: Vec<(MediaId, PathBuf)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, PathBuf::from(format!("/new/{i}.mkv"))))
            .collect();
        let after = Command::RelinkFiles { files: files.clone() }.apply(&p).unwrap();
        for (id, path) in &files {
            assert_eq!(&after.media[id].path, path);
        }
        let mut with_unknown = files;
        with_unknown.push((MediaId(999_999), "/new/x.mkv".into()));
        assert!(Command::RelinkFiles { files: with_unknown }.apply(&p).is_err());
        assert!(Command::RelinkFiles { files: vec![] }.apply(&p).is_err());
    }

    fn video_and_game(p: &Project, i: usize) -> (Clip, Clip) {
        let s = p.active().unwrap();
        (
            s.video_tracks[0].clips[i].clone(),
            s.audio_tracks[0].clips[i].clone(),
        )
    }

    #[test]
    fn ripple_trim_moves_everything_after() {
        let (p, seq) = imported();
        let first = first_video(&p, 0).id;
        let end = Command::RippleTrim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::seconds(25),
        }
        .apply(&p)
        .unwrap();
        let (a, _) = video_and_game(&end, 0);
        let (b, b_game) = video_and_game(&end, 1);
        assert_eq!(a.end(), Time::seconds(25));
        assert_eq!(
            (b.start, b_game.start),
            (Time::seconds(25), Time::seconds(25)),
            "no gap"
        );
        assert_eq!(
            end.active().unwrap().audio_tracks[1].clips[0].end(),
            Time::seconds(25)
        );

        let start = Command::RippleTrim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        let (a, _) = video_and_game(&start, 0);
        assert_eq!(
            (a.start, a.source_in, a.duration),
            (Time::ZERO, Time::seconds(5), Time::seconds(25)),
            "stays at the start, shows from 5 s"
        );
        assert_eq!(video_and_game(&start, 1).0.start, Time::seconds(25));
    }

    /// The first clip (30 s) followed by 30-35 s showing 3-8 s of a 10 s file.
    fn touching() -> (Project, SequenceId, ClipId, ClipId) {
        let (p, seq) = imported();
        let (a, b) = (first_video(&p, 0).id, first_video(&p, 1).id);
        let p = Command::Trim {
            sequence: seq,
            clip: b,
            edge: Edge::End,
            to: Time::seconds(35),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Slip {
            sequence: seq,
            clip: b,
            by: Time::seconds(3),
        }
        .apply(&p)
        .unwrap();
        (p, seq, a, b)
    }

    #[test]
    fn slip_shows_another_part_in_the_same_place() {
        let (p, seq, _, b) = touching();
        let (v, game) = video_and_game(&p, 1);
        assert_eq!(
            (v.start, v.source_in, v.duration),
            (Time::seconds(30), Time::seconds(3), Time::seconds(5))
        );
        assert_eq!(game.source_in, Time::seconds(3), "its sound slipped too");
        let too_far = Command::Slip {
            sequence: seq,
            clip: b,
            by: Time::seconds(3),
        };
        assert!(too_far.apply(&p).is_err(), "the 10 s file ends at 8 + 3 = 11 s");
    }

    #[test]
    fn roll_moves_the_cut_between_two_clips() {
        let (p, seq, a, _) = touching();
        let rolled = Command::Roll {
            sequence: seq,
            clip: a,
            to: Time::seconds(28),
        }
        .apply(&p)
        .unwrap();
        let (va, ga) = video_and_game(&rolled, 0);
        let (vb, gb) = video_and_game(&rolled, 1);
        assert_eq!((va.end(), ga.end()), (Time::seconds(28), Time::seconds(28)));
        assert_eq!(
            (vb.start, vb.source_in, vb.end()),
            (Time::seconds(28), Time::seconds(1), Time::seconds(35))
        );
        assert_eq!(gb.start, Time::seconds(28));
        let too_far = Command::Roll {
            sequence: seq,
            clip: a,
            to: Time::seconds(26),
        };
        assert!(too_far.apply(&p).is_err(), "the second file has only 3 s before");
        let no_cut = Command::Roll {
            sequence: seq,
            clip: first_video(&p, 1).id,
            to: Time::seconds(36),
        };
        assert!(no_cut.apply(&p).is_err(), "nothing after the last clip");
    }

    #[test]
    fn slide_moves_a_clip_between_its_neighbours() {
        let (p, seq, _, b) = touching();
        let slid = Command::Slide {
            sequence: seq,
            clip: b,
            by: Time::seconds(-2),
        }
        .apply(&p)
        .unwrap();
        let (va, _) = video_and_game(&slid, 0);
        let (vb, gb) = video_and_game(&slid, 1);
        assert_eq!(va.end(), Time::seconds(28), "the one before got shorter");
        assert_eq!(
            (vb.start, vb.source_in, vb.duration),
            (Time::seconds(28), Time::seconds(3), Time::seconds(5))
        );
        assert_eq!(gb.start, Time::seconds(28));
    }

    #[test]
    fn placing_a_file_again_adds_it_at_the_end() {
        let (p, seq) = imported();
        let media = first_video(&p, 0).media;
        let q = Command::PlaceMedia { sequence: seq, media }.apply(&p).unwrap();
        let s = q.active().unwrap();
        assert_eq!(q.media.len(), p.media.len(), "the same file, not a copy");
        let last = s.video_tracks[0].clips.last().unwrap();
        assert_eq!((last.start, last.media), (Time::seconds(40), media));
        assert_eq!(
            s.audio_tracks[1].clips.last().unwrap().start,
            Time::seconds(40),
            "with its sound"
        );
        let gone = Command::PlaceMedia {
            sequence: seq,
            media: MediaId(9_999),
        };
        assert!(gone.apply(&p).is_err());
    }

    #[test]
    fn removing_stretches_closes_every_gap() {
        let (p, seq) = imported(); // 0-30 s and 30-40 s
        let ranges = vec![
            Region::between(Time::seconds(35), Time::seconds(37)).unwrap(),
            Region::between(Time::seconds(5), Time::seconds(10)).unwrap(),
            Region::between(Time::seconds(8), Time::seconds(12)).unwrap(),
        ];
        let q = Command::RemoveRanges {
            sequence: seq,
            ranges,
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.length(), Time::seconds(40 - 7 - 2), "5-12 and 35-37 gone");
        let v: Vec<(Time, Time, Time)> = s.video_tracks[0]
            .clips
            .iter()
            .map(|c| (c.start, c.source_in, c.duration))
            .collect();
        assert_eq!(
            v,
            [
                (Time::ZERO, Time::ZERO, Time::seconds(5)),
                (Time::seconds(5), Time::seconds(12), Time::seconds(18)),
                (Time::seconds(23), Time::ZERO, Time::seconds(5)),
                (Time::seconds(28), Time::seconds(7), Time::seconds(3)),
            ]
        );
        assert_eq!(s.audio_tracks[1].clips.len(), 2, "the mic track was cut too");
        assert!(
            Command::RemoveRanges {
                sequence: seq,
                ranges: vec![]
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn labels_are_renamed_and_recoloured() {
        let (p, seq) = imported();
        let p = Command::AddLabel {
            sequence: seq,
            label: TimelineLabel {
                id: LabelId(0),
                name: "Label 1".into(),
                at: Time::seconds(3),
                color: LabelColor::Blue,
            },
        }
        .apply(&p)
        .unwrap();
        let id = p.active().unwrap().labels[0].id;
        let edit = |name: &str| Command::EditLabel {
            sequence: seq,
            label: id,
            name: name.into(),
            color: LabelColor::Red,
        };
        let q = edit("  Clutch  ").apply(&p).unwrap();
        let l = &q.active().unwrap().labels[0];
        assert_eq!((l.name.as_str(), l.color), ("Clutch", LabelColor::Red));
        assert!(edit("   ").apply(&p).is_err());
    }

    #[test]
    fn insert_pushes_later_and_overwrite_covers() {
        let (p, seq) = imported(); // 0-30 s (video, game, mic) and 30-40 s (video, game)
        let short = first_video(&p, 1).media; // the 10 s file
        let ins = Command::InsertMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(10),
        }
        .apply(&p)
        .unwrap();
        let s = ins.active().unwrap();
        assert_eq!(s.length(), Time::seconds(50), "ten seconds longer");
        let starts: Vec<Time> = s.video_tracks[0].clips.iter().map(|c| c.start).collect();
        assert_eq!(
            starts,
            [
                Time::ZERO,
                Time::seconds(10),
                Time::seconds(20),
                Time::seconds(40)
            ]
        );
        assert_eq!(
            s.audio_tracks[1].clips[1].start,
            Time::seconds(20),
            "the mic's second half moved too"
        );

        let over = Command::OverwriteMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(25),
        }
        .apply(&p)
        .unwrap();
        let s = over.active().unwrap();
        assert_eq!(s.length(), Time::seconds(40), "nothing moved");
        let v: Vec<(Time, Time)> = s.video_tracks[0]
            .clips
            .iter()
            .map(|c| (c.start, c.end()))
            .collect();
        assert_eq!(
            v,
            [
                (Time::ZERO, Time::seconds(25)),
                (Time::seconds(25), Time::seconds(35)),
                (Time::seconds(35), Time::seconds(40)),
            ]
        );
        assert_eq!(
            s.audio_tracks[1].clips.last().unwrap().end(),
            Time::seconds(25),
            "the mic under it was cut"
        );
    }

    #[test]
    fn restoring_puts_back_a_whole_project() {
        let (p, _) = imported();
        let earlier = Project::new_automatic("Earlier");
        let q = Command::Restore {
            project: Box::new(earlier.clone()),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(q, earlier);
        let mut broken = earlier;
        broken.sequences.clear();
        assert!(
            Command::Restore {
                project: Box::new(broken)
            }
            .apply(&p)
            .is_err()
        );
    }

    fn faded() -> Sound {
        Sound {
            gain_mdb: -6_000,
            pan: 500_000,
            fade_in: Time::seconds(2),
            fade_out: Time::seconds(3),
        }
    }

    #[test]
    fn sound_is_set_on_sound_clips_in_one_step_and_refused_elsewhere() {
        let (p, seq) = imported(); // 0-30 s (game and mic) and 30-40 s (game)
        let s = p.active().unwrap();
        let (game, mic) = (s.audio_tracks[0].clips[0].id, s.audio_tracks[1].clips[0].id);
        let quiet = Sound {
            gain_mdb: -12_000,
            ..Sound::PLAIN
        };
        let set = Command::SetSound {
            sequence: seq,
            sounds: vec![(game, faded()), (mic, quiet)],
        }
        .apply(&p)
        .unwrap();
        let s = set.active().unwrap();
        assert_eq!(s.audio_tracks[0].clips[0].sound, faded());
        assert_eq!(s.audio_tracks[1].clips[0].sound, quiet, "each its own");
        assert_eq!(s.audio_tracks[0].clips[1].sound, Sound::PLAIN, "only those clips");

        let refused = |sounds: Vec<(ClipId, Sound)>| {
            Command::SetSound {
                sequence: seq,
                sounds,
            }
            .apply(&p)
            .unwrap_err()
            .to_string()
        };
        let picture = first_video(&p, 0).id;
        assert!(refused(vec![(picture, faded())]).contains("no sound of its own"));
        assert!(refused(vec![]).contains("No sound"));
        let loud = Sound {
            gain_mdb: 13_000,
            ..Sound::PLAIN
        };
        assert!(refused(vec![(game, loud)]).contains("can't be used"));
        let long = Sound {
            fade_in: Time::seconds(20),
            fade_out: Time::seconds(11),
            ..Sound::PLAIN
        };
        assert!(refused(vec![(game, long)]).contains("too long"));
        // One refused clip refuses the whole step.
        assert!(refused(vec![(mic, quiet), (game, long)]).contains("too long"));
        assert!(refused(vec![(ClipId(999), quiet)]).contains("no longer exists"));
    }

    #[test]
    fn a_split_keeps_the_fade_in_left_and_the_fade_out_right() {
        let (p, seq) = imported();
        let game = p.active().unwrap().audio_tracks[0].clips[0].id;
        let set = Command::SetSound {
            sequence: seq,
            sounds: vec![(game, faded())],
        }
        .apply(&p)
        .unwrap();
        let cut = Command::Split {
            sequence: seq,
            at: Time::seconds(10),
            clip: Some(first_video(&set, 0).id),
        }
        .apply(&set)
        .unwrap();
        let clips = &cut.active().unwrap().audio_tracks[0].clips;
        assert_eq!(clips[0].sound, faded().without_fade_out());
        assert_eq!(clips[1].sound, faded().without_fade_in());
        // Cut close to the start: the fade in is cut to fit the left half.
        let near = Command::Split {
            sequence: seq,
            at: Time::seconds(1),
            clip: Some(game),
        }
        .apply(&set)
        .unwrap();
        let clips = &near.active().unwrap().audio_tracks[0].clips;
        assert_eq!(clips[0].sound.fade_in, Time::seconds(1));
        assert_eq!(clips[1].sound.fade_out, Time::seconds(3));
    }

    #[test]
    fn trims_and_speed_keep_the_fades_within_the_clip() {
        let (p, seq) = imported();
        let game = p.active().unwrap().audio_tracks[0].clips[0].id;
        let set = Command::SetSound {
            sequence: seq,
            sounds: vec![(game, faded())],
        }
        .apply(&p)
        .unwrap();
        let game_clip = |p: &Project| p.active().unwrap().audio_tracks[0].clips[0].clone();
        let trimmed = Command::Trim {
            sequence: seq,
            clip: game,
            edge: Edge::End,
            to: Time::seconds(4),
        }
        .apply(&set)
        .unwrap();
        let c = game_clip(&trimmed);
        assert_eq!(
            (c.sound.fade_in, c.sound.fade_out),
            (Time::seconds(2), Time::seconds(2))
        );
        assert_eq!((c.sound.gain_mdb, c.sound.pan), (-6_000, 500_000));
        let fast = Command::SetSpeed {
            sequence: seq,
            clip: game,
            speed: Speed::new(4, 1).unwrap(),
        }
        .apply(&trimmed)
        .unwrap();
        let c = game_clip(&fast);
        assert_eq!(c.duration, Time::seconds(1));
        assert!(c.sound.fits(c.duration));
        assert_eq!(c.sound.fade_in, Time::seconds(1));
        // Longer again (undo is another snapshot): the fades stay as cut.
        assert!(fast.problems().is_empty());
    }

    #[test]
    fn a_look_is_set_on_the_picture_alone_and_survives_a_split() {
        let (p, seq) = imported(); // 0-30 s and 30-40 s
        let first = first_video(&p, 0).id;
        let look = Look {
            x: 320_000,
            y: -280_000,
            scale: 300_000,
            crop_left: 100_000,
            opacity: 900_000,
            temperature: 150_000,
            ..Look::PLAIN
        };
        let set = Command::SetLook {
            sequence: seq,
            clip: first,
            look,
        }
        .apply(&p)
        .unwrap();
        let s = set.active().unwrap();
        assert_eq!(s.video_tracks[0].clips[0].look, look);
        assert_eq!(s.video_tracks[0].clips[1].look, Look::PLAIN, "only that clip");
        assert!(
            s.audio_tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .all(|c| c.look == Look::PLAIN),
            "its sound has no look"
        );
        // Both halves of a split keep it.
        let cut = Command::Split {
            sequence: seq,
            at: Time::seconds(10),
            clip: Some(first),
        }
        .apply(&set)
        .unwrap();
        let clips = &cut.active().unwrap().video_tracks[0].clips;
        assert_eq!((clips[0].look, clips[1].look), (look, look));

        // Values out of range, sound and unknown clips are refused, explained.
        let refused = |clip, look| {
            Command::SetLook {
                sequence: seq,
                clip,
                look,
            }
            .apply(&p)
            .unwrap_err()
            .to_string()
        };
        let sound = p.active().unwrap().audio_tracks[0].clips[0].id;
        assert!(
            refused(
                first,
                Look {
                    scale: 0,
                    ..Look::PLAIN
                }
            )
            .contains("can't be used")
        );
        assert!(refused(sound, look).contains("no picture"));
        assert!(refused(ClipId(9999), look).contains("clip"));
    }

    /// The first clip (0-30 s of a 30 s file, with two linked sound
    /// streams) easing from full speed into half speed 10 s in, over 2 s.
    fn ramped() -> (Project, SequenceId, ClipId) {
        let (p, seq) = imported();
        let first = first_video(&p, 0).id;
        let ramp = Ramp::new([
            crate::ramp::SpeedKey {
                at: Time::seconds(10),
                speed: Speed::NORMAL,
                ease: crate::Ease::Linear,
            },
            crate::ramp::SpeedKey {
                at: Time::seconds(12),
                speed: Speed::new(1, 2).unwrap(),
                ease: crate::Ease::Linear,
            },
        ]);
        let p = Command::SetRamp {
            sequence: seq,
            clip: first,
            ramp,
        }
        .apply(&p)
        .unwrap();
        (p, seq, first)
    }

    #[test]
    fn a_ramp_keeps_the_same_part_of_the_file_and_its_length_follows() {
        let (p, _, _) = ramped();
        let s = p.active().unwrap();
        let a = &s.video_tracks[0].clips[0];
        // 10 s at full speed, 2 s playing 1.5 s, then 18.5 s of file at
        // half speed (37 s): 49 s, for the same 30 s of the file.
        assert_eq!(a.duration, Time::seconds(49));
        assert_eq!(a.source_len(), Time::seconds(30));
        assert_eq!(a.source_at(Time::seconds(11)), Time::from_ratio(87, 8).unwrap());
        assert_eq!(a.speed_summary(), "Ramp 100→50%");
        assert_eq!(
            s.video_tracks[0].clips[1].start,
            Time::seconds(49),
            "what follows moved along"
        );
        for sound in [&s.audio_tracks[0].clips[0], &s.audio_tracks[1].clips[0]] {
            assert_eq!(
                (sound.duration, &sound.ramp),
                (a.duration, &a.ramp),
                "its sound follows"
            );
        }
        assert!(p.problems().is_empty());
    }

    #[test]
    fn a_ramps_length_is_whole_frames_and_never_past_the_files_end() {
        let (p, seq) = imported();
        let first = first_video(&p, 0).id;
        // A third of normal speed after an eased start: lengths that are
        // no whole number of frames.
        let ramp = Ramp::new([
            crate::ramp::SpeedKey {
                at: Time::ZERO,
                speed: Speed::NORMAL,
                ease: crate::Ease::Smooth,
            },
            crate::ramp::SpeedKey {
                at: Time::from_ratio(7, 10).unwrap(),
                speed: Speed::new(1, 3).unwrap(),
                ease: crate::Ease::Linear,
            },
        ]);
        let q = Command::SetRamp {
            sequence: seq,
            clip: first,
            ramp,
        }
        .apply(&p)
        .unwrap();
        let a = first_video(&q, 0);
        let rate = Rate::FPS_60;
        assert_eq!(rate.snap(a.duration), a.duration);
        assert!(a.source_len() <= Time::seconds(30), "inside the file");
        assert!(Time::seconds(30) - a.source_len() < rate.frame_duration());
    }

    #[test]
    fn a_ramp_keeps_its_place_on_the_picture_through_trims_and_splits() {
        let (p, seq, first) = ramped();
        let before = first_video(&p, 0);
        let at = |c: &Clip, t: i64| c.source_at(Time::seconds(t));
        // Trimmed 5 s at the start: the rest shows what it showed.
        let trimmed = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        let t = first_video(&trimmed, 0);
        for s in [5, 11, 13, 30] {
            assert_eq!(at(&t, s), at(&before, s), "at {s} s");
        }
        assert_eq!(t.ramp.keys()[0].at, Time::seconds(5));
        // Trimmed back out: as before.
        let back = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::ZERO,
        }
        .apply(&trimmed)
        .unwrap();
        assert_eq!(first_video(&back, 0), before);
        // Split in the middle of the ramp: both halves as before.
        let cut = Command::Split {
            sequence: seq,
            at: Time::seconds(11),
            clip: Some(first),
        }
        .apply(&p)
        .unwrap();
        let (left, right) = (first_video(&cut, 0), first_video(&cut, 1));
        assert_eq!(at(&left, 10), at(&before, 10));
        for s in [11, 12, 20, 40] {
            assert_eq!(at(&right, s), at(&before, s), "at {s} s");
        }
        assert_eq!(right.end(), before.end());
        // A plain speed takes the ramp's place; an empty ramp is the speed again.
        let plain = Command::SetSpeed {
            sequence: seq,
            clip: first,
            speed: Speed::NORMAL,
        }
        .apply(&p)
        .unwrap();
        let c = first_video(&plain, 0);
        assert!(c.ramp.is_empty());
        assert_eq!(
            (c.duration, c.source_len()),
            (Time::seconds(30), Time::seconds(30))
        );
        let none = Command::SetRamp {
            sequence: seq,
            clip: first,
            ramp: Ramp::default(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_video(&none, 0).duration, Time::seconds(30));
    }

    #[test]
    fn frame_blending_is_for_pictures() {
        let (p, seq, first) = ramped();
        let blended = Command::SetFrameBlend {
            sequence: seq,
            clip: first,
            blend: FrameBlend::Blend,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_video(&blended, 0).blend, FrameBlend::Blend);
        let sound = p.active().unwrap().audio_tracks[0].clips[0].id;
        let refused = Command::SetFrameBlend {
            sequence: seq,
            clip: sound,
            blend: FrameBlend::Blend,
        }
        .apply(&p);
        assert!(refused.is_err());
        let odd = Command::SetRamp {
            sequence: seq,
            clip: first,
            ramp: Ramp::new([crate::ramp::SpeedKey {
                at: Time::ZERO,
                speed: Speed::new(10_001, 10_000).unwrap(),
                ease: crate::Ease::Linear,
            }]),
        };
        assert!(odd.apply(&p).is_err(), "speeds a ramp can't add up are refused");
    }

    #[test]
    fn speed_keeps_the_same_part_of_the_file_and_moves_the_rest() {
        let (p, seq) = imported(); // 0-30 s and 30-40 s
        let first = first_video(&p, 0).id;
        let half = Speed::new(1, 2).unwrap();
        let slow = Command::SetSpeed {
            sequence: seq,
            clip: first,
            speed: half,
        }
        .apply(&p)
        .unwrap();
        let s = slow.active().unwrap();
        let a = &s.video_tracks[0].clips[0];
        assert_eq!(
            (a.duration, a.source_len(), a.speed),
            (Time::seconds(60), Time::seconds(30), half)
        );
        assert_eq!(
            s.video_tracks[0].clips[1].start,
            Time::seconds(60),
            "the next clip moved along"
        );
        assert_eq!(
            s.audio_tracks[1].clips[0].duration,
            Time::seconds(60),
            "its sound slowed too"
        );
        assert_eq!(a.source_at(Time::seconds(10)), Time::seconds(5));

        // Splitting a slowed clip keeps each half on the right part of the file.
        let cut = Command::Split {
            sequence: seq,
            at: Time::seconds(20),
            clip: Some(first),
        }
        .apply(&slow)
        .unwrap();
        let right = &cut.active().unwrap().video_tracks[0].clips[1];
        assert_eq!(
            (right.start, right.source_in),
            (Time::seconds(20), Time::seconds(10))
        );

        // Twice as fast: 15 s long, and the timeline closes up.
        let fast = Command::SetSpeed {
            sequence: seq,
            clip: first,
            speed: Speed::new(2, 1).unwrap(),
        }
        .apply(&p)
        .unwrap();
        let s = fast.active().unwrap();
        assert_eq!(s.video_tracks[0].clips[0].duration, Time::seconds(15));
        assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(15));
        assert!(Speed::new(0, 1).is_err() && Speed::new(1, 1000).is_err());
        assert_eq!(Speed::new(2, 4).unwrap().percent(), "50%");
    }

    /// The two imported files of [`imported`] and a second, empty video track.
    fn with_two_video_tracks() -> (Project, SequenceId, TrackId) {
        let (p, seq) = imported();
        let p = Command::AddTrack {
            sequence: seq,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let v2 = p.active().unwrap().video_tracks[1].id;
        (p, seq, v2)
    }

    fn spans(track: &Track) -> Vec<(i64, i64)> {
        track
            .clips
            .iter()
            .map(|c| (c.start.as_secs_f64() as i64, c.end().as_secs_f64() as i64))
            .collect()
    }

    #[test]
    fn a_clip_moves_to_another_track_and_its_linked_clips_stay_on_theirs() {
        let (p, seq, v2) = with_two_video_tracks();
        let second = first_video(&p, 1); // 30-40 s, with its game sound on A1
        let up = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v2,
            by: Time::seconds(2),
        }
        .apply(&p)
        .unwrap();
        let s = up.active().unwrap();
        assert_eq!(spans(&s.video_tracks[0]), [(0, 30)], "it left V1");
        assert_eq!(spans(&s.video_tracks[1]), [(32, 42)]);
        assert_eq!(s.video_tracks[1].clips[0].id, second.id, "the same clip");
        assert_eq!(
            spans(&s.audio_tracks[0]),
            [(0, 30), (32, 42)],
            "its sound moved sideways with it, on its own track"
        );
        assert_eq!(
            s.audio_tracks[0].clips[1].link, second.link,
            "and they are still linked"
        );
        // Straight up, without moving sideways.
        let straight = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v2,
            by: Time::ZERO,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(spans(&straight.active().unwrap().video_tracks[1]), [(30, 40)]);
        // And back down again.
        let v1 = p.active().unwrap().video_tracks[0].id;
        let back = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v1,
            by: Time::ZERO,
        }
        .apply(&straight)
        .unwrap();
        assert_eq!(back, p);
        // To the track it is on: a plain move.
        let same = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v1,
            by: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        let plain = Command::MoveLinked {
            sequence: seq,
            clip: second.id,
            by: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(same, plain);
    }

    #[test]
    fn a_clip_keeps_its_look_and_speed_on_another_track_and_under_a_dropped_file() {
        let (p, seq, v2) = with_two_video_tracks(); // 0-30 s and 30-40 s on V1
        let (first, second) = (first_video(&p, 0), first_video(&p, 1));
        let look = Look {
            x: -250_000,
            scale: 400_000,
            crop_top: 50_000,
            opacity: 800_000,
            tint: -100_000,
            ..Look::PLAIN
        };
        let mut p = p;
        for clip in [first.id, second.id] {
            p = Command::SetLook {
                sequence: seq,
                clip,
                look,
            }
            .apply(&p)
            .unwrap();
        }
        let half = Speed::new(1, 2).unwrap();
        let p = Command::SetSpeed {
            sequence: seq,
            clip: second.id,
            speed: half,
        }
        .apply(&p)
        .unwrap();
        // The second clip, now 30-50 s at half speed, goes up a track.
        let up = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v2,
            by: Time::seconds(3),
        }
        .apply(&p)
        .unwrap();
        let moved = &up.active().unwrap().video_tracks[1].clips[0];
        assert_eq!(
            moved,
            &Clip {
                start: Time::seconds(33),
                ..p.active().unwrap().video_tracks[0].clips[1].clone()
            },
            "nothing but its place changed"
        );
        assert_eq!((moved.look, moved.speed), (look, half));
        // A file dropped on V2 over the first clip: the first clip is cut
        // where its sound is covered, and every piece keeps the look.
        let short = second.media;
        let dropped = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(5),
            track: Some(v2),
            insert: false,
        }
        .apply(&p)
        .unwrap();
        let s = dropped.active().unwrap();
        let pieces: Vec<&Clip> = s.video_tracks[0]
            .clips
            .iter()
            .filter(|c| c.media == first.media)
            .collect();
        assert_eq!(pieces.len(), 3, "0-5, 5-15 and 15-30 s");
        assert!(pieces.iter().all(|c| c.look == look));
        assert_eq!(
            s.video_tracks[1].clips[0].look,
            Look::PLAIN,
            "the new clip starts plain"
        );
    }

    #[test]
    fn a_sound_clip_moves_to_another_audio_track() {
        let (p, seq) = imported();
        let s = p.active().unwrap();
        let (game, mic_track) = (s.audio_tracks[0].clips[1].clone(), s.audio_tracks[1].id);
        let q = Command::MoveToTrack {
            sequence: seq,
            clip: game.id,
            track: mic_track,
            by: Time::ZERO,
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(spans(&s.audio_tracks[0]), [(0, 30)]);
        assert_eq!(spans(&s.audio_tracks[1]), [(0, 30), (30, 40)]);
        assert_eq!(
            spans(&s.video_tracks[0]),
            [(0, 30), (30, 40)],
            "its picture stayed"
        );
    }

    #[test]
    fn moving_to_a_track_is_refused_where_it_does_not_fit_and_nothing_changes() {
        let (p, seq, v2) = with_two_video_tracks();
        let second = first_video(&p, 1);
        let to = |p: &Project, track, by| {
            Command::MoveToTrack {
                sequence: seq,
                clip: second.id,
                track,
                by: Time::seconds(by),
            }
            .apply(p)
        };
        // Its sound would land on the first clip's sound.
        let err = to(&p, v2, -30).unwrap_err();
        assert!(err.what().contains("already a clip"), "{}", err.what());
        assert!(err.report().contains("Game"), "names the track: {}", err.report());
        // A clip is already there on the track it goes to.
        let taken = Command::PlaceClip {
            sequence: seq,
            track: v2,
            clip: Clip {
                start: Time::seconds(35),
                link: None,
                ..second.clone()
            },
        }
        .apply(&p)
        .unwrap();
        let err = to(&taken, v2, 0).unwrap_err();
        assert!(err.report().contains("V2"), "{}", err.report());
        // A video clip on an audio track, a track that is gone, before the start.
        let a1 = p.active().unwrap().audio_tracks[0].id;
        assert!(
            to(&p, a1, 0)
                .unwrap_err()
                .what()
                .contains("can't move to that track")
        );
        assert!(
            to(&p, TrackId(9_999), 0)
                .unwrap_err()
                .what()
                .contains("no longer exists")
        );
        assert!(to(&p, v2, -31).unwrap_err().what().contains("can't move there"));
    }

    #[test]
    fn a_file_dropped_on_a_higher_track_leaves_the_picture_under_it() {
        let (p, seq, v2) = with_two_video_tracks(); // 0-30 s (video, game, mic), 30-40 s (video, game)
        let short = first_video(&p, 1).media; // the 10 s file, with game sound
        let q = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(5),
            track: Some(v2),
            insert: false,
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.length(), Time::seconds(40), "nothing moved");
        assert_eq!(spans(&s.video_tracks[1]), [(5, 15)], "the picture is on V2");
        assert_eq!(
            spans(&s.video_tracks[0]),
            [(0, 5), (5, 15), (15, 30), (30, 40)],
            "V1 keeps its picture under it (cut where its sound was cut)"
        );
        assert_eq!(
            spans(&s.audio_tracks[0]),
            [(0, 5), (5, 15), (15, 30), (30, 40)],
            "its sound lies over the game sound"
        );
        let (picture, sound) = (&s.video_tracks[1].clips[0], &s.audio_tracks[0].clips[1]);
        assert_eq!((sound.media, sound.link), (short, picture.link));
        assert!(picture.link.is_some());
        assert_eq!(
            spans(&s.audio_tracks[1]),
            [(0, 5), (5, 15), (15, 30)],
            "the mic, which nothing landed on, keeps going"
        );
        // Every piece of the first clip is still as long as the pieces linked to it.
        for v in &s.video_tracks[0].clips {
            for a in s.audio_tracks.iter().flat_map(|t| t.clips.iter()) {
                if a.link == v.link {
                    assert_eq!((a.start, a.duration), (v.start, v.duration));
                }
            }
        }
    }

    #[test]
    fn a_dropped_file_covers_only_the_tracks_it_lands_on() {
        let (p, seq, v2) = with_two_video_tracks();
        let short = first_video(&p, 1).media;
        // Something on V2 over the place where the file is dropped on V1.
        let p = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(40),
            track: Some(v2),
            insert: false,
        }
        .apply(&p)
        .unwrap();
        let q = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(35),
            track: None,
            insert: false,
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(
            spans(&s.video_tracks[0]),
            [(0, 30), (30, 35), (35, 45)],
            "on the first video track, over what was there"
        );
        assert_eq!(
            spans(&s.video_tracks[1]),
            [(40, 45), (45, 50)],
            "V2 keeps its clip (cut where its sound was covered)"
        );
        assert_eq!(spans(&s.audio_tracks[0]), [(0, 30), (30, 35), (35, 45), (45, 50)]);
    }

    #[test]
    fn a_file_inserted_on_a_track_pushes_every_track_later() {
        let (p, seq, v2) = with_two_video_tracks();
        let short = first_video(&p, 1).media;
        let q = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(5),
            track: Some(v2),
            insert: true,
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.length(), Time::seconds(50), "ten seconds longer");
        assert_eq!(spans(&s.video_tracks[1]), [(5, 15)]);
        assert_eq!(spans(&s.video_tracks[0]), [(0, 5), (15, 40), (40, 50)]);
        assert_eq!(spans(&s.audio_tracks[0]), [(0, 5), (5, 15), (15, 40), (40, 50)]);
        assert_eq!(spans(&s.audio_tracks[1]), [(0, 5), (15, 40)]);
    }

    #[test]
    fn a_file_is_not_dropped_on_a_track_that_cannot_take_its_picture() {
        let (p, seq, _) = with_two_video_tracks();
        let short = first_video(&p, 1).media;
        let on = |track| {
            Command::DropMedia {
                sequence: seq,
                media: short,
                at: Time::seconds(5),
                track: Some(track),
                insert: false,
            }
            .apply(&p)
        };
        let a1 = p.active().unwrap().audio_tracks[0].id;
        let err = on(a1).unwrap_err();
        assert!(err.what().contains("picture can't go"), "{}", err.what());
        assert!(
            on(TrackId(9_999))
                .unwrap_err()
                .what()
                .contains("no longer exists")
        );
        let early = Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(-1),
            track: None,
            insert: true,
        }
        .apply(&p);
        assert!(early.unwrap_err().what().contains("can't be placed there"));
    }
}
