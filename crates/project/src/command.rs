//! Every change to a project is a [`Command`].
//!
//! A command takes the current snapshot and returns a new one, or an
//! explained error and no change. Commands are plain data, so they can be
//! written to the autosave journal and replayed after a crash.

use crate::animate::{KeyframeChange, KeyframesChange};
use crate::clip_edits;
pub use crate::clip_edits::STILL_LENGTH;
use crate::ids::{ClipId, LabelId, LinkId, MediaId, MediaLabelId, MediaListId, SequenceId, TrackId};
use crate::keyframes::LookField;
use crate::library::{MediaLabel, MediaUse};
use crate::look::Look;
use crate::model::{
    Clip, FrameBlend, LabelColor, MediaRef, Project, Region, Sequence, SequenceSettings, TimelineLabel,
    Track, TrackKind,
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
// A command holding a whole clip is the largest by far; commands are made
// a few at a time, by hand, so their size costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
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
    /// With `name_tracks`, a track nobody has used yet (empty, still with its
    /// usual name, "A1") takes the name of the template's track that lands on
    /// it ("Game"); journals written before it existed read it as off, so
    /// they replay as they did.
    InsertTemplate {
        sequence: SequenceId,
        template: Box<Template>,
        at: Time,
        #[serde(default)]
        name_tracks: bool,
    },
    /// Takes files that no timeline uses out of the project (prune), with
    /// their labels, places in lists and notes, as one step. Refused when
    /// any of them is used. Files on disk are never touched.
    RemoveMedia {
        media: Vec<MediaId>,
    },
    /// Takes files out of the project as `RemoveMedia` does, also those a
    /// timeline uses: every clip playing one of them, on every timeline,
    /// goes with it, leaving gaps (the Library's "Remove anyway"). One step;
    /// files on disk are never touched.
    RemoveMediaAndClips {
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
    /// [`Command::Import`] of the first of several files imported at once:
    /// an automatic timeline takes `settings` (worked out from all of them,
    /// [`SequenceSettings::most_common`]) rather than this file's own. A
    /// timeline whose settings are fixed keeps them, as for `Import`.
    ImportSettled {
        sequence: SequenceId,
        media: MediaRef,
        settings: SequenceSettings,
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
    /// on the same tracks move back to close the gap. The app makes
    /// [`Command::RippleRemoveClips`] for that now; this stays for journals
    /// written before.
    RemoveClip {
        sequence: SequenceId,
        clip: ClipId,
        ripple: bool,
    },
    /// Removes several clips and the clips linked to them, as one step: every
    /// clip is checked first, and if any no longer exists nothing changes.
    /// With `ripple`, later clips on the same tracks move back to close the
    /// gaps, as removing them one after another would. The app makes it
    /// without `ripple` only now ([`Command::RippleRemoveClips`] closes gaps);
    /// with it, it stays for journals written before.
    RemoveClips {
        sequence: SequenceId,
        clips: Vec<ClipId>,
        ripple: bool,
    },
    /// Moves several clips and the clips linked to them by `by` (later is
    /// positive) along their own tracks, keeping them in step, as one step.
    /// Refused, with nothing changed, if any would overlap a clip that
    /// stays or start before zero.
    MoveClips {
        sequence: SequenceId,
        clips: Vec<ClipId>,
        by: Time,
    },
    /// Closes the empty stretch `start`..`end` on every track: everything
    /// after it moves back by its length, as "Delete and close the gap"
    /// moves it. Refused, with nothing changed, when a clip on any track is
    /// in the stretch (closing it would cut that clip).
    CloseGap {
        sequence: SequenceId,
        start: Time,
        end: Time,
    },
    /// Empties the stretch `range` on every track, leaving the gap (clips
    /// crossing its edges are cut there; nothing moves).
    LiftRange {
        sequence: SequenceId,
        range: Region,
    },
    /// Removes several clips and the clips linked to them, as one step, and
    /// closes the gaps on every track, as a ripple trim does: each stretch
    /// the removed clips covered (overlapping ones as one) closes, and every
    /// clip after it on any track moves back by its length, so linked sound
    /// streams and layers stay in step. Refused, with nothing changed, when
    /// a clip that stays is in such a stretch (closing it would cut that
    /// clip), or when a clip no longer exists.
    RippleRemoveClips {
        sequence: SequenceId,
        clips: Vec<ClipId>,
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
    /// `RemoveRanges` and then clears the timeline's region, as one step:
    /// removing the region, or keeping only it, takes the region away with
    /// its stretch, and one undo brings back both. (A command of its own so
    /// that `RemoveRanges` in older journals keeps the region as it did.)
    RemoveRangesAndRegion {
        sequence: SequenceId,
        ranges: Vec<Region>,
    },
    /// Unlinks `clips`: each of them and every clip linked to it plays on
    /// its own afterwards (a picture and each of its sound streams apart).
    Unlink {
        sequence: SequenceId,
        clips: Vec<ClipId>,
    },
    /// Links `clips` (two or more) so they are selected, moved, trimmed
    /// and split together; any links they had are replaced.
    Link {
        sequence: SequenceId,
        clips: Vec<ClipId>,
    },
    /// Unlinks every clip of every timeline (Settings' "Always unlink audio
    /// and video", see `Document::set_always_unlinked`).
    UnlinkAll,
    /// Several changes as one undo step, in order: all or none.
    Steps(Vec<Command>),
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
    /// Moves a timeline label to `at`, keeping the labels in time order.
    MoveLabel {
        sequence: SequenceId,
        label: LabelId,
        at: Time,
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
    /// Adds an effect at the end of a picture's effects (format 19), with
    /// its usual settings and the next number free on that clip.
    AddEffect {
        sequence: SequenceId,
        clip: ClipId,
        kind: crate::effects::EffectKind,
    },
    /// A picture's effects, all at once: taking one away, reordering,
    /// switching one off or changing its settings (format 19). Refused if
    /// it would bring an effect the clip doesn't have (`AddEffect` adds).
    SetEffects {
        sequence: SequenceId,
        clip: ClipId,
        effects: Box<crate::effects::Effects>,
    },
    /// How a picture mixes with what is below it (format 23).
    SetBlendMode {
        sequence: SequenceId,
        clip: ClipId,
        mode: crate::model::BlendMode,
    },
    /// How a picture comes in from the clip just before it on its track
    /// (format 25), or a plain cut (`None`). Refused on sound, on a clip
    /// with no clip ending where it starts, or longer than either clip.
    SetTransition {
        sequence: SequenceId,
        clip: ClipId,
        transition: Option<crate::transitions::Transition>,
    },
    /// Pictures' grades (format 28), each clip its own, in one step:
    /// a grade set, pasted onto several clips or put back as it comes.
    SetGrades {
        sequence: SequenceId,
        grades: Vec<(ClipId, crate::grade::Grade)>,
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
    /// Adds a title (format 26) at `at` for `length`, on a video track as
    /// an adjustment layer goes: the first free from `track` up, or a new
    /// one on top.
    AddTitle {
        sequence: SequenceId,
        at: Time,
        length: Time,
        track: Option<TrackId>,
        title: Box<crate::titles::Title>,
    },
    /// What a title says and how it looks (format 26).
    SetTitle {
        sequence: SequenceId,
        clip: ClipId,
        title: Box<crate::titles::Title>,
    },
    /// Pastes copied clips with the earliest at `at` (see
    /// [`crate::clip_clipboard`]): each on the track of the same kind and
    /// number (missing ones made), cutting away what it lands on there;
    /// clips that can't go into this timeline are left out. The command
    /// carries the clips, so the journal replays it without the clipboard.
    PasteClips {
        sequence: SequenceId,
        at: Time,
        clips: crate::clip_clipboard::CopiedClips,
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
                name_tracks,
            } => crate::templates::insert(&mut p, *sequence, template, *at, *name_tracks)?,
            Command::RemoveMedia { media } => crate::prune::remove_media(&mut p, media)?,
            Command::RemoveMediaAndClips { media } => crate::prune::remove_media_and_clips(&mut p, media)?,
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
            Command::Import { sequence, media } => clip_edits::import(&mut p, *sequence, media)?,
            Command::ImportSettled {
                sequence,
                media,
                settings,
            } => clip_edits::import_settled(&mut p, *sequence, media, settings)?,
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
            Command::PlaceMedia { sequence, media } => {
                clip_edits::place_media(&mut p, *sequence, *media, None, None)?
            }
            Command::InsertMedia { sequence, media, at } => {
                clip_edits::insert_media(&mut p, *sequence, *media, *at)?
            }
            Command::OverwriteMedia { sequence, media, at } => {
                clip_edits::overwrite_media(&mut p, *sequence, *media, *at)?
            }
            Command::DropMedia {
                sequence,
                media,
                at,
                track,
                insert,
            } => clip_edits::drop_media(&mut p, *sequence, *media, *at, *track, *insert)?,
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
            } => clip_edits::place_clip(&mut p, *sequence, *track, clip)?,
            Command::RemoveClip {
                sequence,
                clip,
                ripple,
            } => clip_edits::remove_clips(&mut p, *sequence, &[*clip], *ripple)?,
            Command::RemoveClips {
                sequence,
                clips,
                ripple,
            } => clip_edits::remove_clips(&mut p, *sequence, clips, *ripple)?,
            Command::MoveClips { sequence, clips, by } => {
                clip_edits::move_clips(&mut p, *sequence, clips, *by)?
            }
            Command::CloseGap { sequence, start, end } => {
                clip_edits::close_gap_between(&mut p, *sequence, *start, *end)?
            }
            Command::LiftRange { sequence, range } => {
                if range.end <= range.start || range.start.is_negative() {
                    return Err(
                        AppError::new("That stretch can't be removed").why("It ends before it starts.")
                    );
                }
                crate::edits::clear_space(&mut p, *sequence, range.start, range.length())?
            }
            Command::RippleRemoveClips { sequence, clips } => {
                clip_edits::ripple_remove_clips(&mut p, *sequence, clips)?
            }
            Command::MoveClip {
                sequence,
                clip,
                start,
                track,
            } => clip_edits::move_clip(&mut p, *sequence, *clip, *start, *track)?,
            Command::MoveLinked { sequence, clip, by } => {
                clip_edits::move_linked(&mut p, *sequence, *clip, *by, None)?
            }
            Command::MoveToTrack {
                sequence,
                clip,
                track,
                by,
            } => clip_edits::move_linked(&mut p, *sequence, *clip, *by, Some(*track))?,
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
            } => clip_edits::trim(&mut p, *sequence, *clip, *edge, *to)?,
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
            Command::RemoveRangesAndRegion { sequence, ranges } => {
                crate::edits::remove_ranges(&mut p, *sequence, ranges)?;
                sequence_mut(&mut p, *sequence)?.region = None;
            }
            Command::Split { sequence, at, clip } => split(&mut p, *sequence, *at, *clip)?,
            Command::Unlink { sequence, clips } => crate::linking::unlink(&mut p, *sequence, clips)?,
            Command::Link { sequence, clips } => crate::linking::link(&mut p, *sequence, clips)?,
            Command::UnlinkAll => crate::linking::unlink_all(&mut p),
            Command::Steps(steps) => {
                for step in steps {
                    p = step.apply(&p)?;
                }
            }
            Command::AddLabel { sequence, label } => {
                if label.at.is_negative() {
                    return Err(
                        AppError::new("A label can't go there").why("That is before the timeline starts.")
                    );
                }
                if label.until.is_some_and(|until| until <= label.at) {
                    return Err(AppError::new("A label can't cover that")
                        .why("A labelled stretch must end after it starts."));
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
            Command::MoveLabel { sequence, label, at } => {
                if at.is_negative() {
                    return Err(
                        AppError::new("A label can't go there").why("That is before the timeline starts.")
                    );
                }
                let seq = sequence_mut(&mut p, *sequence)?;
                let index = seq
                    .labels
                    .iter()
                    .position(|l| l.id == *label)
                    .ok_or_else(|| missing("label"))?;
                let was = seq.labels.remove(index);
                // A stretch moves whole.
                let moved = TimelineLabel {
                    at: *at,
                    until: was.until.map(|until| *at + (until - was.at)),
                    ..was
                };
                let index = seq
                    .labels
                    .iter()
                    .position(|l| l.at > moved.at)
                    .unwrap_or(seq.labels.len());
                seq.labels.insert(index, moved);
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
                if !Track::GAINS_DB.contains(gain_db) {
                    return Err(AppError::new("That volume is out of range")
                        .why(format!("The mix's volume goes from {}.", Track::gains_text())));
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
                if !gain_db.is_finite() || !Track::GAINS_DB.contains(gain_db) {
                    return Err(AppError::new("That volume can't be set").why(format!(
                        "Track volume goes from {} (the bottom nearly silent).",
                        Track::gains_text()
                    )));
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
            Command::AddEffect { sequence, clip, kind } => {
                let found = picture_for_effects(sequence_mut(&mut p, *sequence)?, *clip)?;
                let (effects, _) = found.effects.with_added(*kind);
                effects.check()?;
                found.effects = effects;
            }
            Command::SetEffects {
                sequence,
                clip,
                effects,
            } => {
                effects.check()?;
                let found = picture_for_effects(sequence_mut(&mut p, *sequence)?, *clip)?;
                if !found.effects.has_every_id_of(effects) {
                    return Err(AppError::new("Those effects can't be set")
                        .why("One of them isn't on the clip.")
                        .fix("Add it from the inspector's Effects section first."));
                }
                found.effects = (**effects).clone();
            }
            Command::SetBlendMode { sequence, clip, mode } => {
                picture_for_effects(sequence_mut(&mut p, *sequence)?, *clip)?.mode = *mode;
            }
            Command::SetTransition {
                sequence,
                clip,
                transition,
            } => crate::transitions::set(sequence_mut(&mut p, *sequence)?, *clip, *transition)?,
            Command::SetGrades { sequence, grades } => {
                let seq = sequence_mut(&mut p, *sequence)?;
                for (clip, grade) in grades {
                    grade.check()?;
                    picture_for_effects(seq, *clip)?.grade = grade.clone();
                }
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
            Command::AddTitle {
                sequence,
                at,
                length,
                track,
                title,
            } => crate::timelines::add_title(&mut p, *sequence, *at, *length, *track, title)?,
            Command::SetTitle {
                sequence,
                clip,
                title,
            } => {
                title.check()?;
                let seq = sequence_mut(&mut p, *sequence)?;
                let found = seq
                    .video_tracks
                    .iter_mut()
                    .flat_map(|track| track.clips.iter_mut())
                    .find(|c| c.id == *clip)
                    .filter(|c| c.is_title())
                    .ok_or_else(|| {
                        AppError::new("That clip isn't a title").why("Only a title has words to change.")
                    })?;
                found.title = Some(title.clone());
            }
            Command::PasteClips { sequence, at, clips } => {
                crate::clip_clipboard::paste(&mut p, *sequence, *at, clips)?
            }
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
    // A track's clips are in start order and apart, so at most one
    // crosses `at`, found by a search.
    let mut cuts: Vec<(ClipId, Option<LinkId>)> = Vec::new();
    for track in sequence_ref(p, sequence)?.tracks() {
        if let Some(clip) = crossing(&track.clips, at).and_then(|i| track.clips.get(i)) {
            let chosen = targets.as_ref().is_none_or(|t| t.contains(&clip.id));
            if chosen {
                cuts.push((clip.id, clip.link));
            }
        }
    }
    if cuts.is_empty() {
        return Err(AppError::new("Nothing to split here")
            .why("No clip crosses the edit needle at this point.")
            .fix("Move the edit needle over a clip, then split."));
    }
    let right_ids = right_ids(p, &cuts);
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        let Some(i) = crossing(&track.clips, at) else {
            continue;
        };
        for (left_id, right_id, right_link) in &right_ids {
            if track.clips.get(i).is_some_and(|c| c.id == *left_id) {
                let (left, right) = halves(&track.clips[i], at, *right_id, *right_link);
                track.clips[i] = left;
                track.clips.insert(i + 1, right);
            }
        }
    }
    Ok(())
}

/// The ids of the parts after the cuts of one split, `cuts` being the clips
/// cut, in track order, with their links: each a new id, and a new link per
/// link cut, so the parts after stay linked together. Given out in this
/// order, which journals replayed rely on.
pub(crate) fn right_ids(
    p: &mut Project,
    cuts: &[(ClipId, Option<LinkId>)],
) -> Vec<(ClipId, ClipId, Option<LinkId>)> {
    let mut new_links: Vec<(LinkId, LinkId)> = Vec::new();
    let mut right_ids = Vec::with_capacity(cuts.len());
    for &(id, link) in cuts {
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
    right_ids
}

/// `clip` cut at `at` (inside it): the part before, which keeps its id,
/// and the part after, `right_id`, linked by `right_link`.
pub(crate) fn halves(clip: &Clip, at: Time, right_id: ClipId, right_link: Option<LinkId>) -> (Clip, Clip) {
    let first = at - clip.start;
    // Volume and pan stay on both halves; the fade in stays at the start,
    // on the left half, and the fade out at the end, on the right (each cut
    // to fit its half after the change). Each half keeps the keyframes of
    // its own part of the file, and one at the cut holding the value there.
    let cut = clip.source_at(at);
    let right = Clip {
        id: right_id,
        start: at,
        source_in: cut,
        duration: clip.duration - first,
        link: right_link,
        sound: clip.sound.without_fade_in(),
        keyframes: clip.keyframes.cut_to(cut, clip.source_at(clip.end())),
        // Both halves keep the whole ramp, in its place.
        ramp: clip.ramp.shifted(Time::ZERO - first),
        // A transition comes in at the start, which stays on the left.
        transition: None,
        ..clip.clone()
    };
    let left = Clip {
        duration: first,
        sound: clip.sound.without_fade_out(),
        keyframes: clip.keyframes.cut_to(clip.source_in, cut),
        ..clip.clone()
    };
    (left, right)
}

/// The clip of `clips` (in start order, apart) that crosses `at`: starts
/// before it and ends after it.
pub(crate) fn crossing(clips: &imbl::Vector<Clip>, at: Time) -> Option<usize> {
    let after = clips.binary_search_by(|c| {
        if c.start < at {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let i = after.unwrap_or_else(|i| i).checked_sub(1)?;
    clips.get(i).filter(|c| at < c.end()).map(|_| i)
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

pub(crate) fn overlap_error(track: &Track) -> AppError {
    AppError::new("There is already a clip there")
        .why(format!("It would overlap another clip on {}.", track.name))
        .fix("Move it to an empty spot or another track.")
}

/// Inserts in start order; refuses an overlap.
pub(crate) fn insert_clip(track: &mut Track, clip: Clip) -> diag::Result<()> {
    if track
        .clips
        .iter()
        .any(|c| c.start < clip.end() && clip.start < c.end())
    {
        return Err(overlap_error(track));
    }
    let index = track
        .clips
        .iter()
        .position(|c| c.start > clip.start)
        .unwrap_or(track.clips.len());
    track.clips.insert(index, clip);
    Ok(())
}

/// The clip `clip` of `seq`, which effects can be set on: a picture (on a
/// video track) that plays a file or a nested timeline.
fn picture_for_effects(seq: &mut Sequence, clip: ClipId) -> diag::Result<&mut Clip> {
    if seq
        .audio_tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == clip))
    {
        return Err(AppError::new("Effects are set on pictures")
            .why("This clip is sound, on an audio track.")
            .fix("Select the picture it belongs to."));
    }
    let found = seq
        .video_tracks
        .iter_mut()
        .flat_map(|track| track.clips.iter_mut())
        .find(|c| c.id == clip)
        .ok_or_else(|| missing("clip"))?;
    if found.is_adjustment() {
        return Err(AppError::new("An adjustment layer has no effects yet")
            .why("Effects on everything below an adjustment layer come later; its colour works today."));
    }
    Ok(found)
}

pub(crate) fn missing(thing: &str) -> AppError {
    AppError::new(format!("That {thing} no longer exists"))
        .why("It may have been removed by an earlier change.")
        .fix("Undo the last change if this was unexpected.")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{AudioStream, Fingerprint, MediaInfo, MediaKind, SequenceSettings, VideoStream};
    use crate::time::Rate;

    pub(crate) struct Fixture {
        pub(crate) p: Project,
        pub(crate) seq: SequenceId,
        pub(crate) v1: TrackId,
        pub(crate) a1: TrackId,
        pub(crate) media: MediaId,
    }

    pub(crate) fn fixture() -> Fixture {
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

    pub(crate) fn clip(f: &Fixture, start: i64, len: i64, link: Option<LinkId>) -> Clip {
        Clip {
            lines: Default::default(),
            effects: Default::default(),
            mode: Default::default(),
            transition: None,
            title: None,
            grade: Default::default(),
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

    pub(crate) fn place(f: &Fixture, p: &Project, track: TrackId, c: Clip) -> diag::Result<Project> {
        Command::PlaceClip {
            sequence: f.seq,
            track,
            clip: c,
        }
        .apply(p)
    }

    pub(crate) fn clips(p: &Project, track: usize, audio: bool) -> Vec<(i64, i64)> {
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
    fn labels_stay_in_time_order() {
        let f = fixture();
        let label = |at| TimelineLabel {
            id: LabelId(0),
            name: "x".into(),
            at: Time::seconds(at),
            color: crate::model::LabelColor::Blue,
            until: None,
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

    #[test]
    fn a_moved_label_keeps_its_name_and_the_labels_their_order() {
        let f = fixture();
        let label = |name: &str, at| TimelineLabel {
            id: LabelId(0),
            name: name.into(),
            at: Time::seconds(at),
            color: crate::model::LabelColor::Green,
            until: None,
        };
        let mut p = f.p.clone();
        for (name, at) in [("a", 10), ("b", 20), ("c", 30)] {
            p = Command::AddLabel {
                sequence: f.seq,
                label: label(name, at),
            }
            .apply(&p)
            .unwrap();
        }
        let first = p.active().unwrap().labels[0].clone();
        let moved = Command::MoveLabel {
            sequence: f.seq,
            label: first.id,
            at: Time::seconds(25),
        }
        .apply(&p)
        .unwrap();
        let seq = moved.active().unwrap();
        let order: Vec<_> = seq.labels.iter().map(|l| (l.name.as_str(), l.at)).collect();
        assert_eq!(
            order,
            vec![
                ("b", Time::seconds(20)),
                ("a", Time::seconds(25)),
                ("c", Time::seconds(30))
            ]
        );
        assert_eq!(seq.labels[1].id, first.id, "the same label, only moved");
        assert_eq!(seq.labels[1].color, first.color);
        // Before the start, or a label no longer there: refused.
        let refused = |label, at| {
            Command::MoveLabel {
                sequence: f.seq,
                label,
                at,
            }
            .apply(&p)
            .is_err()
        };
        assert!(refused(first.id, Time::ZERO - Time::seconds(1)));
        assert!(refused(LabelId(999), Time::seconds(5)));
    }

    #[test]
    fn a_labelled_stretch_moves_whole_and_follows_ripple_edits() {
        let f = fixture();
        let p = Command::AddLabel {
            sequence: f.seq,
            label: TimelineLabel {
                id: LabelId(0),
                name: "Round".into(),
                at: Time::seconds(10),
                color: crate::model::LabelColor::Orange,
                until: Some(Time::seconds(20)),
            },
        }
        .apply(&f.p)
        .unwrap();
        let id = p.active().unwrap().labels[0].id;
        let stretch = |p: &Project| p.active().unwrap().labels[0].stretch();
        // Moved: both ends go along.
        let moved = Command::MoveLabel {
            sequence: f.seq,
            label: id,
            at: Time::seconds(30),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(
            stretch(&moved),
            Region::between(Time::seconds(30), Time::seconds(40))
        );
        // A stretch can't end before it starts.
        let backwards = Command::AddLabel {
            sequence: f.seq,
            label: TimelineLabel {
                id: LabelId(0),
                name: "x".into(),
                at: Time::seconds(5),
                color: crate::model::LabelColor::Blue,
                until: Some(Time::seconds(5)),
            },
        };
        assert!(backwards.apply(&p).is_err());
        // Room opened inside it: it grows. Cut out of it: it shrinks, and a
        // stretch cut out whole becomes a moment.
        let mut label = p.active().unwrap().labels[0].clone();
        label.open_at(Time::seconds(15), Time::seconds(5));
        assert_eq!(
            (label.at, label.until),
            (Time::seconds(10), Some(Time::seconds(25)))
        );
        label.close_up(Region::between(Time::seconds(12), Time::seconds(17)).unwrap());
        assert_eq!(
            (label.at, label.until),
            (Time::seconds(10), Some(Time::seconds(20)))
        );
        label.close_up(Region::between(Time::seconds(5), Time::seconds(30)).unwrap());
        assert_eq!((label.at, label.until), (Time::seconds(5), None));
    }

    #[test]
    fn a_new_label_never_takes_a_name_in_use() {
        let f = fixture();
        let add = |p: &Project, at| {
            let seq = p.active().unwrap();
            Command::AddLabel {
                sequence: f.seq,
                label: TimelineLabel {
                    id: LabelId(0),
                    name: seq.next_label_name(),
                    at: Time::seconds(at),
                    color: crate::model::LabelColor::Blue,
                    until: None,
                },
            }
            .apply(p)
            .unwrap()
        };
        let p = add(&add(&add(&f.p, 1), 2), 3);
        let names = |p: &Project| -> Vec<String> {
            p.active()
                .unwrap()
                .labels
                .iter()
                .map(|l| l.name.clone())
                .collect()
        };
        assert_eq!(names(&p), ["Label 1", "Label 2", "Label 3"]);
        // Removing the second leaves two labels; the next is not a second "Label 3".
        let second = p.active().unwrap().labels[1].id;
        let p = Command::RemoveLabel {
            sequence: f.seq,
            label: second,
        }
        .apply(&p)
        .unwrap();
        let p = add(&p, 4);
        assert_eq!(names(&p), ["Label 1", "Label 3", "Label 4"]);
    }

    pub(crate) fn recording(audio: &[&str], seconds: i64) -> MediaRef {
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

    pub(crate) fn imported() -> (Project, SequenceId) {
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

    pub(crate) fn first_video(p: &Project, i: usize) -> Clip {
        p.active().unwrap().video_tracks[0].clips[i].clone()
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

    /// A volume a command accepts loads again: the commands and load
    /// validation keep one range, so an edit never makes a project that
    /// fails to open. Just past either end, both refuse.
    #[test]
    fn volumes_at_the_ends_of_the_range_load_again() {
        let f = fixture();
        let (low, high) = (*Track::GAINS_DB.start(), *Track::GAINS_DB.end());
        assert_eq!(Track::gains_text(), "-60 dB to +12 dB");
        let mix = |gain_db| Command::SetTrackMix {
            sequence: f.seq,
            track: f.a1,
            muted: false,
            solo: false,
            gain_db,
        };
        let master = |gain_db| Command::SetMasterGain {
            sequence: f.seq,
            gain_db,
        };
        for (track_db, master_db) in [(low, high), (high, low)] {
            let p = mix(track_db).apply(&f.p).unwrap();
            let p = master(master_db).apply(&p).unwrap();
            let text = crate::file::to_string(&p).unwrap();
            let loaded = crate::file::from_str(&text).unwrap();
            let seq = loaded.active().unwrap();
            assert_eq!(seq.audio_tracks[0].gain_db, track_db);
            assert_eq!(seq.master_gain_db, master_db);
        }
        for outside in [low - 0.01, high + 0.01, f32::NAN, f32::INFINITY] {
            let error = mix(outside).apply(&f.p).unwrap_err();
            assert!(error.to_string().contains(&Track::gains_text()), "{error}");
            assert!(master(outside).apply(&f.p).is_err(), "{outside}");
            let mut p = f.p.clone();
            p.sequences[0].audio_tracks[0].gain_db = outside;
            assert!(p.validate().is_err(), "{outside} is refused on load too");
        }
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
    fn grades_are_set_on_pictures_in_one_step() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 4, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 4, 4, None)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 0, 4, None)).unwrap();
        let seq = p.active().unwrap();
        let pictures: Vec<ClipId> = seq.video_tracks[0].clips.iter().map(|c| c.id).collect();
        let sound = seq.audio_tracks[0].clips[0].id;
        let mut warm = crate::grade::Grade::default();
        warm.layer_mut(0).primary.temperature = 300_000;
        let set = |ids: &[ClipId]| Command::SetGrades {
            sequence: f.seq,
            grades: ids.iter().map(|id| (*id, warm.clone())).collect(),
        };
        assert!(set(&[sound]).apply(&p).is_err(), "sound has no grade");
        let p = set(&pictures).apply(&p).unwrap();
        assert!(
            p.active().unwrap().video_tracks[0]
                .clips
                .iter()
                .all(|c| c.grade == warm)
        );
        let mut bad = warm.clone();
        bad.layer_mut(0).primary.pivot = 0;
        let refused = Command::SetGrades {
            sequence: f.seq,
            grades: vec![(pictures[0], bad)],
        };
        assert!(refused.apply(&p).is_err());
    }

    #[test]
    fn a_title_goes_above_the_picture_and_its_words_change() {
        use crate::titles::{Title, TitleStyle};
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let add = |track| Command::AddTitle {
            sequence: f.seq,
            at: Time::seconds(2),
            length: Time::seconds(4),
            track,
            title: Box::new(Title::new(TitleStyle::LowerThird)),
        };
        assert!(add(Some(f.a1)).apply(&p).is_err(), "not on sound");
        let p = add(None).apply(&p).unwrap();
        let seq = p.active().unwrap();
        assert_eq!(seq.video_tracks.len(), 2, "a track made above the picture");
        let title = seq.video_tracks[1].clips[0].clone();
        assert!(title.is_title());
        assert_eq!(p.clip_name(&title), "Title: Name");
        let mut words = Title::new(TitleStyle::Centred);
        words.text = "Round two".into();
        let set = |clip| Command::SetTitle {
            sequence: f.seq,
            clip,
            title: Box::new(words.clone()),
        };
        let picture = seq.video_tracks[0].clips[0].id;
        assert!(set(picture).apply(&p).is_err(), "a picture has no words");
        let p = set(title.id).apply(&p).unwrap();
        let shown = &p.active().unwrap().video_tracks[1].clips[0];
        assert_eq!(shown.title.as_ref().unwrap().text, "Round two");
        // Split, both halves keep the words.
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(4),
            clip: Some(title.id),
        }
        .apply(&p)
        .unwrap();
        let clips = &p.active().unwrap().video_tracks[1].clips;
        assert!(
            clips
                .iter()
                .all(|c| c.title.as_ref().is_some_and(|t| t.text == "Round two"))
        );
    }

    #[test]
    fn a_transition_goes_on_a_picture_with_one_just_before_it() {
        use crate::transitions::{Transition, TransitionKind};
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 4, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 4, 4, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 10, 4, None)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 4, 4, None)).unwrap();
        let seq = p.active().unwrap();
        let ids: Vec<ClipId> = seq.video_tracks[0].clips.iter().map(|c| c.id).collect();
        let sound = seq.audio_tracks[0].clips[0].id;
        let set = |clip, transition| Command::SetTransition {
            sequence: f.seq,
            clip,
            transition,
        };
        let dissolve = Some(Transition::new(TransitionKind::Dissolve));
        assert!(
            set(ids[0], dissolve).apply(&p).is_err(),
            "nothing before the first"
        );
        assert!(set(ids[2], dissolve).apply(&p).is_err(), "a gap before the third");
        assert!(set(sound, dissolve).apply(&p).is_err(), "sound has none");
        let p = set(ids[1], dissolve).apply(&p).unwrap();
        assert_eq!(p.active().unwrap().video_tracks[0].clips[1].transition, dissolve);
        let p = set(ids[1], None).apply(&p).unwrap();
        assert_eq!(p.active().unwrap().video_tracks[0].clips[1].transition, None);
    }

    #[test]
    fn a_split_leaves_the_transition_at_the_start() {
        use crate::transitions::{Transition, TransitionKind};
        let f = fixture();
        let mut second = clip(&f, 4, 4, None);
        second.transition = Some(Transition::new(TransitionKind::Wipe));
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 4, None)).unwrap();
        let p = place(&f, &p, f.v1, second).unwrap();
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(6),
            clip: None,
        }
        .apply(&p)
        .unwrap();
        let clips = &p.active().unwrap().video_tracks[0].clips;
        assert!(clips[1].transition.is_some() && clips[2].transition.is_none());
    }

    #[test]
    fn a_pictures_blend_mode_is_set_and_sound_has_none() {
        let f = fixture();
        let p = place(&f, &f.p, f.a1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let seq = p.active().unwrap();
        let (sound, picture) = (seq.audio_tracks[0].clips[0].id, seq.video_tracks[0].clips[0].id);
        let set = |clip| Command::SetBlendMode {
            sequence: f.seq,
            clip,
            mode: crate::BlendMode::Screen,
        };
        assert!(set(sound).apply(&p).is_err());
        let p = set(picture).apply(&p).unwrap();
        assert_eq!(
            p.active().unwrap().video_tracks[0].clips[0].mode,
            crate::BlendMode::Screen
        );
    }

    #[test]
    fn effects_are_added_changed_and_taken_away_on_pictures_only() {
        use crate::effects::{EffectId, EffectKind, Vignette};
        let f = fixture();
        let p = place(&f, &f.p, f.a1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let seq = p.active().unwrap();
        let (sound, picture) = (seq.audio_tracks[0].clips[0].id, seq.video_tracks[0].clips[0].id);
        let add = |clip| Command::AddEffect {
            sequence: f.seq,
            clip,
            kind: EffectKind::ALL[0],
        };
        assert!(add(sound).apply(&p).is_err(), "sound has no picture effects");
        let p = add(picture).apply(&p).unwrap();
        let p = add(picture).apply(&p).unwrap();
        let effects = |p: &Project| p.active().unwrap().video_tracks[0].clips[0].effects.clone();
        let ids: Vec<EffectId> = effects(&p).iter().map(|e| e.id).collect();
        assert_eq!(ids, [EffectId(1), EffectId(2)]);
        // Reorder, switch the first off and darken the other: one step.
        let mut changed = effects(&p);
        changed.0.reverse();
        changed.0[1].off = true;
        changed.0[0].kind = EffectKind::Vignette(Vignette {
            amount: 900_000,
            ..Vignette::USUAL
        });
        let set = |effects: &crate::effects::Effects| Command::SetEffects {
            sequence: f.seq,
            clip: picture,
            effects: Box::new(effects.clone()),
        };
        let p = set(&changed).apply(&p).unwrap();
        assert_eq!(effects(&p), changed);
        // Taking one away is a change too; bringing a new number is not.
        let fewer = crate::effects::Effects(changed.0[..1].to_vec());
        assert_eq!(effects(&set(&fewer).apply(&p).unwrap()), fewer);
        let (forged, _) = changed.with_added(EffectKind::ALL[0]);
        assert!(set(&forged).apply(&p).is_err());
        let mut far = changed.clone();
        far.0[0].kind = EffectKind::Vignette(Vignette {
            softness: -1,
            ..Vignette::USUAL
        });
        assert!(set(&far).apply(&p).is_err(), "settings are checked");
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

    /// A short recording, then one with game and microphone sound and an
    /// adjustment layer over its picture.
    pub(crate) fn gap_then_streams() -> (Project, SequenceId) {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game", "Mic"], 30),
        }
        .apply(&p)
        .unwrap();
        let p = Command::AddAdjustment {
            sequence: seq,
            at: Time::seconds(15),
            length: Time::seconds(5),
            track: None,
        }
        .apply(&p)
        .unwrap();
        (p, seq)
    }

    #[test]
    fn lifting_a_range_leaves_the_gap() {
        let (p, seq) = gap_then_streams();
        let q = Command::LiftRange {
            sequence: seq,
            range: Region {
                start: Time::seconds(12),
                end: Time::seconds(16),
            },
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        let spans: Vec<(Time, Time)> = s.video_tracks[0]
            .clips
            .iter()
            .map(|c| (c.start, c.end()))
            .collect();
        assert_eq!(
            spans,
            vec![
                (Time::ZERO, Time::seconds(10)),
                (Time::seconds(10), Time::seconds(12)),
                (Time::seconds(16), Time::seconds(40))
            ]
        );
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
    fn removing_stretches_and_the_region_is_one_undo_step() {
        let (p, seq) = imported(); // 0-30 s and 30-40 s
        let region = Region::between(Time::seconds(5), Time::seconds(10)).unwrap();
        let p = Command::SetRegion {
            sequence: seq,
            region: Some(region),
        }
        .apply(&p)
        .unwrap();
        // The older command keeps the region, as journals expect.
        let kept = Command::RemoveRanges {
            sequence: seq,
            ranges: vec![region],
        }
        .apply(&p)
        .unwrap();
        assert_eq!(kept.active().unwrap().region, Some(region));
        let mut history = crate::history::History::new(p.clone());
        history
            .apply(&Command::RemoveRangesAndRegion {
                sequence: seq,
                ranges: vec![region],
            })
            .unwrap();
        let s = history.current().active().unwrap();
        assert_eq!(s.region, None, "the region went with its stretch");
        assert_eq!(s.length(), Time::seconds(35));
        assert_eq!(s.video_tracks, kept.active().unwrap().video_tracks);
        assert!(history.undo());
        assert_eq!(history.current(), &p, "one undo brings back clips and region");
        assert!(!history.undo(), "nothing else to undo");
    }

    /// A timeline of `n` clips of 3 s on V1, each linked to one on A1.
    pub(crate) fn many_linked(n: i64) -> (Fixture, Project) {
        let f = fixture();
        let mut p = f.p.clone();
        for i in 0..n {
            let link = Some(LinkId(1_000 + i as u64));
            let mut v = clip(&f, i * 3, 3, link);
            v.source_in = Time::seconds(i % 30);
            p = place(&f, &p, f.v1, v.clone()).unwrap();
            p = place(&f, &p, f.a1, v).unwrap();
        }
        (f, p)
    }

    #[test]
    fn many_stretches_leave_what_each_clip_keeps_outside_them() {
        let (f, p) = many_linked(200);
        let t = |ms: i64| Time::from_ratio(ms, 1000).unwrap();
        // Inside clips, on their edges, across several, overlapping each
        // other, and running past the end of the timeline.
        let mut ranges: Vec<Region> = (0..150)
            .map(|i| Region {
                start: t(i * 3_900 + 700),
                end: t(i * 3_900 + 700 + 40 * (i % 7) + 10),
            })
            .collect();
        ranges.push(Region {
            start: t(30_000),
            end: t(36_000),
        });
        ranges.push(Region {
            start: t(35_000),
            end: t(40_000),
        });
        ranges.push(Region {
            start: t(590_000),
            end: t(2_000_000),
        });
        let q = Command::RemoveRanges {
            sequence: f.seq,
            ranges: ranges.clone(),
        }
        .apply(&p)
        .unwrap();
        let mut merged: Vec<Region> = ranges.clone();
        merged.sort_by_key(|r| r.start);
        let merged = merged.into_iter().fold(Vec::<Region>::new(), |mut m, r| {
            match m.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => m.push(r),
            }
            m
        });
        let before = p.active().unwrap();
        let after = q.active().unwrap();
        for (old, new) in before.tracks().zip(after.tracks()) {
            // Each clip on its own: its parts outside the stretches, moved
            // back by the stretches before them.
            let mut want = Vec::new();
            for c in &old.clips {
                let mut parts = vec![(c.start, c.end())];
                for r in &merged {
                    parts = parts
                        .into_iter()
                        .flat_map(|(a, b)| [(a, b.min(r.start)), (a.max(r.end), b)])
                        .filter(|(a, b)| a < b)
                        .collect();
                }
                for (a, b) in parts {
                    let back = merged
                        .iter()
                        .filter(|r| r.end <= a)
                        .fold(Time::ZERO, |sum, r| sum + r.length());
                    want.push((a - back, b - a, c.source_at(a)));
                }
            }
            let got: Vec<_> = new
                .clips
                .iter()
                .map(|c| (c.start, c.duration, c.source_in))
                .collect();
            assert_eq!(got, want, "{}", old.name);
        }
        // The parts of linked clips stay linked to each other.
        let (v, a) = (&after.video_tracks[0].clips, &after.audio_tracks[0].clips);
        assert_eq!(v.len(), a.len());
        for (v, a) in v.iter().zip(a.iter()) {
            assert!(v.link.is_some() && v.link == a.link && v.id != a.id);
        }
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
                until: None,
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

    /// A speed keyframe's mark on the timeline works like any other: moved,
    /// eased or removed in one step, the clip's length following.
    #[test]
    fn a_speed_keyframes_mark_moves_eases_and_removes_it() {
        let (p, seq, clip) = ramped();
        let rate = p.active().unwrap().settings.frame_rate;
        let change = |p: &Project, at: Time, change| {
            Command::ChangeKeyframes {
                sequence: seq,
                clip,
                frames: vec![rate.snap(at)],
                change,
            }
            .apply(p)
        };
        let keys = |p: &Project| first_video(p, 0).ramp.keys().to_vec();
        // Moved a second later: the slow-down starts later, the clip is shorter.
        let moved = change(&p, Time::seconds(12), KeyframesChange::Move(Time::seconds(1))).unwrap();
        assert_eq!(keys(&moved)[1].at, Time::seconds(13));
        assert!(first_video(&moved, 0).duration < first_video(&p, 0).duration);
        assert_eq!(
            first_video(&moved, 0).source_len(),
            Time::seconds(30),
            "the same part of the file"
        );
        // Eased.
        let eased = change(&p, Time::seconds(10), KeyframesChange::Ease(crate::Ease::Smooth)).unwrap();
        assert_eq!(keys(&eased)[0].ease, crate::Ease::Smooth);
        // Removed: one speed keyframe left, so one speed (half) all along.
        let removed = change(&p, Time::seconds(10), KeyframesChange::Remove).unwrap();
        assert_eq!(keys(&removed).len(), 1);
        // Onto the other speed keyframe's frame: refused.
        assert!(change(&p, Time::seconds(10), KeyframesChange::Move(Time::seconds(2))).is_err());
        assert!(removed.problems().is_empty());
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

    pub(crate) fn spans(track: &Track) -> Vec<(i64, i64)> {
        track
            .clips
            .iter()
            .map(|c| (c.start.as_secs_f64() as i64, c.end().as_secs_f64() as i64))
            .collect()
    }
}
