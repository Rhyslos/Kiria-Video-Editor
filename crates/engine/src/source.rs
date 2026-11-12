//! A clip's own picture, raw: the file it plays at the moment the playhead
//! is in it, with nothing of its look (place, crop, opacity, colour). What
//! the Source half of a split viewer shows beside the timeline's picture.
//!
//! No new way of decoding is needed for it: [`file_alone`] makes a small
//! project holding just the clip's file, whole, on one track, at the file's
//! own size and rate. A preview worker asked for that project at a moment
//! of the file decodes and shows exactly that file there, with the frame
//! cache, the stepping down between ways of decoding and everything else
//! the viewer's worker does. The time asked for is the time in the file, so
//! the project stays the same while the playhead moves and only changes
//! when another file is shown.

use project::{Clip, ClipId, MediaRef, Project, Rate, Sequence, SequenceSettings, Time, TrackKind};

/// What the Source shows for a selection and a playhead.
#[derive(Debug, Clone, PartialEq)]
pub enum Source<'a> {
    /// Nothing is selected, or nothing selected has a picture.
    NothingSelected,
    /// The clip's file at `at` (time in the file, from its start).
    Shot {
        clip: &'a Clip,
        media: &'a MediaRef,
        at: Time,
    },
}

/// What the Source half shows: the first selected clip whose file has a
/// picture (a selected sound clip stands for the picture clip linked to
/// it), at the playhead's place in it; with the playhead before or after
/// the clip, its first or last frame.
pub fn source<'a>(
    project: &'a Project,
    sequence: &'a Sequence,
    selected: &[ClipId],
    playhead: Time,
) -> Source<'a> {
    let Some(clip) = picture_clip(project, sequence, selected) else {
        return Source::NothingSelected;
    };
    let Some(media) = project.media.get(&clip.media) else {
        return Source::NothingSelected;
    };
    Source::Shot {
        clip,
        media,
        at: clip.source_at(within(clip, playhead, sequence.settings.frame_rate)),
    }
}

/// The selected clip whose picture the Source shows: the first selected
/// one on a video track whose file has a picture, else the picture clip
/// linked to a selected sound clip.
fn picture_clip<'a>(project: &Project, sequence: &'a Sequence, selected: &[ClipId]) -> Option<&'a Clip> {
    let has_picture = |clip: &Clip| {
        project
            .media
            .get(&clip.media)
            .is_some_and(|media| media.info.video.is_some())
    };
    let pictures = || sequence.video_tracks.iter().flat_map(|track| track.clips.iter());
    for id in selected {
        if let Some(clip) = pictures().find(|clip| clip.id == *id && has_picture(clip)) {
            return Some(clip);
        }
        let link = sequence
            .audio_tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.id == *id)
            .and_then(|clip| clip.link);
        if let Some(clip) =
            link.and_then(|link| pictures().find(|clip| clip.link == Some(link) && has_picture(clip)))
        {
            return Some(clip);
        }
    }
    None
}

/// The playhead held inside `clip`: from its first frame to its last.
pub fn within(clip: &Clip, playhead: Time, rate: Rate) -> Time {
    let last = (clip.end() - rate.frame_duration()).max(clip.start);
    playhead.max(clip.start).min(last)
}

/// A project of `media` alone: the whole file on one video track from the
/// start, at its own size and frame rate, looking as it is. Asked for at a
/// time, a preview worker shows the file at that time. `None` when the
/// file has no picture.
pub fn file_alone(media: &MediaRef) -> Option<Project> {
    let video = media.info.video.as_ref()?;
    let (width, height) = video.display_size();
    let settings = SequenceSettings {
        width,
        height,
        frame_rate: video.frame_rate,
        sample_rate: SequenceSettings::GAME_CLIP.sample_rate,
    };
    let mut project = Project::new("Source", settings);
    project.media.insert(media.id, media.clone());
    let id = project.allocate_id();
    let clip = Clip {
        id,
        media: media.id,
        stream: video.index,
        start: Time::ZERO,
        source_in: Time::ZERO,
        duration: media.duration,
        link: None,
        speed: project::Speed::NORMAL,
        look: project::Look::default(),
        sound: project::Sound::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
    };
    let sequence = project.sequences.iter_mut().next()?;
    let track = sequence
        .video_tracks
        .iter_mut()
        .find(|track| track.kind == TrackKind::Video)?;
    track.clips.push_back(clip);
    Some(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Fingerprint, MediaId, MediaInfo, MediaKind, VideoStream};

    fn media(name: &str, (width, height): (u32, u32), rotation: u16) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: name.into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(10),
            info: MediaInfo {
                container: String::new(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "h264".into(),
                    width,
                    height,
                    frame_rate: Rate::FPS_30,
                    rotation,
                    variable_frame_rate: false,
                }),
                audio: vec![project::AudioStream {
                    index: 1,
                    codec: "aac".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    name: String::new(),
                }],
            },
        }
    }

    /// A project with `a.mkv` imported (picture and sound, linked), its
    /// clip moved to start at 5 s and trimmed to begin 2 s into the file.
    fn project() -> Project {
        let project = Project::new("T", SequenceSettings::GAME_CLIP);
        let mut project = Command::Import {
            sequence: project.active_sequence,
            media: media("a.mkv", (1920, 1080), 0),
        }
        .apply(&project)
        .unwrap();
        let sequence = project.sequences.iter_mut().next().unwrap();
        for track in sequence
            .video_tracks
            .iter_mut()
            .chain(sequence.audio_tracks.iter_mut())
        {
            for clip in track.clips.iter_mut() {
                clip.start = Time::seconds(5);
                clip.source_in = Time::seconds(2);
                clip.duration = Time::seconds(4);
            }
        }
        project
    }

    fn first_clip(project: &Project, kind: TrackKind) -> ClipId {
        let sequence = project.active().unwrap();
        let tracks = match kind {
            TrackKind::Video => &sequence.video_tracks,
            TrackKind::Audio => &sequence.audio_tracks,
        };
        tracks[0].clips[0].id
    }

    fn shown_at(project: &Project, selected: &[ClipId], playhead: Time) -> Option<Time> {
        match source(project, project.active().unwrap(), selected, playhead) {
            Source::Shot { at, .. } => Some(at),
            Source::NothingSelected => None,
        }
    }

    #[test]
    fn nothing_selected_shows_nothing() {
        let project = project();
        assert_eq!(shown_at(&project, &[], Time::seconds(6)), None);
        assert_eq!(shown_at(&project, &[ClipId(999)], Time::seconds(6)), None);
    }

    #[test]
    fn the_playhead_on_the_clip_shows_its_place_in_the_file() {
        let project = project();
        let clip = first_clip(&project, TrackKind::Video);
        // 1 s into the clip, which begins 2 s into its file.
        assert_eq!(
            shown_at(&project, &[clip], Time::seconds(6)),
            Some(Time::seconds(3))
        );
    }

    #[test]
    fn the_playhead_outside_the_clip_shows_its_first_or_last_frame() {
        let project = project();
        let clip = first_clip(&project, TrackKind::Video);
        assert_eq!(
            shown_at(&project, &[clip], Time::seconds(1)),
            Some(Time::seconds(2)),
            "before it: the first frame"
        );
        let last = Time::seconds(6) - Rate::FPS_60.frame_duration();
        assert_eq!(
            shown_at(&project, &[clip], Time::seconds(30)),
            Some(last),
            "after it: the last frame, one timeline frame before its end"
        );
    }

    #[test]
    fn a_selected_sound_clip_shows_the_picture_linked_to_it() {
        let project = project();
        let sound = first_clip(&project, TrackKind::Audio);
        let picture = first_clip(&project, TrackKind::Video);
        let Source::Shot { clip, .. } =
            source(&project, project.active().unwrap(), &[sound], Time::seconds(6))
        else {
            panic!("the linked picture");
        };
        assert_eq!(clip.id, picture);
    }

    #[test]
    fn the_clips_speed_is_followed() {
        let mut project = project();
        let sequence = project.sequences.iter_mut().next().unwrap();
        sequence.video_tracks[0].clips[0].speed = project::Speed::new(2, 1).unwrap();
        let clip = first_clip(&project, TrackKind::Video);
        // 1 s into the clip at twice the speed is 2 s into what it plays.
        assert_eq!(
            shown_at(&project, &[clip], Time::seconds(6)),
            Some(Time::seconds(4))
        );
    }

    #[test]
    fn the_file_alone_is_the_whole_file_at_its_own_size_looking_as_it_is() {
        let media = media("tall.mp4", (1920, 1080), 90);
        let alone = file_alone(&media).unwrap();
        let sequence = alone.active().unwrap();
        assert_eq!(
            (sequence.settings.width, sequence.settings.height),
            (1080, 1920),
            "upright"
        );
        assert_eq!(sequence.settings.frame_rate, Rate::FPS_30);
        let clip = &sequence.video_tracks[0].clips[0];
        assert_eq!(
            (clip.start, clip.source_in, clip.duration),
            (Time::ZERO, Time::ZERO, Time::seconds(10))
        );
        assert_eq!(clip.look, project::Look::default());
        assert!(alone.problems().is_empty(), "{:?}", alone.problems());
        // Asked for at a time in the file, the picture is the file there,
        // filling the frame.
        let frame = crate::frame::describe(&alone, sequence, Time::seconds(3));
        assert_eq!(frame.layers.len(), 1);
        assert_eq!(frame.layers[0].source_time, Time::seconds(3));
        assert!(frame.layers[0].hides_what_is_below(frame.size));
        assert_eq!(frame.layers[0].colour, render::Colour::NEUTRAL);
    }

    #[test]
    fn a_file_without_a_picture_has_no_source() {
        let mut sound = media("a.wav", (0, 0), 0);
        sound.info.video = None;
        assert!(file_alone(&sound).is_none());
    }
}
