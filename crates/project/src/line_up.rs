//! Moving clips to where they line up with another, as sync by sound
//! finds: several clips (each with the clips linked to it) in one step.
//!
//! A clip with a picture moves by whole frames, so its picture stays on
//! the frames it had: it lines up to the nearest frame. A clip of sound
//! only (with only sound linked to it) lines up exactly: its edges go to
//! the frames just inside where it would be, and where in its file it
//! starts moves by the little left over. What would start before the
//! timeline is cut off there. Where something is in the way, the rules of
//! a clip dragged to another track apply: a picture is refused, sound
//! takes the first audio track that is free there (one is made when none
//! is), so nothing is cut.

use crate::command::{insert_clip, missing, sequence_mut, tracks_mut, with_linked};
use crate::ids::{ClipId, SequenceId};
use crate::layers::{is_free, settle_sound};
use crate::model::{Clip, Project, TrackKind};
use crate::time::{Rate, Time};
use diag::AppError;

/// Moves each `clip` (with its linked clips) so that it starts at `to`,
/// exactly as far as the frames allow (see the module's notes).
pub(crate) fn line_up(p: &mut Project, sequence: SequenceId, moves: &[(ClipId, Time)]) -> diag::Result<()> {
    if moves.is_empty() {
        return Err(AppError::new("Nothing to line up").why("No clip was given a place to move to."));
    }
    let seq = sequence_mut(p, sequence)?;
    let rate = seq.settings.frame_rate;
    // Every clip that moves, and by how much exactly.
    let mut groups: Vec<(Vec<ClipId>, Time)> = Vec::new();
    for (clip, to) in moves {
        let start = seq
            .tracks()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == *clip)
            .ok_or_else(|| missing("clip"))?
            .start;
        let group = with_linked(seq, *clip)?;
        if groups.iter().any(|(g, _)| g.iter().any(|id| group.contains(id))) {
            return Err(AppError::new("A clip can only be lined up once")
                .why("Two of the clips to move are linked to each other."));
        }
        groups.push((group, *to - start));
    }
    // Take them all out first, so they can swap places among themselves,
    // keeping each one's track and that track's place among its kind.
    let mut taken = Vec::new();
    let tracks = seq
        .video_tracks
        .iter_mut()
        .enumerate()
        .chain(seq.audio_tracks.iter_mut().enumerate());
    for (index, track) in tracks {
        let (ours, rest): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut track.clips)
            .into_iter()
            .partition(|c| groups.iter().any(|(g, _)| g.contains(&c.id)));
        track.clips = rest.into_iter().collect();
        for c in ours {
            taken.push((index, track.id, track.kind, c));
        }
    }
    let mut placed = Vec::new();
    for (group, by) in &groups {
        let has_picture = taken
            .iter()
            .any(|(_, _, kind, c)| *kind == TrackKind::Video && group.contains(&c.id));
        for (index, track, kind, c) in taken.iter().filter(|(.., c)| group.contains(&c.id)) {
            let moved = if has_picture {
                by_frames(c, rate.snap(*by))?
            } else {
                exactly(c, *by, rate)?
            };
            placed.push((*index, *track, *kind, moved));
        }
    }
    // Pictures land where they line up or not at all; sound stays on its
    // track where that is free, and otherwise takes the first audio track
    // that is, as a dragged clip's linked sound does.
    let mut in_the_way = Vec::new();
    for (index, track, kind, c) in placed {
        let own = tracks_mut(seq)
            .find(|t| t.id == track)
            .ok_or_else(|| missing("track"))?;
        if kind == TrackKind::Video {
            let name = own.name.clone();
            insert_clip(own, c).map_err(|_| picture_in_the_way(&name))?;
        } else if is_free(own, c.start, c.end()) {
            insert_clip(own, c)?;
        } else {
            in_the_way.push((index, c));
        }
    }
    for (index, c) in in_the_way {
        let count = sequence_mut(p, sequence)?.audio_tracks.len();
        settle_sound(p, sequence, c, (index + 1..count).chain(0..index))?;
    }
    Ok(())
}

/// `c` moved by `by` (whole frames), cut where it would start before zero.
fn by_frames(c: &Clip, by: Time) -> diag::Result<Clip> {
    let mut moved = Clip {
        start: c.start + by,
        ..c.clone()
    };
    if moved.start.is_negative() {
        moved = cut_before_zero(&moved)?;
    }
    Ok(moved)
}

/// `c` moved by `by` exactly: its edges on the frames just inside, where
/// in its file it starts moved by what is left over.
fn exactly(c: &Clip, by: Time, rate: Rate) -> diag::Result<Clip> {
    let start = c.start + by;
    let end = c.end() + by;
    let first = frame_at_or_after(rate, start);
    let last = rate.frame_start(rate.frame_at(end));
    if last <= first {
        // Shorter than a frame once inside them: to the nearest frame.
        return by_frames(c, rate.snap(by));
    }
    let mut moved = c.start_cut_by(first - start);
    moved.start = first;
    moved.duration = last - first;
    if moved.start.is_negative() {
        moved = cut_before_zero(&moved)?;
    }
    Ok(moved)
}

/// The first frame boundary at or after `t`.
fn frame_at_or_after(rate: Rate, t: Time) -> Time {
    let floor = rate.frame_start(rate.frame_at(t));
    if floor == t {
        floor
    } else {
        rate.frame_start(rate.frame_at(t) + 1)
    }
}

/// `c` (starting before zero) with what is before zero cut off.
fn cut_before_zero(c: &Clip) -> diag::Result<Clip> {
    let before = Time::ZERO - c.start;
    if c.duration <= before {
        return Err(AppError::new("The clip can't line up there")
            .why("It would be entirely before the timeline starts.")
            .fix("Move the clip it lines up with later first, then sync again."));
    }
    let mut cut = c.start_cut_by(before);
    cut.start = Time::ZERO;
    cut.duration = c.duration - before;
    Ok(cut)
}

fn picture_in_the_way(track: &str) -> AppError {
    AppError::new("There is already a clip there")
        .why(format!(
            "Lined up, a picture would overlap another clip on {track}."
        ))
        .fix("Move what is in the way, or put the clip on an empty track, then sync again.")
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::ids::{ClipId, MediaId};
    use crate::model::{
        AudioStream, Fingerprint, MediaInfo, MediaKind, MediaRef, Project, SequenceSettings, VideoStream,
    };
    use crate::time::{Rate, Time};

    fn file(picture: bool, seconds: i64) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: "take.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: "y".into(),
            },
            kind: if picture {
                MediaKind::Video
            } else {
                MediaKind::Audio
            },
            duration: Time::seconds(seconds),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: picture.then_some(VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: vec![AudioStream {
                    index: 1,
                    codec: "aac".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    name: "Sound".into(),
                }],
                stated_length: None,
            },
        }
    }

    /// The game (picture and sound, 0 to 60 s), a friend's recording
    /// (picture and sound, 60 to 90 s) and a mic (sound only, 90 to 150 s).
    fn project() -> (Project, [ClipId; 3]) {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let import = |p: &Project, media| Command::Import { sequence, media }.apply(p).unwrap();
        let p = import(
            &import(&import(&p, file(true, 60)), file(true, 30)),
            file(false, 60),
        );
        let seq = p.active().unwrap();
        let ids = [
            seq.video_tracks[0].clips[0].id,
            seq.video_tracks[0].clips[1].id,
            seq.audio_tracks[0].clips[2].id,
        ];
        // The mic on a track of its own.
        let p = Command::AddTrack {
            sequence,
            kind: crate::model::TrackKind::Audio,
            name: "Mic".into(),
        }
        .apply(&p)
        .unwrap();
        let mic_track = p.active().unwrap().audio_tracks[1].id;
        let p = Command::MoveToTrack {
            sequence,
            clip: ids[2],
            track: mic_track,
            by: Time::ZERO,
        }
        .apply(&p)
        .unwrap();
        (p, ids)
    }

    fn line_up(p: &Project, moves: Vec<(ClipId, Time)>) -> diag::Result<Project> {
        Command::LineUp {
            sequence: p.active_sequence,
            moves,
        }
        .apply(p)
    }

    fn find(p: &Project, id: ClipId) -> crate::model::Clip {
        p.active()
            .unwrap()
            .tracks()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == id)
            .unwrap()
            .clone()
    }

    #[test]
    fn several_clips_line_up_in_one_step_with_their_linked_sound() {
        let (p, [game, friend, mic]) = project();
        // On another track first, so the friend's picture can sit over the game.
        let p = Command::AddTrack {
            sequence: p.active_sequence,
            kind: crate::model::TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let p = Command::AddTrack {
            sequence: p.active_sequence,
            kind: crate::model::TrackKind::Audio,
            name: "A3".into(),
        }
        .apply(&p)
        .unwrap();
        let (v2, a3) = (
            p.active().unwrap().video_tracks[1].id,
            p.active().unwrap().audio_tracks[2].id,
        );
        let friend_sound = p.active().unwrap().audio_tracks[0].clips[1].id;
        let to_track = |p: &Project, clip, track| {
            Command::MoveToTrack {
                sequence: p.active_sequence,
                clip,
                track,
                by: Time::ZERO,
            }
            .apply(p)
            .unwrap()
        };
        let p = to_track(&to_track(&p, friend, v2), friend_sound, a3);
        let rate = Rate::FPS_60;
        // The friend's recording at 12.5 s plus a third of a frame: the
        // picture moves by whole frames, to the nearest.
        let friend_to = Time::from_ratio(12 * 180 + 90 + 1, 180).unwrap();
        // The mic at 7 s and a quarter of a frame: lined up exactly.
        let mic_to = Time::from_ratio(7 * 240 + 1, 240).unwrap();
        let moved = line_up(&p, vec![(friend, friend_to), (mic, mic_to)]).unwrap();

        let f = find(&moved, friend);
        assert_eq!(f.start, rate.snap(friend_to));
        let sound = moved
            .active()
            .unwrap()
            .audio_tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.link == f.link && c.id != friend)
            .unwrap()
            .clone();
        assert_eq!(sound.start, f.start, "its sound came along");
        assert_eq!(find(&moved, game).start, Time::ZERO, "the reference stays");

        let m = find(&moved, mic);
        let before = find(&p, mic);
        // On frames, the file moved by what was left: the sound plays at
        // exactly the time asked for.
        assert_eq!(rate.snap(m.start), m.start);
        assert_eq!(rate.snap(m.end()), m.end());
        assert_eq!(m.start - m.source_in, mic_to - before.source_in);
        assert!(m.start > mic_to && m.start - mic_to < rate.frame_duration());
        assert!(m.end() <= mic_to + before.duration);

        // One step: undone in one.
        let mut history = crate::history::History::new(p.clone());
        history
            .apply(&Command::LineUp {
                sequence: p.active_sequence,
                moves: vec![(friend, friend_to), (mic, mic_to)],
            })
            .unwrap();
        assert!(history.undo());
        assert_eq!(history.current(), &p);
    }

    #[test]
    fn what_would_start_before_the_timeline_is_cut_off() {
        let (p, [_, friend, mic]) = project();
        // The mic started 5 s before the game: its first 5 s go.
        let moved = line_up(&p, vec![(mic, Time::seconds(-5))]).unwrap();
        let m = find(&moved, mic);
        assert_eq!(
            (m.start, m.source_in, m.duration),
            (Time::ZERO, Time::seconds(5), Time::seconds(55))
        );
        // Entirely before: refused.
        assert!(line_up(&p, vec![(friend, Time::seconds(-40))]).is_err());
    }

    #[test]
    fn a_picture_in_the_way_is_refused_and_sound_finds_a_free_track() {
        let (p, [game, friend, mic]) = project();
        // The friend's picture onto the game's on the same track.
        let err = line_up(&p, vec![(friend, Time::seconds(10))]).unwrap_err();
        assert_eq!(err.what(), "There is already a clip there");
        // The mic back on A1 after the game's sound, then lined up over
        // it: it goes to the first audio track free there (the Mic track),
        // and nothing is cut.
        let a1 = p.active().unwrap().audio_tracks[0].id;
        let p1 = Command::MoveToTrack {
            sequence: p.active_sequence,
            clip: mic,
            track: a1,
            by: Time::ZERO,
        }
        .apply(&p)
        .unwrap();
        let moved = line_up(&p1, vec![(mic, Time::seconds(5))]).unwrap();
        let seq = moved.active().unwrap();
        assert!(seq.audio_tracks[1].clips.iter().any(|c| c.id == mic));
        assert_eq!(find(&moved, mic).start, Time::seconds(5));
        assert_eq!(
            seq.audio_tracks[0].clips.len(),
            2,
            "the game's and the friend's sound stay"
        );
        // Clips moving out of each other's way don't count.
        let swapped = line_up(&p, vec![(game, Time::seconds(30)), (friend, Time::ZERO)]).unwrap();
        assert_eq!(find(&swapped, friend).start, Time::ZERO);
        // Linked twice over: refused.
        let sound_of_game = p.active().unwrap().audio_tracks[0].clips[0].id;
        assert!(
            line_up(
                &p,
                vec![(game, Time::seconds(1)), (sound_of_game, Time::seconds(2))]
            )
            .is_err()
        );
        let _ = mic;
    }
}
