//! The timeline's picture at a moment, described: which pictures to draw,
//! where, bottom layer first. Plain numbers, worked out without a graphics
//! card, so it can be tested anywhere and read by the viewer, by export and
//! by anything else that needs the picture.

use project::{Clip, ClipId, MediaId, MediaRef, Project, Sequence, Time};

/// A point or a step in the frame, in frame pixels: x to the right, y down.
pub type Point = (f32, f32);

/// What to draw for one moment of a timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameDescription {
    /// The timeline's picture size.
    pub size: (u32, u32),
    /// What is seen, bottom layer first. Empty in a gap: the frame is black.
    pub layers: Vec<Layer>,
}

/// One clip's picture in the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub clip: ClipId,
    pub media: MediaId,
    /// Which stream of the file holds the picture.
    pub stream: u32,
    /// Time in the file, from its start.
    pub source_time: Time,
    /// Where the shown part of the picture lands in the frame.
    pub area: Area,
    /// The part of the upright picture shown, in 0-1 of its width and height.
    pub part: Part,
    /// 1 hides what is below it, 0 is not seen at all.
    pub opacity: f32,
}

/// A rectangle in the frame that may be turned: its top left corner and its
/// two edges from there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub origin: Point,
    /// From the top left corner to the top right.
    pub across: Point,
    /// From the top left corner to the bottom left.
    pub down: Point,
}

/// A part of a picture: its left and top edge and its size, each in 0-1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl Part {
    pub const WHOLE: Part = Part {
        left: 0.0,
        top: 0.0,
        width: 1.0,
        height: 1.0,
    };
}

impl Area {
    /// The largest upright rectangle of a picture's shape that fits in the
    /// frame, centred: bars at the sides or above and below when the shapes
    /// differ, never a cut-off or stretched picture.
    pub fn fitted(picture: (u32, u32), frame: (u32, u32)) -> Area {
        let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
        let (fw, fh) = (frame.0 as f32, frame.1 as f32);
        // The side that limits the size is the frame's own, exactly.
        let (w, h) = if fw * ph <= fh * pw {
            (fw, ph * fw / pw)
        } else {
            (pw * fh / ph, fh)
        };
        Area {
            origin: ((fw - w) / 2.0, (fh - h) / 2.0),
            across: (w, 0.0),
            down: (0.0, h),
        }
    }

    /// Whether nothing of a frame of `frame` pixels shows past this area.
    /// Only said of upright areas: a turned one is taken not to.
    pub fn covers(&self, frame: (u32, u32)) -> bool {
        // Half a pixel of slack: a fitted picture may miss the edge by rounding.
        const SLACK: f32 = 0.5;
        let upright = self.across.1 == 0.0 && self.down.0 == 0.0;
        let (left, right) = ends(self.origin.0, self.across.0);
        let (top, bottom) = ends(self.origin.1, self.down.1);
        upright
            && left <= SLACK
            && top <= SLACK
            && right >= frame.0 as f32 - SLACK
            && bottom >= frame.1 as f32 - SLACK
    }
}

/// The lower and higher end of a side that starts at `from` and goes `by`.
fn ends(from: f32, by: f32) -> (f32, f32) {
    (from.min(from + by), from.max(from + by))
}

impl Layer {
    /// Whether nothing below this layer can be seen in a frame of `frame` pixels.
    pub fn hides_what_is_below(&self, frame: (u32, u32)) -> bool {
        self.opacity >= 1.0 && self.area.covers(frame)
    }
}

/// What is seen at `at`: every video track that isn't hidden contributes the
/// clip it has there, lower tracks first. Layers that can't be seen, because
/// one above them fills the frame, are left out, so nothing is decoded only
/// to be painted over.
pub fn describe(project: &Project, sequence: &Sequence, at: Time) -> FrameDescription {
    let size = (sequence.settings.width, sequence.settings.height);
    let mut layers = Vec::new();
    // From the top down, stopping at the first layer that hides the rest.
    for track in sequence.video_tracks.iter().rev().filter(|track| !track.hidden) {
        let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.start <= at && at < clip.end())
        else {
            continue;
        };
        let Some(layer) = project
            .media
            .get(&clip.media)
            .and_then(|media| layer(clip, media, size, at))
        else {
            continue;
        };
        if layer.opacity <= 0.0 {
            continue;
        }
        let last = layer.hides_what_is_below(size);
        layers.push(layer);
        if last {
            break;
        }
    }
    layers.reverse();
    FrameDescription { size, layers }
}

/// `clip`'s picture at `at`, or nothing when its file has no picture.
fn layer(clip: &Clip, media: &MediaRef, frame: (u32, u32), at: Time) -> Option<Layer> {
    let video = media.info.video.as_ref()?;
    Some(Layer {
        clip: clip.id,
        media: clip.media,
        stream: clip.stream,
        source_time: clip.source_at(at),
        area: Area::fitted(video.display_size(), frame),
        part: Part::WHOLE,
        opacity: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{
        Command, Fingerprint, MediaInfo, MediaKind, Rate, SequenceSettings, TrackId, TrackKind, VideoStream,
    };

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
                    frame_rate: Rate::FPS_60,
                    rotation,
                    variable_frame_rate: false,
                }),
                audio: Vec::new(),
            },
        }
    }

    fn import(project: &Project, media: MediaRef) -> Project {
        Command::Import {
            sequence: project.active_sequence,
            media,
        }
        .apply(project)
        .unwrap()
    }

    /// Adds a second video track and puts a copy of the first clip of `from`
    /// (a track index) on it, from `start` for `seconds`.
    fn stack(project: &Project, from_clip: usize, start: i64, seconds: i64) -> (Project, TrackId) {
        let mut with_track = Command::AddTrack {
            sequence: project.active_sequence,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(project)
        .unwrap();
        let sequence = with_track.active().unwrap();
        let track = sequence.video_tracks.last().unwrap().id;
        let mut clip = sequence.video_tracks[0].clips[from_clip].clone();
        clip.id = with_track.allocate_id();
        clip.start = Time::seconds(start);
        clip.duration = Time::seconds(seconds);
        clip.link = None;
        let placed = Command::PlaceClip {
            sequence: with_track.active_sequence,
            track,
            clip,
        }
        .apply(&with_track)
        .unwrap();
        (placed, track)
    }

    #[test]
    fn a_picture_of_the_frames_shape_fills_it() {
        let area = Area::fitted((2560, 1440), (1920, 1080));
        assert_eq!(area.origin, (0.0, 0.0));
        assert_eq!(area.across, (1920.0, 0.0));
        assert_eq!(area.down, (0.0, 1080.0));
        assert!(area.covers((1920, 1080)));
    }

    #[test]
    fn a_wider_picture_gets_bars_above_and_below() {
        // 21:9 into 16:9.
        let area = Area::fitted((3440, 1440), (1920, 1080));
        assert_eq!(area.across, (1920.0, 0.0));
        let height = 1920.0 * 1440.0 / 3440.0;
        assert!((area.down.1 - height).abs() < 0.01);
        assert!((area.origin.1 - (1080.0 - height) / 2.0).abs() < 0.01);
        assert!(!area.covers((1920, 1080)));
    }

    #[test]
    fn a_wide_picture_in_a_tall_frame_sits_in_the_middle() {
        let area = Area::fitted((1920, 1080), (1080, 1920));
        assert_eq!(area.across, (1080.0, 0.0));
        assert!((area.down.1 - 607.5).abs() < 0.01);
        assert!((area.origin.1 - 656.25).abs() < 0.01);
    }

    #[test]
    fn a_turned_area_never_counts_as_covering() {
        let mut area = Area::fitted((1920, 1080), (1920, 1080));
        area.across.1 = 1.0;
        assert!(!area.covers((1920, 1080)));
    }

    #[test]
    fn a_gap_is_an_empty_frame_of_the_timelines_size() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let frame = describe(&project, project.active().unwrap(), Time::seconds(30));
        assert_eq!(frame.size, (1920, 1080));
        assert!(frame.layers.is_empty());
    }

    #[test]
    fn one_clip_is_one_layer_at_its_place_in_the_file() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let frame = describe(&project, project.active().unwrap(), Time::seconds(3));
        assert_eq!(frame.layers.len(), 1);
        let layer = &frame.layers[0];
        assert_eq!(layer.source_time, Time::seconds(3));
        assert_eq!(layer.part, Part::WHOLE);
        assert!(layer.hides_what_is_below(frame.size));
    }

    #[test]
    fn a_phone_recording_is_fitted_upright() {
        // Stored lying down, shown turned: 1080 wide and 1920 tall.
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("phone.mp4", (1920, 1080), 90),
        );
        let frame = describe(&project, project.active().unwrap(), Time::seconds(1));
        let area = frame.layers[0].area;
        assert_eq!(area.down, (0.0, 1080.0));
        assert!((area.across.0 - 607.5).abs() < 0.01);
    }

    #[test]
    fn a_clip_that_fills_the_frame_hides_the_ones_below() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let (project, _) = stack(&project, 0, 2, 2);
        let sequence = project.active().unwrap();
        let frame = describe(&project, sequence, Time::seconds(3));
        assert_eq!(frame.layers.len(), 1, "the lower clip can't be seen");
        assert_eq!(frame.layers[0].clip, sequence.video_tracks[1].clips[0].id);
        // Outside the upper clip the lower one shows.
        let frame = describe(&project, sequence, Time::seconds(5));
        assert_eq!(frame.layers[0].clip, sequence.video_tracks[0].clips[0].id);
    }

    #[test]
    fn a_clip_with_bars_lets_the_one_below_show_bottom_first() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("game.mkv", (1920, 1080), 0),
        );
        let project = import(&project, media("wide.mkv", (3440, 1440), 0));
        // The wide clip (second on V1, from 10 s) copied above the first.
        let (project, _) = stack(&project, 1, 2, 2);
        let sequence = project.active().unwrap();
        let frame = describe(&project, sequence, Time::seconds(3));
        assert_eq!(frame.layers.len(), 2);
        assert_eq!(
            frame.layers[0].clip, sequence.video_tracks[0].clips[0].id,
            "bottom first"
        );
        assert_eq!(frame.layers[1].clip, sequence.video_tracks[1].clips[0].id);
    }

    #[test]
    fn hidden_tracks_are_left_out() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let (mut project, _) = stack(&project, 0, 2, 2);
        project.sequences[0].video_tracks[1].hidden = true;
        let sequence = project.active().unwrap();
        let frame = describe(&project, sequence, Time::seconds(3));
        assert_eq!(frame.layers.len(), 1);
        assert_eq!(frame.layers[0].clip, sequence.video_tracks[0].clips[0].id);
    }
}
