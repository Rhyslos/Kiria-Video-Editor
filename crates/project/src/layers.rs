//! Working in layers: laying a file over a track, and moving a clip from
//! one track to another.
//!
//! In both, sound finds a free place by itself. A sound clip that would
//! land on other sound goes to the first audio track where that stretch is
//! free, and a track is made when there is none. So putting one picture
//! over another never cuts or moves the sound that is already there.

use crate::command::{
    insert_clip, missing, picture_track, placeable_length, placed_streams, sequence_mut, sequence_ref,
    track_mut, with_linked, wrong_kind_of_track,
};
use crate::ids::{ClipId, LinkId, MediaId, SequenceId, TrackId};
use crate::look::Look;
use crate::model::{Clip, Project, Track, TrackKind};
use crate::time::{Speed, Time};
use diag::AppError;

/// Whether nothing on `track` lies between `start` and `end`.
pub(crate) fn is_free(track: &Track, start: Time, end: Time) -> bool {
    !track.clips.iter().any(|c| c.start < end && start < c.end())
}

/// Puts a sound clip on the first audio track where its stretch is free,
/// trying the tracks in `order` (their places among the audio tracks), or
/// on a new track under the others when none of them is.
pub(crate) fn settle_sound(
    p: &mut Project,
    sequence: SequenceId,
    clip: Clip,
    order: impl Iterator<Item = usize>,
) -> diag::Result<()> {
    let seq = sequence_ref(p, sequence)?;
    let mut order = order;
    let free = order.find(|index| {
        seq.audio_tracks
            .get(*index)
            .is_some_and(|track| is_free(track, clip.start, clip.end()))
    });
    let index = match free {
        Some(index) => index,
        None => {
            let name = seq.next_track_name(TrackKind::Audio);
            let track = p.new_track(TrackKind::Audio, &name);
            let tracks = &mut sequence_mut(p, sequence)?.audio_tracks;
            tracks.push_back(track);
            tracks.len() - 1
        }
    };
    insert_clip(&mut sequence_mut(p, sequence)?.audio_tracks[index], clip)
}

/// Lays a file of the project over the timeline at `at`, its picture on the
/// video track `track` (the first one when `None`).
///
/// What the picture covers on its own track is replaced; a file with sound
/// of its own also replaces the sound that belongs to what it covers. Its
/// sound streams then take the first audio tracks where the stretch is
/// free, each counting from the track an import would give it. Nothing
/// else on the timeline is cut or moved.
pub(crate) fn lay(
    p: &mut Project,
    sequence: SequenceId,
    id: MediaId,
    at: Time,
    track: Option<TrackId>,
) -> diag::Result<()> {
    if at.is_negative() {
        return Err(
            AppError::new("The file can't be placed there").why("That is before the timeline starts.")
        );
    }
    let media = p.media.get(&id).cloned().ok_or_else(|| missing("media file"))?;
    let length = placeable_length(&media)?;
    let (video, audio) = placed_streams(&media);
    let end = at + length;
    // The track that was pointed at has to be a video track, also for a
    // file without a picture.
    let chosen = picture_track(sequence_ref(p, sequence)?, track)?;
    let link: Option<LinkId> = (usize::from(video.is_some()) + audio.len() > 1).then(|| p.allocate_id());
    let clip = |p: &mut Project, stream: u32| Clip {
        lines: Default::default(),
        effects: Default::default(),
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
        let track = match chosen {
            Some(track) => track,
            None => {
                let track = p.new_track(TrackKind::Video, "V1");
                let id = track.id;
                sequence_mut(p, sequence)?.video_tracks.push_back(track);
                id
            }
        };
        cover(p, sequence, track, at, end, !audio.is_empty())?;
        insert_clip(track_mut(sequence_mut(p, sequence)?, track)?, c)?;
    }
    for (i, stream) in audio.iter().enumerate() {
        let c = clip(p, stream.index);
        // The track an import would give this stream: made when missing,
        // and named after the stream while nobody has used it.
        if sequence_ref(p, sequence)?.audio_tracks.len() <= i {
            let name = crate::tracks::name_for_stream(sequence_ref(p, sequence)?, &stream.name);
            let track = p.new_track(TrackKind::Audio, &name);
            sequence_mut(p, sequence)?.audio_tracks.push_back(track);
        }
        let tracks = &mut sequence_mut(p, sequence)?.audio_tracks;
        let usual = &mut tracks[i];
        if usual.clips.is_empty()
            && usual.name == format!("A{}", i + 1)
            && crate::tracks::titled(&stream.name)
        {
            usual.name = stream.name.clone();
        }
        let count = tracks.len();
        settle_sound(p, sequence, c, i..count)?;
    }
    Ok(())
}

/// Makes room on the video track `track` for a picture from `at` to `end`:
/// the clips it covers are cut at its edges (each with its linked clips, so
/// they stay as long as each other) and what lies under it goes. With
/// `and_their_sound`, the sound linked to those pieces goes too.
fn cover(
    p: &mut Project,
    sequence: SequenceId,
    track: TrackId,
    at: Time,
    end: Time,
    and_their_sound: bool,
) -> diag::Result<()> {
    crate::edits::cut_edges_on(p, sequence, at, end, &[track])?;
    let under = |c: &Clip| at <= c.start && c.end() <= end;
    let seq = sequence_mut(p, sequence)?;
    let covered = track_mut(seq, track)?;
    let links: Vec<LinkId> = covered
        .clips
        .iter()
        .filter(|c| under(c))
        .filter_map(|c| c.link)
        .collect();
    covered.clips.retain(|c| !under(c));
    if and_their_sound {
        for track in seq.audio_tracks.iter_mut() {
            track
                .clips
                .retain(|c| !(under(c) && c.link.is_some_and(|link| links.contains(&link))));
        }
    }
    Ok(())
}

/// Moves `clip` to the track `to` of its kind (a new one after the others
/// when `None`) and, with the clips linked to it, by `by`.
///
/// The clip itself lands where it was put, or the move is refused. Linked
/// pictures stay on their tracks. Linked sound stays on its track where
/// the stretch is free there (where the moved clips are now does not
/// count: they leave it), and otherwise goes to the first audio track
/// where it is, looking under its own track first.
pub(crate) fn move_between(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    to: Option<TrackId>,
    by: Time,
) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let group = with_linked(seq, clip)?;
    // Everything that moves is taken off first, with the track it was on
    // and that track's place among those of its kind.
    let mut lifted: Vec<(usize, TrackId, TrackKind, Clip)> = Vec::new();
    let tracks = seq
        .video_tracks
        .iter_mut()
        .enumerate()
        .chain(seq.audio_tracks.iter_mut().enumerate());
    for (index, track) in tracks {
        let (ours, rest): (Vec<Clip>, Vec<Clip>) =
            track.clips.iter().cloned().partition(|c| group.contains(&c.id));
        if ours.is_empty() {
            continue;
        }
        track.clips = rest.into_iter().collect();
        for mut c in ours {
            c.start = c.start + by;
            if c.start.is_negative() {
                return Err(
                    AppError::new("The clip can't move there").why("That is before the timeline starts.")
                );
            }
            lifted.push((index, track.id, track.kind, c));
        }
    }
    let position = lifted
        .iter()
        .position(|(_, _, _, c)| c.id == clip)
        .ok_or_else(|| missing("clip"))?;
    let (_, _, kind, moved) = lifted.remove(position);
    let target = match to {
        Some(id) => {
            if track_mut(seq, id)?.kind != kind {
                return Err(wrong_kind_of_track());
            }
            id
        }
        None => {
            let name = seq.next_track_name(kind);
            let track = p.new_track(kind, &name);
            let id = track.id;
            let seq = sequence_mut(p, sequence)?;
            match kind {
                TrackKind::Video => seq.video_tracks.push_back(track),
                TrackKind::Audio => seq.audio_tracks.push_back(track),
            }
            id
        }
    };
    let seq = sequence_mut(p, sequence)?;
    insert_clip(track_mut(seq, target)?, moved)?;
    // Linked clips that can stay on their own tracks do; sound that can't
    // looks for a free track afterwards.
    let mut in_the_way = Vec::new();
    for (index, track, kind, c) in lifted {
        let own = track_mut(seq, track)?;
        if kind == TrackKind::Video || is_free(own, c.start, c.end()) {
            insert_clip(own, c)?;
        } else {
            in_the_way.push((index, c));
        }
    }
    for (index, c) in in_the_way {
        let count = sequence_ref(p, sequence)?.audio_tracks.len();
        settle_sound(p, sequence, c, (index + 1..count).chain(0..index))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::ids::{ClipId, MediaId, SequenceId, TrackId};
    use crate::model::{
        AudioStream, Clip, Fingerprint, MediaInfo, MediaKind, MediaRef, Project, SequenceSettings, Track,
        TrackKind, VideoStream,
    };
    use crate::time::{Rate, Time};

    /// A recording with a picture and one sound stream per name.
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

    fn still() -> MediaRef {
        MediaRef {
            kind: MediaKind::Image,
            duration: Time::ZERO,
            ..recording(&[], 0)
        }
    }

    fn music(seconds: i64) -> MediaRef {
        let mut media = recording(&["Music"], seconds);
        media.kind = MediaKind::Audio;
        media.info.video = None;
        media
    }

    fn import(p: &Project, media: MediaRef) -> Project {
        Command::Import {
            sequence: p.active_sequence,
            media,
        }
        .apply(p)
        .unwrap()
    }

    fn add_media(p: &Project, media: MediaRef) -> (Project, MediaId) {
        let p = Command::AddMedia { media }.apply(p).unwrap();
        let id = p.media.keys().copied().max().unwrap();
        (p, id)
    }

    /// A long recording with game and microphone sound from 0 to 30 s and a
    /// short one with game sound from 30 to 40 s, on V1 with their sound on
    /// the tracks Game and Mic; V2 is empty. Returns the short one's file too.
    fn two_layers() -> (Project, SequenceId, TrackId, MediaId) {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = import(&p, recording(&["Game", "Mic"], 30));
        let p = import(&p, recording(&["Game"], 10));
        let p = Command::AddTrack {
            sequence: seq,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let (v2, short) = (s.video_tracks[1].id, s.video_tracks[0].clips[1].media);
        (p, seq, v2, short)
    }

    fn spans(track: &Track) -> Vec<(i64, i64)> {
        track
            .clips
            .iter()
            .map(|c| (c.start.as_secs_f64() as i64, c.end().as_secs_f64() as i64))
            .collect()
    }

    /// Every track with its name and what is on it, video first.
    fn layout(p: &Project) -> Vec<(String, Vec<(i64, i64)>)> {
        p.active()
            .unwrap()
            .tracks()
            .map(|t| (t.name.clone(), spans(t)))
            .collect()
    }

    fn expect(p: &Project, tracks: &[(&str, &[(i64, i64)])]) {
        let wanted: Vec<(String, Vec<(i64, i64)>)> = tracks
            .iter()
            .map(|(name, spans)| ((*name).to_owned(), spans.to_vec()))
            .collect();
        assert_eq!(layout(p), wanted);
    }

    fn lay(p: &Project, media: MediaId, at: i64, track: Option<TrackId>) -> diag::Result<Project> {
        Command::LayMedia {
            sequence: p.active_sequence,
            media,
            at: Time::seconds(at),
            track,
        }
        .apply(p)
    }

    fn move_to(p: &Project, clip: ClipId, track: Option<TrackId>, by: i64) -> diag::Result<Project> {
        Command::MoveBetweenTracks {
            sequence: p.active_sequence,
            clip,
            track,
            by: Time::seconds(by),
        }
        .apply(p)
    }

    fn video_clip(p: &Project, track: usize, i: usize) -> Clip {
        p.active().unwrap().video_tracks[track].clips[i].clone()
    }

    fn audio_clip(p: &Project, track: usize, i: usize) -> Clip {
        p.active().unwrap().audio_tracks[track].clips[i].clone()
    }

    #[test]
    fn a_file_laid_on_a_higher_track_puts_its_sound_on_a_free_track_and_cuts_nothing() {
        let (p, _, v2, short) = two_layers();
        let q = lay(&p, short, 5, Some(v2)).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[(5, 15)]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(5, 15)]),
            ],
        );
        let (picture, sound) = (video_clip(&q, 1, 0), audio_clip(&q, 2, 0));
        assert!(picture.link.is_some() && picture.link == sound.link, "linked");
        assert_eq!((picture.media, sound.media, sound.stream), (short, short, 1));
        // Everything that was there is as it was, ids and links too.
        let (before, after) = (p.active().unwrap(), q.active().unwrap());
        assert_eq!(before.video_tracks[0], after.video_tracks[0]);
        assert_eq!(before.audio_tracks[0], after.audio_tracks[0]);
        assert_eq!(before.audio_tracks[1], after.audio_tracks[1]);

        // The next one on that layer shares the track that was made.
        let q = lay(&q, short, 18, Some(v2)).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[(5, 15), (18, 28)]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(5, 15), (18, 28)]),
            ],
        );
        // Past the end of everything, its sound is back on the first track.
        let q = lay(&q, short, 50, Some(v2)).unwrap();
        assert_eq!(
            spans(&q.active().unwrap().audio_tracks[0]).last(),
            Some(&(50, 60))
        );
        assert_eq!(q.active().unwrap().audio_tracks.len(), 3);
    }

    #[test]
    fn each_sound_stream_takes_the_first_free_track_from_its_own_on() {
        let (p, _, v2, _) = two_layers();
        let (p, long) = add_media(&p, recording(&["Game", "Mic", "Chat"], 20));
        // Over the short clip, 30 to 40 s: Game is taken there, Mic is not.
        let q = lay(&p, long, 32, Some(v2)).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[(32, 52)]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30), (32, 52)]),
                ("A3", &[(32, 52)]),
                ("A4", &[(32, 52)]),
            ],
        );
        let s = q.active().unwrap();
        let streams: Vec<u32> = s
            .audio_tracks
            .iter()
            .skip(1)
            .map(|t| t.clips.last().unwrap().stream)
            .collect();
        assert_eq!(streams, [1, 2, 3], "in the file's order, top to bottom");
    }

    #[test]
    fn a_file_laid_on_a_track_replaces_what_it_covers_there_with_its_sound() {
        let (p, _, _, short) = two_layers();
        // On the first video track, over the long recording.
        let q = lay(&p, short, 5, None).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("Mic", &[(0, 5), (15, 30)]),
            ],
        );
        let (picture, sound) = (video_clip(&q, 0, 1), audio_clip(&q, 0, 1));
        assert_eq!((picture.media, sound.media), (short, short));
        assert_eq!(picture.link, sound.link);
        assert_eq!(q.active().unwrap().length(), Time::seconds(40), "nothing moved");
        // The pieces left of the long recording still go together.
        let s = q.active().unwrap();
        for v in s.video_tracks[0].clips.iter().filter(|c| c.media != short) {
            let linked = s
                .audio_tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|a| a.link == v.link);
            for a in linked {
                assert_eq!((a.start, a.duration), (v.start, v.duration));
            }
        }
    }

    #[test]
    fn a_layer_above_and_sound_of_its_own_are_left_alone() {
        let (p, seq, v2, short) = two_layers();
        // A title on V2 and music under everything, both from 2 to 12 s.
        let p = lay(&p, short, 2, Some(v2)).unwrap();
        let (p, song) = add_media(&p, music(10));
        let p = lay(&p, song, 2, None).unwrap();
        expect(
            &p,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[(2, 12)]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(2, 12)]),
                ("A4", &[(2, 12)]),
            ],
        );
        // The short recording over the long one on V1, 5 to 15 s.
        let q = lay(&p, short, 5, None).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("V2", &[(2, 12)]),
                ("Game", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("Mic", &[(0, 5), (15, 30)]),
                ("A3", &[(2, 12)]),
                ("A4", &[(2, 12)]),
            ],
        );
        // The old command for the same thing cut every track.
        let old = Command::OverwriteMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(spans(&old.active().unwrap().video_tracks[1]), [(2, 5)]);
    }

    #[test]
    fn a_file_without_sound_replaces_the_picture_and_leaves_the_sound_playing() {
        let (p, _, _, _) = two_layers();
        let (p, picture) = add_media(&p, still());
        let q = lay(&p, picture, 10, None).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 10), (10, 15), (15, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 10), (10, 15), (15, 30), (30, 40)]),
                ("Mic", &[(0, 10), (10, 15), (15, 30)]),
            ],
        );
        assert_eq!(video_clip(&q, 0, 1).media, picture);
        assert_eq!(video_clip(&q, 0, 1).link, None, "one clip: nothing to link");
        let (game, mic) = (audio_clip(&q, 0, 1), audio_clip(&q, 1, 1));
        assert!(game.link.is_some() && game.link == mic.link);
    }

    #[test]
    fn sound_alone_is_laid_where_there_is_room_and_cuts_nothing() {
        let (p, _, v2, _) = two_layers();
        let (p, song) = add_media(&p, music(50));
        let q = lay(&p, song, 20, None).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(20, 70)]),
            ],
        );
        assert_eq!(audio_clip(&q, 2, 0).link, None);
        // Dropped over a video track, it is still only sound.
        let q = lay(&p, song, 30, Some(v2)).unwrap();
        assert!(q.active().unwrap().video_tracks[1].clips.is_empty());
        assert_eq!(spans(&q.active().unwrap().audio_tracks[1]), [(0, 30), (30, 80)]);
    }

    #[test]
    fn on_an_empty_timeline_a_laid_file_lands_as_an_import_does() {
        let empty = Project::new("Test", SequenceSettings::GAME_CLIP);
        let (p, file) = add_media(&empty, recording(&["Game", "Mic", "Chat"], 30));
        let q = lay(&p, file, 0, None).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30)]),
                ("Game", &[(0, 30)]),
                ("Mic", &[(0, 30)]),
                ("Chat", &[(0, 30)]),
            ],
        );
        let imported = import(&empty, recording(&["Game", "Mic", "Chat"], 30));
        assert_eq!(layout(&q), layout(&imported));
        let all: Vec<&Clip> = q
            .active()
            .unwrap()
            .tracks()
            .flat_map(|t| t.clips.iter())
            .collect();
        assert!(all.iter().all(|c| c.link.is_some() && c.link == all[0].link));
    }

    #[test]
    fn laying_a_file_is_refused_where_it_cannot_go_and_nothing_changes() {
        let (p, seq, v2, short) = two_layers();
        let a1 = p.active().unwrap().audio_tracks[0].id;
        let err = lay(&p, short, 5, Some(a1)).unwrap_err();
        assert!(err.what().contains("picture can't go"), "{}", err.what());
        let err = lay(&p, short, 5, Some(TrackId(9_999))).unwrap_err();
        assert!(err.what().contains("no longer exists"), "{}", err.what());
        let err = lay(&p, short, -1, Some(v2)).unwrap_err();
        assert!(err.what().contains("can't be placed there"), "{}", err.what());
        let err = lay(&p, MediaId(9_999), 5, None).unwrap_err();
        assert!(err.what().contains("no longer exists"), "{}", err.what());
        let (p, unread) = add_media(&p, recording(&["Game"], 0));
        let err = lay(&p, unread, 5, None).unwrap_err();
        assert!(err.what().contains("can't be placed"), "{}", err.what());

        let mut history = crate::History::new(p.clone());
        let refused = Command::LayMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(-1),
            track: None,
        };
        assert!(history.apply(&refused).is_err());
        assert!(!history.can_undo() && history.current() == &p);
    }

    #[test]
    fn laying_a_file_and_moving_between_tracks_are_one_undo_step_each() {
        let (p, seq, v2, short) = two_layers();
        let mut history = crate::History::new(p.clone());
        history
            .apply(&Command::LayMedia {
                sequence: seq,
                media: short,
                at: Time::seconds(5),
                track: Some(v2),
            })
            .unwrap();
        let laid = history.current().clone();
        assert_eq!(laid.active().unwrap().audio_tracks.len(), 3);
        // The second clip of V1 goes above everything, over the first.
        let second = video_clip(&laid, 0, 1);
        history
            .apply(&Command::MoveBetweenTracks {
                sequence: seq,
                clip: second.id,
                track: None,
                by: Time::seconds(-10),
            })
            .unwrap();
        let moved = history.current().clone();
        assert_eq!(moved.active().unwrap().video_tracks.len(), 3);
        assert!(history.undo());
        assert_eq!(history.current(), &laid, "the track it made is gone again");
        assert!(history.undo());
        assert_eq!(history.current(), &p, "and so is the sound's track");
        assert!(history.redo() && history.redo());
        assert_eq!(history.current(), &moved);
        assert!(moved.problems().is_empty(), "{:?}", moved.problems());
    }

    #[test]
    fn a_picture_moved_over_another_takes_its_sound_to_a_free_track() {
        let (p, seq, v2, _) = two_layers();
        let second = video_clip(&p, 0, 1); // 30 to 40 s, its sound on Game
        // The plain move to a track refuses this: its sound would land on
        // the first recording's.
        let old = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v2,
            by: Time::seconds(-20),
        };
        assert!(old.apply(&p).unwrap_err().what().contains("already a clip"));

        let q = move_to(&p, second.id, Some(v2), -20).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30)]),
                ("V2", &[(10, 20)]),
                ("Game", &[(0, 30)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(10, 20)]),
            ],
        );
        let (picture, sound) = (video_clip(&q, 1, 0), audio_clip(&q, 2, 0));
        assert_eq!(picture.id, second.id, "the same clip");
        assert_eq!(sound.link, second.link, "still linked to its sound");
        assert_eq!(
            p.active().unwrap().audio_tracks[1],
            q.active().unwrap().audio_tracks[1]
        );
        assert_eq!(
            video_clip(&p, 0, 0),
            video_clip(&q, 0, 0),
            "nothing under it was touched"
        );
    }

    #[test]
    fn sound_that_has_room_stays_on_its_track() {
        let (p, seq, v2, _) = two_layers();
        let second = video_clip(&p, 0, 1);
        // Straight up: its sound is where it was.
        let up = move_to(&p, second.id, Some(v2), 0).unwrap();
        let plain = Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track: v2,
            by: Time::ZERO,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(up, plain, "where nothing is in the way, as the plain move");
        // Over the first clip and back: the sound stays on the track it
        // found, because it has room there.
        let over = move_to(&p, second.id, Some(v2), -20).unwrap();
        let v1 = over.active().unwrap().video_tracks[0].id;
        let back = move_to(&over, second.id, Some(v1), 20).unwrap();
        expect(
            &back,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 30)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(30, 40)]),
            ],
        );
    }

    #[test]
    fn sound_in_the_way_looks_under_its_track_first_then_above_and_only_then_makes_one() {
        let (p, seq, v2, short) = two_layers();
        let second = video_clip(&p, 0, 1);
        // Its sound on A3, the last track; other sound later on A3.
        let p = move_to(&p, second.id, Some(v2), -20).unwrap();
        let a3 = p.active().unwrap().audio_tracks[2].id;
        let p = Command::PlaceClip {
            sequence: seq,
            track: a3,
            clip: Clip {
                lines: Default::default(),
                effects: Default::default(),
                id: ClipId(0),
                media: short,
                stream: 1,
                start: Time::seconds(50),
                source_in: Time::ZERO,
                duration: Time::seconds(10),
                link: None,
                speed: Default::default(),
                look: Default::default(),
                sound: Default::default(),
                keyframes: Default::default(),
                ramp: Default::default(),
                blend: Default::default(),
                kind: Default::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let v1 = p.active().unwrap().video_tracks[0].id;
        // To 50 s on V1: A3 is taken there, nothing is under it, Game is free.
        let q = move_to(&p, second.id, Some(v1), 40).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (50, 60)]),
                ("V2", &[]),
                ("Game", &[(0, 30), (50, 60)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(50, 60)]),
            ],
        );
        assert_eq!(audio_clip(&q, 0, 1).link, second.link);
    }

    #[test]
    fn linked_sound_keeps_what_room_it_has_and_only_the_rest_moves() {
        let (p, _, v2, _) = two_layers();
        let first = video_clip(&p, 0, 0); // 0 to 30 s, with Game and Mic
        // Over the second clip (30 to 40 s, Game only): its game sound is
        // in the way, its microphone is not.
        let q = move_to(&p, first.id, Some(v2), 30).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(30, 40)]),
                ("V2", &[(30, 60)]),
                ("Game", &[(30, 40)]),
                ("Mic", &[(30, 60)]),
                ("A3", &[(30, 60)]),
            ],
        );
        let s = q.active().unwrap();
        assert_eq!(s.audio_tracks[1].clips[0].stream, 2, "the microphone stayed");
        assert_eq!(s.audio_tracks[2].clips[0].stream, 1, "the game sound moved");
        assert!(
            s.audio_tracks
                .iter()
                .skip(1)
                .all(|t| t.clips[0].link == first.link)
        );
    }

    #[test]
    fn a_clip_moved_past_the_last_track_gets_a_new_one() {
        let (p, _, _, _) = two_layers();
        let second = video_clip(&p, 0, 1);
        let q = move_to(&p, second.id, None, -25).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30)]),
                ("V2", &[]),
                ("V3", &[(5, 15)]),
                ("Game", &[(0, 30)]),
                ("Mic", &[(0, 30)]),
                ("A3", &[(5, 15)]),
            ],
        );
        let top = &q.active().unwrap().video_tracks[2];
        assert_eq!(
            (top.kind, top.hidden, top.clips[0].id),
            (TrackKind::Video, false, second.id)
        );
        // A sound clip under the last audio track: a new audio track, and
        // the clips linked to it stay.
        let mic = audio_clip(&p, 1, 0);
        let q = move_to(&p, mic.id, None, 0).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[]),
                ("A3", &[(0, 30)]),
            ],
        );
        assert_eq!(audio_clip(&q, 2, 0).id, mic.id);
        assert_eq!(q.active().unwrap().audio_tracks[2].kind, TrackKind::Audio);
    }

    #[test]
    fn a_sound_clip_put_on_its_partners_track_changes_places_with_it() {
        let (p, _, _, _) = two_layers();
        let (game, mic) = (audio_clip(&p, 0, 0), audio_clip(&p, 1, 0));
        let mic_track = p.active().unwrap().audio_tracks[1].id;
        let q = move_to(&p, game.id, Some(mic_track), 0).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 30), (30, 40)]),
                ("V2", &[]),
                ("Game", &[(0, 30), (30, 40)]),
                ("Mic", &[(0, 30)]),
            ],
        );
        assert_eq!(audio_clip(&q, 1, 0).id, game.id);
        assert_eq!(audio_clip(&q, 0, 0).id, mic.id);
    }

    #[test]
    fn moving_between_tracks_is_refused_where_the_clip_does_not_fit_and_nothing_changes() {
        let (p, seq, v2, short) = two_layers();
        let second = video_clip(&p, 0, 1);
        // Something is already there on the track it goes to.
        let taken = lay(&p, short, 12, Some(v2)).unwrap();
        let err = move_to(&taken, second.id, Some(v2), -20).unwrap_err();
        assert!(err.what().contains("already a clip"), "{}", err.what());
        assert!(err.report().contains("V2"), "names the track: {}", err.report());
        // A video clip on an audio track, a track that is gone, before the start.
        let a1 = p.active().unwrap().audio_tracks[0].id;
        let err = move_to(&p, second.id, Some(a1), 0).unwrap_err();
        assert!(err.what().contains("can't move to that track"), "{}", err.what());
        let err = move_to(&p, second.id, Some(TrackId(9_999)), 0).unwrap_err();
        assert!(err.what().contains("no longer exists"), "{}", err.what());
        let err = move_to(&p, second.id, None, -31).unwrap_err();
        assert!(err.what().contains("can't move there"), "{}", err.what());
        let err = move_to(&p, ClipId(9_999), None, 0).unwrap_err();
        assert!(err.what().contains("no longer exists"), "{}", err.what());
        // A sound clip moved sideways takes its picture along, and a
        // picture never changes track by itself.
        let mic = audio_clip(&p, 1, 0);
        let err = move_to(&p, mic.id, None, 5).unwrap_err();
        assert!(err.report().contains("V1"), "{}", err.report());

        let mut history = crate::History::new(p.clone());
        let refused = Command::MoveBetweenTracks {
            sequence: seq,
            clip: mic.id,
            track: None,
            by: Time::seconds(5),
        };
        assert!(history.apply(&refused).is_err());
        assert!(
            !history.can_undo() && history.current() == &p,
            "no track was made"
        );
    }

    #[test]
    fn commands_of_older_journals_read_and_do_what_they_did() {
        let (p, _, v2, short) = two_layers();
        let (seq, second) = (p.active_sequence, video_clip(&p, 0, 1));
        // Lines as the journal of an earlier version holds them.
        let dropped = format!(
            "DropMedia(sequence:{},media:{},at:(5,1),track:Some({}),insert:false)",
            seq.0, short.0, v2.0
        );
        let dropped: Command = ron::from_str(&dropped).unwrap();
        let q = dropped.apply(&p).unwrap();
        expect(
            &q,
            &[
                ("V1", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("V2", &[(5, 15)]),
                ("Game", &[(0, 5), (5, 15), (15, 30), (30, 40)]),
                ("Mic", &[(0, 5), (5, 15), (15, 30)]),
            ],
        );
        let overwritten = format!("OverwriteMedia(sequence:{},media:{},at:(25,1))", seq.0, short.0);
        let overwritten: Command = ron::from_str(&overwritten).unwrap();
        let q = overwritten.apply(&p).unwrap();
        assert_eq!(spans(&q.active().unwrap().audio_tracks[1]), [(0, 25)]);
        let moved = format!(
            "MoveToTrack(sequence:{},clip:{},track:{},by:(-20,1))",
            seq.0, second.id.0, v2.0
        );
        let moved: Command = ron::from_str(&moved).unwrap();
        assert!(moved.apply(&p).is_err(), "still refused, as it was");
        // And what is written now reads back.
        let new = Command::MoveBetweenTracks {
            sequence: seq,
            clip: second.id,
            track: None,
            by: Time::seconds(-20),
        };
        let text = ron::to_string(&new).unwrap();
        assert_eq!(ron::from_str::<Command>(&text).unwrap(), new);
    }
}
