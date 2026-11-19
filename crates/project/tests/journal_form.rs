//! Every command keeps the form it has in the journal.
//!
//! A journal holds commands as they were written when they were done, and
//! recovery replays them, so a command's written form never changes: one
//! sample of every `Command` variant is written and compared, line by
//! line, with `tests/samples/commands.ron`. A new variant gets a line
//! there (the test says which is missing); the lines already there stay.
//! To write the file again after adding a variant:
//!
//! ```sh
//! WRITE_COMMANDS=1 cargo test -p project --test journal_form
//! ```

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used)]

use project::animate::{KeyframeChange, KeyframesChange};
use project::file::{self, FORMAT_VERSION};
use project::templates;
use project::{
    BlendMode, ClipId, ClipLines, Command, CopiedClips, CopiedKeyframes, Ease, Edge, FrameBlend, LabelColor,
    LabelId, LookField, MediaId, MediaLabel, MediaLabelId, MediaListId, MediaUse, Project, Ramp, Region,
    SequenceId, SequenceSettings, Sound, Speed, SpeedKey, Time, TimelineLabel, Title, TitleStyle, TrackId,
    TrackKind, Transition, TransitionKind,
};
use std::path::{Path, PathBuf};

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples")
}

/// The variant's name, by a match that names every one, so a new variant
/// can't be left out of [`every_command`] unnoticed.
fn name(c: &Command) -> &'static str {
    match c {
        Command::RenameProject { .. } => "RenameProject",
        Command::AddMedia { .. } => "AddMedia",
        Command::PlaceMedia { .. } => "PlaceMedia",
        Command::InsertMedia { .. } => "InsertMedia",
        Command::OverwriteMedia { .. } => "OverwriteMedia",
        Command::SetSpeed { .. } => "SetSpeed",
        Command::SetRamp { .. } => "SetRamp",
        Command::SetFrameBlend { .. } => "SetFrameBlend",
        Command::SetLook { .. } => "SetLook",
        Command::SetLookAt { .. } => "SetLookAt",
        Command::Keyframe { .. } => "Keyframe",
        Command::ChangeKeyframes { .. } => "ChangeKeyframes",
        Command::PasteKeyframes { .. } => "PasteKeyframes",
        Command::ResetLook { .. } => "ResetLook",
        Command::SetSound { .. } => "SetSound",
        Command::SetFades { .. } => "SetFades",
        Command::Restore { .. } => "Restore",
        Command::InsertTemplate { .. } => "InsertTemplate",
        Command::RemoveMedia { .. } => "RemoveMedia",
        Command::RemoveMediaAndClips { .. } => "RemoveMediaAndClips",
        Command::Relink { .. } => "Relink",
        Command::RelinkFiles { .. } => "RelinkFiles",
        Command::Import { .. } => "Import",
        Command::ImportSettled { .. } => "ImportSettled",
        Command::DropMedia { .. } => "DropMedia",
        Command::LayMedia { .. } => "LayMedia",
        Command::AddTrack { .. } => "AddTrack",
        Command::RemoveTrack { .. } => "RemoveTrack",
        Command::RenameTrack { .. } => "RenameTrack",
        Command::SetTrackHidden { .. } => "SetTrackHidden",
        Command::PlaceClip { .. } => "PlaceClip",
        Command::RemoveClip { .. } => "RemoveClip",
        Command::RemoveClips { .. } => "RemoveClips",
        Command::MoveClips { .. } => "MoveClips",
        Command::CloseGap { .. } => "CloseGap",
        Command::LiftRange { .. } => "LiftRange",
        Command::RippleRemoveClips { .. } => "RippleRemoveClips",
        Command::MoveClip { .. } => "MoveClip",
        Command::MoveLinked { .. } => "MoveLinked",
        Command::MoveToTrack { .. } => "MoveToTrack",
        Command::MoveBetweenTracks { .. } => "MoveBetweenTracks",
        Command::LineUp { .. } => "LineUp",
        Command::Trim { .. } => "Trim",
        Command::RippleTrim { .. } => "RippleTrim",
        Command::Roll { .. } => "Roll",
        Command::Slip { .. } => "Slip",
        Command::Slide { .. } => "Slide",
        Command::RemoveRanges { .. } => "RemoveRanges",
        Command::RemoveRangesAndRegion { .. } => "RemoveRangesAndRegion",
        Command::Unlink { .. } => "Unlink",
        Command::Link { .. } => "Link",
        Command::UnlinkAll => "UnlinkAll",
        Command::Steps(_) => "Steps",
        Command::Split { .. } => "Split",
        Command::AddLabel { .. } => "AddLabel",
        Command::RemoveLabel { .. } => "RemoveLabel",
        Command::EditLabel { .. } => "EditLabel",
        Command::MoveLabel { .. } => "MoveLabel",
        Command::SetRegion { .. } => "SetRegion",
        Command::SetEditNeedle { .. } => "SetEditNeedle",
        Command::SetMasterGain { .. } => "SetMasterGain",
        Command::SetTrackMix { .. } => "SetTrackMix",
        Command::SetTrackSound { .. } => "SetTrackSound",
        Command::SetClipLines { .. } => "SetClipLines",
        Command::AddEffect { .. } => "AddEffect",
        Command::SetEffects { .. } => "SetEffects",
        Command::SetBlendMode { .. } => "SetBlendMode",
        Command::SetTransition { .. } => "SetTransition",
        Command::SetGrades { .. } => "SetGrades",
        Command::SetMasterSound { .. } => "SetMasterSound",
        Command::SetSettings { .. } => "SetSettings",
        Command::AddSequence { .. } => "AddSequence",
        Command::DuplicateSequence { .. } => "DuplicateSequence",
        Command::RenameSequence { .. } => "RenameSequence",
        Command::RemoveSequence { .. } => "RemoveSequence",
        Command::NestClips { .. } => "NestClips",
        Command::PlaceSequence { .. } => "PlaceSequence",
        Command::AddAdjustment { .. } => "AddAdjustment",
        Command::AddTitle { .. } => "AddTitle",
        Command::SetTitle { .. } => "SetTitle",
        Command::PasteClips { .. } => "PasteClips",
        Command::AddMediaLabel { .. } => "AddMediaLabel",
        Command::EditMediaLabel { .. } => "EditMediaLabel",
        Command::RemoveMediaLabel { .. } => "RemoveMediaLabel",
        Command::SetMediaLabel { .. } => "SetMediaLabel",
        Command::SetMediaNote { .. } => "SetMediaNote",
        Command::AddMediaList { .. } => "AddMediaList",
        Command::RenameMediaList { .. } => "RenameMediaList",
        Command::RemoveMediaList { .. } => "RemoveMediaList",
        Command::SetListMember { .. } => "SetListMember",
        Command::SetMediaUses { .. } => "SetMediaUses",
    }
}

/// One of every command, its values taken from the stored sample of the
/// current project format where it holds richer ones than a default.
fn every_command() -> Vec<Command> {
    let p = file::load(&samples().join(format!("project-v{FORMAT_VERSION}.ron"))).expect("the sample opens");
    let template = templates::load(&samples().join("template-v2.ron")).expect("the template opens");
    let seq = p.active().expect("an open timeline");
    let sequence = seq.id;
    let clips = || seq.tracks().flat_map(|t| t.clips.iter());
    let picture = seq.video_tracks[0].clips[0].clone();
    let clip = picture.id;
    let sound_clip = seq.audio_tracks[0].clips[0].clone();
    let rich =
        |keep: &dyn Fn(&project::Clip) -> bool| clips().find(|c| keep(c)).cloned().unwrap_or(picture.clone());
    let keyed = rich(&|c| !c.keyframes.is_empty());
    let effected = rich(&|c| c.effects.applied().next().is_some());
    let graded = rich(&|c| c.grade != Default::default());
    let titled = rich(&|c| c.title.is_some());
    let crossed = rich(&|c| c.transition.is_some());
    let lined = clips()
        .find(|c| c.lines != ClipLines::default())
        .map(|c| c.lines.clone())
        .unwrap_or_default();
    let track = seq.video_tracks[0].id;
    let audio_track = seq.audio_tracks[0].id;
    let track_sound = seq
        .audio_tracks
        .iter()
        .map(|t| t.sound.clone())
        .find(|s| *s != Default::default())
        .unwrap_or_default();
    let media_ref = p.media.values().next().expect("the sample has media").clone();
    let media = media_ref.id;
    let at = Time::seconds(3);
    let by = Time::from_ratio(-1001, 30_000).expect("a time");
    let region = Region {
        start: Time::seconds(1),
        end: Time::seconds(4),
    };
    let half = Speed::new(1, 2).expect("a speed");
    let rate = seq.settings.frame_rate;
    let frames: Vec<Time> = keyed
        .keyframe_frames(&LookField::ALL, rate)
        .into_iter()
        .map(|f| rate.frame_start(f))
        .collect();
    let keyframes = CopiedKeyframes::copy(&keyed, rate, &frames).expect("keyframes copy");
    let copied = CopiedClips::copy(&p, sequence, &[clip]).expect("clips copy");
    let mut small = Project::new("Restored", SequenceSettings::GAME_CLIP);
    small.media.insert(media, media_ref.clone());
    let first_effect = effected
        .effects
        .applied()
        .next()
        .map(|e| e.kind)
        .expect("the sample has an effect");
    let other = SequenceId(9_001);
    let label = MediaLabelId(77);
    let list = MediaListId(78);
    let mut uses = MediaUse {
        mark_in: Some(Time::seconds(2)),
        mark_out: Some(Time::seconds(9)),
        ..Default::default()
    };
    uses.levels.insert(1, -3_000);
    vec![
        Command::RenameProject {
            name: "Renamed".into(),
        },
        Command::AddMedia {
            media: media_ref.clone(),
        },
        Command::PlaceMedia { sequence, media },
        Command::InsertMedia { sequence, media, at },
        Command::OverwriteMedia { sequence, media, at },
        Command::SetSpeed {
            sequence,
            clip,
            speed: half,
        },
        Command::SetRamp {
            sequence,
            clip,
            ramp: Ramp::new([
                SpeedKey {
                    at: Time::ZERO,
                    speed: Speed::NORMAL,
                    ease: Ease::Smooth,
                },
                SpeedKey {
                    at,
                    speed: half,
                    ease: Ease::Linear,
                },
            ]),
        },
        Command::SetFrameBlend {
            sequence,
            clip,
            blend: FrameBlend::Blend,
        },
        Command::SetLook {
            sequence,
            clip,
            look: keyed.look,
        },
        Command::SetLookAt {
            sequence,
            clip,
            at,
            look: keyed.look,
        },
        Command::Keyframe {
            sequence,
            clip,
            at,
            fields: vec![LookField::X, LookField::Y],
            change: KeyframeChange::Ease(Ease::Hold),
        },
        Command::ChangeKeyframes {
            sequence,
            clip,
            frames: frames.clone(),
            change: KeyframesChange::Move(by),
        },
        Command::PasteKeyframes {
            sequence,
            clip,
            at,
            keyframes,
        },
        Command::ResetLook {
            sequence,
            clip,
            fields: vec![LookField::Scale],
        },
        Command::SetSound {
            sequence,
            sounds: vec![
                (sound_clip.id, Sound::default()),
                (sound_clip.id, sound_clip.sound),
            ],
        },
        Command::SetFades {
            sequence,
            fades: vec![(
                clip,
                project::fades::PictureFades {
                    fade_in: Time::seconds(1),
                    fade_out: Time::ZERO,
                    fade_in_curve: 250_000,
                    fade_out_curve: 0,
                },
            )],
        },
        Command::Restore {
            project: Box::new(small),
        },
        Command::InsertTemplate {
            sequence,
            template: Box::new(template),
            at,
            name_tracks: true,
        },
        Command::RemoveMedia { media: vec![media] },
        Command::RemoveMediaAndClips { media: vec![media] },
        Command::Relink {
            media,
            path: PathBuf::from("/recordings/again.mp4"),
        },
        Command::RelinkFiles {
            files: vec![(media, PathBuf::from("D:\\found\\clip.mkv"))],
        },
        Command::Import {
            sequence,
            media: media_ref.clone(),
        },
        Command::ImportSettled {
            sequence,
            media: media_ref.clone(),
            settings: SequenceSettings::VERTICAL_SHORT,
        },
        Command::DropMedia {
            sequence,
            media,
            at,
            track: Some(track),
            insert: true,
        },
        Command::LayMedia {
            sequence,
            media,
            at,
            track: None,
        },
        Command::AddTrack {
            sequence,
            kind: TrackKind::Audio,
            name: "Mic".into(),
        },
        Command::RemoveTrack { sequence, track },
        Command::RenameTrack {
            sequence,
            track,
            name: "Game".into(),
        },
        Command::SetTrackHidden {
            sequence,
            track,
            hidden: true,
        },
        Command::PlaceClip {
            sequence,
            track,
            clip: picture.clone(),
        },
        Command::RemoveClip {
            sequence,
            clip,
            ripple: true,
        },
        Command::RemoveClips {
            sequence,
            clips: vec![clip, sound_clip.id],
            ripple: false,
        },
        Command::MoveClips {
            sequence,
            clips: vec![clip],
            by,
        },
        Command::CloseGap {
            sequence,
            start: region.start,
            end: region.end,
        },
        Command::LiftRange {
            sequence,
            range: region,
        },
        Command::RippleRemoveClips {
            sequence,
            clips: vec![clip],
        },
        Command::MoveClip {
            sequence,
            clip,
            start: at,
            track: Some(track),
        },
        Command::MoveLinked { sequence, clip, by },
        Command::MoveToTrack {
            sequence,
            clip,
            track,
            by,
        },
        Command::MoveBetweenTracks {
            sequence,
            clip,
            track: None,
            by,
        },
        Command::LineUp {
            sequence,
            moves: vec![(clip, by), (sound_clip.id, at)],
        },
        Command::Trim {
            sequence,
            clip,
            edge: Edge::Start,
            to: at,
        },
        Command::RippleTrim {
            sequence,
            clip,
            edge: Edge::End,
            to: at,
        },
        Command::Roll {
            sequence,
            clip,
            to: at,
        },
        Command::Slip { sequence, clip, by },
        Command::Slide { sequence, clip, by },
        Command::RemoveRanges {
            sequence,
            ranges: vec![region],
        },
        Command::RemoveRangesAndRegion {
            sequence,
            ranges: vec![region, region],
        },
        Command::Unlink {
            sequence,
            clips: vec![clip],
        },
        Command::Link {
            sequence,
            clips: vec![clip, sound_clip.id],
        },
        Command::UnlinkAll,
        Command::Steps(vec![Command::UnlinkAll]),
        Command::Split {
            sequence,
            at,
            clip: Some(clip),
        },
        Command::AddLabel {
            sequence,
            label: TimelineLabel {
                id: LabelId(0),
                name: "Clutch".into(),
                at,
                color: LabelColor::Orange,
                until: None,
            },
        },
        Command::RemoveLabel {
            sequence,
            label: LabelId(5),
        },
        Command::EditLabel {
            sequence,
            label: LabelId(5),
            name: "Ace".into(),
            color: LabelColor::Purple,
        },
        Command::MoveLabel {
            sequence,
            label: LabelId(5),
            at,
        },
        Command::SetRegion {
            sequence,
            region: Some(region),
        },
        Command::SetEditNeedle { sequence, at },
        Command::SetMasterGain {
            sequence,
            gain_db: -3.5,
        },
        Command::SetTrackMix {
            sequence,
            track: audio_track,
            muted: true,
            solo: false,
            gain_db: 2.25,
        },
        Command::SetTrackSound {
            sequence,
            track: audio_track,
            sound: Box::new(track_sound),
        },
        Command::SetClipLines {
            sequence,
            clip: sound_clip.id,
            lines: Box::new(lined),
        },
        Command::AddEffect {
            sequence,
            clip,
            kind: first_effect,
        },
        Command::SetEffects {
            sequence,
            clip,
            effects: Box::new(effected.effects.clone()),
        },
        Command::SetBlendMode {
            sequence,
            clip,
            mode: BlendMode::Screen,
        },
        Command::SetTransition {
            sequence,
            clip,
            transition: crossed.transition.or(Some(Transition {
                kind: TransitionKind::Push,
                length: Time::from_ratio(1001, 1500).expect("a time"),
            })),
        },
        Command::SetGrades {
            sequence,
            grades: vec![(clip, graded.grade.clone())],
        },
        Command::SetMasterSound {
            sequence,
            sound: seq.master_sound,
        },
        Command::SetSettings {
            sequence,
            settings: SequenceSettings::HIGH_FRAME_RATE,
        },
        Command::AddSequence {
            name: "B-roll".into(),
            settings: SequenceSettings::GAME_CLIP,
        },
        Command::DuplicateSequence {
            sequence,
            name: "Copy".into(),
        },
        Command::RenameSequence {
            sequence,
            name: "Main".into(),
        },
        Command::RemoveSequence { sequence: other },
        Command::NestClips {
            sequence,
            clips: vec![clip],
            name: "Nest".into(),
        },
        Command::PlaceSequence {
            sequence,
            nested: other,
            at: Some(at),
            track: Some(track),
        },
        Command::AddAdjustment {
            sequence,
            at,
            length: Time::seconds(2),
            track: None,
        },
        Command::AddTitle {
            sequence,
            at,
            length: Time::seconds(5),
            track: Some(track),
            title: Box::new(Title::new(TitleStyle::EndCard)),
        },
        Command::SetTitle {
            sequence,
            clip: titled.id,
            title: titled
                .title
                .clone()
                .unwrap_or_else(|| Box::new(Title::new(TitleStyle::LowerThird))),
        },
        Command::PasteClips {
            sequence,
            at,
            clips: copied,
        },
        Command::AddMediaLabel {
            label: MediaLabel {
                id: label,
                name: "Highlights".into(),
                color: LabelColor::Green,
            },
            media: vec![media],
        },
        Command::EditMediaLabel {
            label,
            name: "Best".into(),
            color: LabelColor::Red,
        },
        Command::RemoveMediaLabel { label },
        Command::SetMediaLabel {
            media: vec![media, MediaId(4)],
            label,
            on: true,
        },
        Command::SetMediaNote {
            media,
            note: "Good \"quote\" here\nand a second line".into(),
        },
        Command::AddMediaList {
            name: "Selects".into(),
            media: vec![media],
        },
        Command::RenameMediaList {
            list,
            name: "Picks".into(),
        },
        Command::RemoveMediaList { list },
        Command::SetListMember {
            list,
            media: vec![media],
            member: false,
        },
        Command::SetMediaUses {
            uses: vec![(media, uses), (MediaId(4), MediaUse::default())],
        },
    ]
    .into_iter()
    .chain(std::iter::once(Command::PlaceClip {
        sequence,
        track: audio_track,
        clip: sound_clip,
    }))
    .chain([ClipId(1), ClipId(2)].map(|clip| Command::RemoveClip {
        sequence,
        clip,
        ripple: false,
    }))
    .chain(std::iter::once(Command::AddTrack {
        sequence,
        kind: TrackKind::Video,
        name: String::new(),
    }))
    .chain(std::iter::once(Command::PlaceSequence {
        sequence,
        nested: other,
        at: None,
        track: Some(TrackId(3)),
    }))
    .collect()
}

#[test]
fn every_command_keeps_its_journal_form() {
    let commands = every_command();
    let written: Vec<String> = commands
        .iter()
        .map(|c| format!("{}: {}", name(c), ron::to_string(c).expect("a command writes")))
        .collect();
    let path = samples().join("commands.ron");
    if std::env::var_os("WRITE_COMMANDS").is_some() {
        std::fs::write(&path, written.join("\n") + "\n").expect("written");
    }
    let stored = std::fs::read_to_string(&path).expect("the stored commands are there");
    let stored: Vec<&str> = stored.lines().collect();
    assert_eq!(stored.len(), written.len(), "one line per sample command");
    for (now, before) in written.iter().zip(&stored) {
        assert_eq!(now, before, "a command's journal form changed");
    }
    // Every line still reads back as the command it was written from.
    for (line, command) in stored.iter().zip(&commands) {
        let (_, text) = line.split_once(": ").expect("a name and a command");
        let read: Command = ron::from_str(text).expect("the stored command reads");
        assert_eq!(&read, command);
    }
}

#[test]
fn every_variant_has_a_sample() {
    let commands = every_command();
    let mut names: Vec<&str> = commands.iter().map(name).collect();
    names.sort_unstable();
    names.dedup();
    // The match in `name` lists every variant; count its arms here.
    let arms = include_str!("journal_form.rs")
        .lines()
        .filter(|l| l.trim_start().starts_with("Command::") && l.contains(" => \""))
        .count();
    assert_eq!(names.len(), arms, "a variant has no sample command");
}
