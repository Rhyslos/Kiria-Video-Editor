//! The timeline's picture at a moment, described: which pictures to draw,
//! where, bottom layer first. Plain numbers, worked out without a graphics
//! card, so it can be tested anywhere and read by the viewer, by export and
//! by anything else that needs the picture.

use project::{Clip, ClipId, Look, MediaId, MediaRef, Project, Sequence, Time};
use render::{Area, Colour, Part, Placement};

/// What to draw for one moment of a timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameDescription {
    /// The timeline's picture size.
    pub size: (u32, u32),
    /// What is seen, bottom layer first. Empty in a gap: the frame is black.
    pub layers: Vec<Layer>,
}

impl FrameDescription {
    /// The same picture drawn at another size (a preview smaller than the
    /// timeline's): everything keeps its place in the frame.
    pub fn at_size(mut self, size: (u32, u32)) -> FrameDescription {
        let x = size.0 as f32 / self.size.0.max(1) as f32;
        let y = size.1 as f32 / self.size.1.max(1) as f32;
        for layer in &mut self.layers {
            layer.area = layer.area.scaled(x, y);
        }
        self.size = size;
        self
    }
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
    /// Its quick colour.
    pub colour: Colour,
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
    // Its look at this moment: keyframes give the numbers that change.
    let look = clip.look_at(at);
    let part = shown_part(&look);
    Some(Layer {
        clip: clip.id,
        media: clip.media,
        stream: clip.stream,
        source_time: clip.source_at(at),
        area: shown_area(&look, video.display_size(), frame),
        part,
        opacity: fraction(look.opacity),
        colour: colour(&look),
    })
}

/// One of a look's whole numbers (millionths) as a fraction.
fn fraction(value: i32) -> f32 {
    (f64::from(value) / f64::from(project::look::ONE)) as f32
}

/// Where a clip's whole picture sits in a frame of `frame` pixels: fitted,
/// then sized, moved and turned as its look says.
pub fn placement(look: &Look, picture: (u32, u32), frame: (u32, u32)) -> Placement {
    let fitted = Area::fitted(picture, frame);
    let (w, h) = fitted.size();
    let scale = fraction(look.scale);
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    Placement {
        centre: (fw * (0.5 + fraction(look.x)), fh * (0.5 + fraction(look.y))),
        size: (w * scale, h * scale),
        degrees: fraction(look.turn),
    }
}

/// Where what is seen of a clip's picture lands in a frame of `frame`
/// pixels: its place, with what its look cuts off left out. What is drawn,
/// and what an outline around the picture follows.
pub fn shown_area(look: &Look, picture: (u32, u32), frame: (u32, u32)) -> Area {
    Area::of_part(placement(look, picture, frame), shown_part(look))
}

/// The part of the picture left after what its look cuts off each side.
pub fn shown_part(look: &Look) -> Part {
    let (left, top) = (fraction(look.crop_left), fraction(look.crop_top));
    Part {
        left,
        top,
        width: (1.0 - left - fraction(look.crop_right)).max(0.01),
        height: (1.0 - top - fraction(look.crop_bottom)).max(0.01),
    }
}

/// A look's quick colour as the compositor takes it.
pub fn colour(look: &Look) -> Colour {
    if !look.is_coloured() {
        return Colour::NEUTRAL;
    }
    Colour::from_sliders(
        fraction(look.exposure),
        fraction(look.contrast),
        fraction(look.saturation),
        fraction(look.temperature),
        fraction(look.tint),
    )
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
    fn a_smaller_preview_keeps_everything_in_its_place() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("wide.mkv", (3440, 1440), 0),
        );
        let frame = describe(&project, project.active().unwrap(), Time::seconds(1)).at_size((960, 540));
        assert_eq!(frame.size, (960, 540));
        let area = frame.layers[0].area;
        assert_eq!(area.across, (960.0, 0.0));
        assert!((area.down.1 - 960.0 * 1440.0 / 3440.0).abs() < 0.01);
    }

    /// The first clip on V1 given `look`.
    fn with_look(project: &Project, look: Look) -> Project {
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        Command::SetLook {
            sequence: project.active_sequence,
            clip,
            look,
        }
        .apply(project)
        .unwrap()
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn a_look_sizes_moves_and_cuts_the_picture() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("cam.mkv", (1920, 1080), 0),
        );
        // A third of the size, towards the bottom right corner.
        let small = Look {
            scale: 300_000,
            x: 320_000,
            y: 280_000,
            ..Look::PLAIN
        };
        let frame_of = |p: &Project| describe(p, p.active().unwrap(), Time::seconds(1));
        let layer = &frame_of(&with_look(&project, small)).layers[0];
        assert!(near(layer.area.across, (576.0, 0.0)) && near(layer.area.down, (0.0, 324.0)));
        // Centre at (0.82 x 1920, 0.78 x 1080).
        assert!(
            near(layer.area.origin, (1574.4 - 288.0, 842.4 - 162.0)),
            "{:?}",
            layer.area
        );
        assert!(!layer.hides_what_is_below((1920, 1080)));

        // A fifth cut off the left: the rest stays where it was.
        let cut = Look {
            crop_left: 200_000,
            ..Look::PLAIN
        };
        let layer = &frame_of(&with_look(&project, cut)).layers[0];
        assert_eq!(layer.part.left, 0.2);
        assert!((layer.part.width - 0.8).abs() < 1e-6);
        assert!(near(layer.area.origin, (384.0, 0.0)) && near(layer.area.across, (1536.0, 0.0)));
        assert!(
            !layer.hides_what_is_below((1920, 1080)),
            "the cut side shows what is below"
        );

        // Larger than the frame still hides what is below; see-through doesn't.
        let large = Look {
            scale: 1_500_000,
            ..Look::PLAIN
        };
        assert!(frame_of(&with_look(&project, large)).layers[0].hides_what_is_below((1920, 1080)));
        let faint = Look {
            opacity: 500_000,
            ..Look::PLAIN
        };
        let layer = &frame_of(&with_look(&project, faint)).layers[0];
        assert_eq!(layer.opacity, 0.5);
        assert!(!layer.hides_what_is_below((1920, 1080)));
        // Nothing to see at all is no layer.
        let gone = Look {
            opacity: 0,
            ..Look::PLAIN
        };
        assert!(frame_of(&with_look(&project, gone)).layers.is_empty());
    }

    #[test]
    fn a_look_turns_the_picture_and_carries_its_quick_colour() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("cam.mkv", (1920, 1080), 0),
        );
        let turned = Look {
            turn: 90_000_000,
            temperature: 400_000,
            ..Look::PLAIN
        };
        let changed = with_look(&project, turned);
        let layer = &describe(&changed, changed.active().unwrap(), Time::seconds(1)).layers[0];
        // A quarter turn clockwise: the top edge runs down.
        assert!(near(layer.area.across, (0.0, 1920.0)), "{:?}", layer.area);
        assert!(
            !layer.hides_what_is_below((1920, 1080)),
            "a turned picture is taken not to cover"
        );
        assert!(layer.colour.gains[0] > layer.colour.gains[2], "warmer");
        let plain = describe(&project, project.active().unwrap(), Time::seconds(1));
        assert!(plain.layers[0].colour.is_neutral());
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
