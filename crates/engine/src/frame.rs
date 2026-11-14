//! The timeline's picture at a moment, described: which pictures to draw,
//! where, bottom layer first. Plain numbers, worked out without a graphics
//! card, so it can be tested anywhere and read by the viewer, by export and
//! by anything else that needs the picture.

use project::{
    Clip, ClipId, ClipKind, FrameBlend, Look, MediaId, MediaRef, Project, Rate, Sequence, SequenceId, Time,
};
use render::{Area, Colour, Part, Placement};

/// What to draw for one moment of a timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameDescription {
    /// The timeline's picture size.
    pub size: (u32, u32),
    /// Every picture of a file that is seen, in the order they are drawn
    /// (the top one last), whether in the timeline itself or in a timeline
    /// nested in it: what has to be decoded. Empty in a gap: the frame is
    /// black.
    pub layers: Vec<Layer>,
    /// How they are put together, bottom first.
    pub steps: Vec<Step>,
}

/// One thing drawn over what is below it.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The picture `layers[i]`.
    Picture(usize),
    /// A nested timeline: its own steps put together into a picture of its
    /// own (see-through where nothing is), then placed as one picture.
    Timeline(Group),
    /// An adjustment layer: everything below it in the same picture,
    /// coloured.
    Adjust(Adjust),
}

/// A nested timeline's picture in the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub clip: ClipId,
    /// The size it is put together at: the nested timeline's own, scaled
    /// as the frame is (`at_size`).
    pub size: (u32, u32),
    /// What it is made of, bottom first; areas are in its own picture.
    pub steps: Vec<Step>,
    /// Where the shown part of its picture lands in the frame.
    pub area: Area,
    pub part: Part,
    pub opacity: f32,
    pub colour: Colour,
    /// Its picture effects, in order, after its colour.
    pub effects: Vec<render::Effect>,
}

/// What an adjustment layer does to everything below it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjust {
    pub clip: ClipId,
    /// How much of the coloured picture shows over the picture as it was.
    pub opacity: f32,
    pub colour: Colour,
}

impl FrameDescription {
    /// The same picture drawn at another size (a preview smaller than the
    /// timeline's): everything keeps its place in the frame, and nested
    /// timelines are put together at the same scale.
    pub fn at_size(mut self, size: (u32, u32)) -> FrameDescription {
        let x = size.0 as f32 / self.size.0.max(1) as f32;
        let y = size.1 as f32 / self.size.1.max(1) as f32;
        for layer in &mut self.layers {
            layer.area = layer.area.scaled(x, y);
        }
        scale_groups(&mut self.steps, x, y);
        self.size = size;
        self
    }

    /// Whether anything is drawn that has to come from a nested timeline
    /// or be coloured by an adjustment layer.
    pub fn is_flat(&self) -> bool {
        self.steps.iter().all(|step| matches!(step, Step::Picture(_)))
    }
}

fn scale_groups(steps: &mut [Step], x: f32, y: f32) {
    for step in steps {
        if let Step::Timeline(group) = step {
            group.area = group.area.scaled(x, y);
            group.size = (
                ((group.size.0 as f32 * x).round() as u32).max(1),
                ((group.size.1 as f32 * y).round() as u32).max(1),
            );
            scale_groups(&mut group.steps, x, y);
        }
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
    /// Its picture effects, in order, after its colour: drawn upright at
    /// the size it takes in the frame, changed, then placed.
    pub effects: Vec<render::Effect>,
    /// With frame blending on and the moment between two recorded frames:
    /// the next frame to mix in, and how much of it.
    pub blend: Option<Blend>,
}

/// The second picture of a blended layer: the recorded frame after the one
/// on screen at the layer's moment, and its share of the mix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blend {
    /// When the next frame starts in the file.
    pub next: Time,
    /// How far the moment is from the frame on screen towards the next
    /// one: above 0 and below 1.
    pub weight: f32,
}

/// Where `at` (in a file recorded at `rate`) lies between two recorded
/// frames: `None` exactly on one (a clip slowed to a speed that suits its
/// frame rate, such as 120 fps at half speed on 60 fps, always is).
pub fn blend_at(at: Time, rate: Rate) -> Option<Blend> {
    let frame = rate.frame_at(at);
    let past = at - rate.frame_start(frame);
    if past <= Time::ZERO {
        return None;
    }
    // past / frame length = past * rate, exactly, then as a share.
    let share = i128::from(past.num()) * i128::from(rate.num());
    let whole = i128::from(past.den()) * i128::from(rate.den());
    let weight = (share as f64 / whole as f64) as f32;
    (weight > 0.0 && weight < 1.0).then_some(Blend {
        next: rate.frame_start(frame + 1),
        weight,
    })
}

impl Layer {
    /// Whether nothing below this layer can be seen in a frame of `frame` pixels.
    pub fn hides_what_is_below(&self, frame: (u32, u32)) -> bool {
        self.opacity >= 1.0 && self.area.covers(frame)
    }
}

/// What is seen at `at`: every video track that isn't hidden contributes the
/// clip it has there, lower tracks first. A nested timeline contributes
/// what is seen in it at its moment, put together and placed as one
/// picture; an adjustment layer colours what is below it. Layers that can't
/// be seen, because one above them fills the frame, are left out, so
/// nothing is decoded only to be painted over.
pub fn describe(project: &Project, sequence: &Sequence, at: Time) -> FrameDescription {
    let size = (sequence.settings.width, sequence.settings.height);
    let (nodes, _) = seen(project, sequence, at, 0);
    let mut layers = Vec::new();
    let steps = flatten(nodes, &mut layers);
    FrameDescription { size, layers, steps }
}

/// What is seen in a timeline before it is numbered: a tree.
enum Node {
    Picture(Layer),
    Timeline(Group, Vec<Node>),
    Adjust(Adjust),
}

/// What is seen in `sequence` at `at`, bottom first, and whether it fills
/// its whole frame with nothing see-through.
fn seen(project: &Project, sequence: &Sequence, at: Time, depth: usize) -> (Vec<Node>, bool) {
    let size = (sequence.settings.width, sequence.settings.height);
    let mut nodes = Vec::new();
    let mut opaque = false;
    // From the top down, stopping at the first layer that hides the rest.
    for track in sequence.video_tracks.iter().rev().filter(|track| !track.hidden) {
        let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.start <= at && at < clip.end())
        else {
            continue;
        };
        let hides = match clip.kind {
            ClipKind::File => {
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
                let hides = layer.hides_what_is_below(size);
                nodes.push(Node::Picture(layer));
                hides
            }
            ClipKind::Timeline(id) => {
                let Some((group, inner, inner_opaque)) = nested(project, clip, id, size, at, depth) else {
                    continue;
                };
                let hides = inner_opaque && group.opacity >= 1.0 && group.area.covers(size);
                nodes.push(Node::Timeline(group, inner));
                hides
            }
            ClipKind::Adjustment => {
                let look = clip.look_at(at);
                let adjust = Adjust {
                    clip: clip.id,
                    opacity: fraction(look.opacity),
                    colour: colour(&look),
                };
                if adjust.opacity > 0.0 && !adjust.colour.is_neutral() {
                    nodes.push(Node::Adjust(adjust));
                }
                false
            }
        };
        if hides {
            opaque = true;
            break;
        }
    }
    nodes.reverse();
    // An adjustment layer with nothing below it has nothing to colour.
    let first_picture = nodes
        .iter()
        .position(|node| !matches!(node, Node::Adjust(_)))
        .unwrap_or(nodes.len());
    nodes.drain(..first_picture);
    (nodes, opaque)
}

/// A nested timeline's clip at `at`: its group (without steps yet), what is
/// seen in it at its moment, and whether that fills its frame. Nothing
/// when nothing of it is seen, or it is nested deeper than is followed.
fn nested(
    project: &Project,
    clip: &Clip,
    id: SequenceId,
    frame: (u32, u32),
    at: Time,
    depth: usize,
) -> Option<(Group, Vec<Node>, bool)> {
    if depth >= crate::timeline::DEEPEST {
        return None;
    }
    let inner = project.sequence(id)?;
    let look = clip.look_at(at);
    let opacity = fraction(look.opacity);
    if opacity <= 0.0 {
        return None;
    }
    let (nodes, opaque) = seen(project, inner, clip.source_at(at), depth + 1);
    if nodes.is_empty() {
        return None;
    }
    let size = (inner.settings.width.max(1), inner.settings.height.max(1));
    let group = Group {
        clip: clip.id,
        size,
        steps: Vec::new(),
        area: shown_area(&look, size, frame),
        part: shown_part(&look),
        opacity,
        colour: colour(&look),
        effects: effects(clip, at),
    };
    Some((group, nodes, opaque))
}

/// Numbers the pictures in drawing order and turns the tree into steps.
fn flatten(nodes: Vec<Node>, layers: &mut Vec<Layer>) -> Vec<Step> {
    nodes
        .into_iter()
        .map(|node| match node {
            Node::Picture(layer) => {
                layers.push(layer);
                Step::Picture(layers.len() - 1)
            }
            Node::Timeline(group, inner) => Step::Timeline(Group {
                steps: flatten(inner, layers),
                ..group
            }),
            Node::Adjust(adjust) => Step::Adjust(adjust),
        })
        .collect()
}

/// `clip`'s picture at `at`, or nothing when its file has no picture.
fn layer(clip: &Clip, media: &MediaRef, frame: (u32, u32), at: Time) -> Option<Layer> {
    let video = media.info.video.as_ref()?;
    // Its look at this moment: keyframes give the numbers that change.
    let look = clip.look_at(at);
    let part = shown_part(&look);
    let source_time = clip.source_at(at);
    Some(Layer {
        clip: clip.id,
        media: clip.media,
        stream: clip.stream,
        source_time,
        area: shown_area(&look, video.display_size(), frame),
        part,
        opacity: fraction(look.opacity),
        colour: colour(&look),
        effects: effects(clip, at),
        blend: match clip.blend {
            FrameBlend::Blend => blend_at(source_time, video.frame_rate),
            FrameBlend::Off => None,
        },
    })
}

/// A clip's picture effects that are switched on, as the shaders take them
/// at `at` (a time of the timeline the clip is on). Sizes become reference
/// pixels (of 1080 lines). Grain's pattern follows the moment of the file,
/// so the same frame always has the same grain, in preview and export.
fn effects(clip: &Clip, at: Time) -> Vec<render::Effect> {
    let seed = (clip.source_at(at).as_secs_f64() * 1200.0)
        .round()
        .rem_euclid(f64::from(1 << 24)) as u32;
    clip.effects
        .applied()
        .map(|kind| match kind {
            project::EffectKind::Vignette(v) => render::Effect::Vignette {
                amount: fraction(v.amount),
                size: fraction(v.size),
                softness: fraction(v.softness),
            },
            project::EffectKind::Grain(g) => render::Effect::Grain {
                amount: fraction(g.amount),
                size: 1.0 + 3.0 * fraction(g.size),
                seed,
            },
            project::EffectKind::Sharpen(s) => render::Effect::Sharpen {
                amount: fraction(s.amount),
                radius: 0.5 + 2.5 * fraction(s.radius),
            },
            project::EffectKind::Glow(g) => render::Effect::Glow {
                amount: fraction(g.amount),
                threshold: fraction(g.threshold),
                radius: 2.0 + 58.0 * fraction(g.radius),
            },
            project::EffectKind::Blur(b) => render::Effect::Blur {
                radius: 2.0 + 118.0 * fraction(b.radius),
                spot: spot(&b.spot),
            },
            project::EffectKind::Pixelate(p) => render::Effect::Pixelate {
                size: 4.0 + 96.0 * fraction(p.size),
                spot: spot(&p.spot),
            },
        })
        .collect()
}

/// A box of a picture as the shaders take it: its edges, 0-1.
fn spot(spot: &project::effects::Spot) -> render::Spot {
    render::Spot {
        left: fraction(spot.left),
        top: fraction(spot.top),
        right: fraction(spot.left + spot.width),
        bottom: fraction(spot.top + spot.height),
    }
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
                stated_length: None,
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
    fn a_clips_effects_that_are_on_go_with_its_picture() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let sequence = project.active_sequence;
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        let add = |p: &Project| {
            Command::AddEffect {
                sequence,
                clip,
                kind: project::EffectKind::ALL[0],
            }
            .apply(p)
            .unwrap()
        };
        let project = add(&add(&project));
        let mut effects = project.active().unwrap().video_tracks[0].clips[0].effects.clone();
        effects.0[0].off = true;
        let project = Command::SetEffects {
            sequence,
            clip,
            effects: Box::new(effects),
        }
        .apply(&project)
        .unwrap();
        let frame = describe(&project, project.active().unwrap(), Time::seconds(3));
        assert_eq!(
            frame.layers[0].effects,
            [render::Effect::Vignette {
                amount: 0.5,
                size: 0.6,
                softness: 0.5
            }],
            "the one switched off is left out"
        );
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

    #[test]
    fn blending_mixes_in_the_next_frame_only_between_recorded_frames() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let sequence = project.active().unwrap();
        let clip = sequence.video_tracks[0].clips[0].id;
        let slow = Command::SetSpeed {
            sequence: sequence.id,
            clip,
            speed: project::Speed::new(1, 4).unwrap(),
        }
        .apply(&project)
        .unwrap();
        let blended = Command::SetFrameBlend {
            sequence: sequence.id,
            clip,
            blend: FrameBlend::Blend,
        }
        .apply(&slow)
        .unwrap();
        let at = |p: &Project, frame: i64| {
            let sequence = p.active().unwrap();
            describe(p, sequence, Rate::FPS_60.frame_start(frame)).layers[0].clone()
        };
        // A 60 fps file at a quarter speed on 60 fps: 4 timeline frames a recorded one.
        assert_eq!(at(&blended, 8).blend, None, "on a recorded frame");
        let between = at(&blended, 9).blend.unwrap();
        assert_eq!(between.next, Rate::FPS_60.frame_start(3));
        assert!((between.weight - 0.25).abs() < 1e-6);
        assert!((at(&blended, 11).blend.unwrap().weight - 0.75).abs() < 1e-6);
        assert_eq!(at(&slow, 9).blend, None, "off unless asked for");
    }

    #[test]
    fn a_high_frame_rate_file_slowed_to_suit_it_needs_no_blending() {
        for (fps, num, den) in [(120, 1, 2), (144, 5, 12)] {
            let rate = Rate::whole(fps);
            assert!((0..1000).all(|frame| {
                let at = Rate::FPS_60.frame_start(frame);
                let speed = project::Speed::new(num, den).unwrap();
                blend_at(speed.scale(at), rate).is_none()
            }));
        }
        assert!(blend_at(Time::from_ratio(1, 240).unwrap(), Rate::FPS_120).is_some());
    }

    #[test]
    fn a_nested_timeline_is_one_picture_made_of_its_own_layers() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let outer = project.active_sequence;
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        let project = Command::NestClips {
            sequence: outer,
            clips: vec![clip],
            name: "Inner".into(),
        }
        .apply(&project)
        .unwrap();
        let nested = project.active().unwrap().video_tracks[0].clips[0].id;
        // Half size: what is below would show around it.
        let project = with_look(
            &project,
            Look {
                scale: 500_000,
                ..Look::PLAIN
            },
        );
        let frame = describe(&project, project.active().unwrap(), Time::seconds(3));
        assert_eq!(frame.layers.len(), 1, "the file's picture is decoded");
        assert_eq!(frame.layers[0].source_time, Time::seconds(3));
        assert!(!frame.is_flat());
        let [Step::Timeline(group)] = frame.steps.as_slice() else {
            panic!("one nested timeline: {:?}", frame.steps);
        };
        assert_eq!(group.clip, nested);
        assert_eq!(group.size, (1920, 1080));
        assert_eq!(group.steps, vec![Step::Picture(0)]);
        assert!(near(group.area.across, (960.0, 0.0)), "{:?}", group.area);
        // A smaller preview puts the nested timeline together smaller too.
        let small = frame.at_size((960, 540));
        let Step::Timeline(group) = &small.steps[0] else {
            panic!("still nested");
        };
        assert_eq!(group.size, (960, 540));
        assert!(near(small.layers[0].area.across, (960.0, 0.0)));
    }

    #[test]
    fn an_adjustment_layer_colours_what_is_below_it() {
        let project = import(
            &Project::new("T", SequenceSettings::GAME_CLIP),
            media("a.mkv", (1920, 1080), 0),
        );
        let sequence = project.active_sequence;
        let project = Command::AddAdjustment {
            sequence,
            at: Time::seconds(2),
            length: Time::seconds(20),
            track: None,
        }
        .apply(&project)
        .unwrap();
        let layer = project.active().unwrap().video_tracks[1].clips[0].id;
        // As it comes it changes nothing, so it is left out.
        let frame = describe(&project, project.active().unwrap(), Time::seconds(3));
        assert_eq!(frame.steps, vec![Step::Picture(0)]);
        let warm = Command::SetLook {
            sequence,
            clip: layer,
            look: Look {
                temperature: 500_000,
                opacity: 750_000,
                ..Look::PLAIN
            },
        }
        .apply(&project)
        .unwrap();
        let frame = describe(&warm, warm.active().unwrap(), Time::seconds(3));
        let [Step::Picture(0), Step::Adjust(adjust)] = frame.steps.as_slice() else {
            panic!("the picture, then the adjustment: {:?}", frame.steps);
        };
        assert_eq!(adjust.clip, layer);
        assert_eq!(adjust.opacity, 0.75);
        assert!(adjust.colour.gains[0] > adjust.colour.gains[2], "warmer");
        // Over a gap it has nothing to colour.
        let frame = describe(&warm, warm.active().unwrap(), Time::seconds(15));
        assert!(frame.steps.is_empty() && frame.layers.is_empty());
    }
}
