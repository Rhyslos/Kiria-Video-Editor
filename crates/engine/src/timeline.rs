//! What is on the timeline at a moment.

use project::{Clip, MediaRef, Project, Sequence, Time};

/// The picture showing at a time: which file, which stream, and where in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Showing<'a> {
    pub media: &'a MediaRef,
    pub clip: &'a Clip,
    /// Time in the file, from its start.
    pub source_time: Time,
}

/// The top visible video clip at `at`: higher tracks cover lower ones;
/// hidden tracks are skipped. `None` in a gap.
pub fn video_at<'a>(project: &'a Project, sequence: &'a Sequence, at: Time) -> Option<Showing<'a>> {
    sequence
        .video_tracks
        .iter()
        .rev()
        .filter(|t| !t.hidden)
        .find_map(|track| track.clips.iter().find(|c| c.start <= at && at < c.end()))
        .and_then(|clip| {
            let media = project.media.get(&clip.media)?;
            Some(Showing {
                media,
                clip,
                source_time: clip.source_at(at),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Fingerprint, MediaId, MediaInfo, MediaKind, Rate, SequenceSettings, VideoStream};

    fn media(name: &str) -> MediaRef {
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
                    width: 1920,
                    height: 1080,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: Vec::new(),
            },
        }
    }

    #[test]
    fn the_clip_under_the_time_and_its_place_in_the_file() {
        let p = Project::new("T", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: media("a.mkv"),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: media("b.mkv"),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let at = video_at(&p, s, Time::seconds(12)).unwrap();
        assert_eq!(at.media.path, std::path::PathBuf::from("b.mkv"));
        assert_eq!(at.source_time, Time::seconds(2));
        assert_eq!(
            video_at(&p, s, Time::seconds(3)).unwrap().source_time,
            Time::seconds(3)
        );
        assert!(
            video_at(&p, s, Time::seconds(25)).is_none(),
            "after the end is a gap"
        );
    }

    #[test]
    fn higher_tracks_cover_lower_ones_unless_hidden() {
        let p = Project::new("T", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let mut p = Command::Import {
            sequence: seq,
            media: media("low.mkv"),
        }
        .apply(&p)
        .unwrap();
        let p2 = Command::AddTrack {
            sequence: seq,
            kind: project::TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let s = p2.active().unwrap();
        let (v2, mut clip) = (s.video_tracks[1].id, s.video_tracks[0].clips[0].clone());
        clip.start = Time::seconds(2);
        clip.duration = Time::seconds(2);
        clip.link = None;
        p = Command::PlaceClip {
            sequence: seq,
            track: v2,
            clip,
        }
        .apply(&p2)
        .unwrap();
        let s = p.active().unwrap();
        let top = video_at(&p, s, Time::seconds(3)).unwrap();
        assert_eq!(top.clip.start, Time::seconds(2), "V2 is on top");
        assert_eq!(top.source_time, Time::seconds(1));
        let mut hidden = p.clone();
        hidden.sequences[0].video_tracks[1].hidden = true;
        let s = hidden.active().unwrap();
        assert_eq!(
            video_at(&hidden, s, Time::seconds(3)).unwrap().clip.start,
            Time::ZERO
        );
    }
}
