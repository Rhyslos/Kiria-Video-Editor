//! Volume and pan lines between the project and the timeline widget.
//!
//! The project keeps a track's lines in timeline time and a clip's at
//! moments of its file (`project::ClipLines`); the widget draws and edits
//! points of (seconds on the timeline, value): decibels for volume, -1 to
//! 1 for pan. Points coming back are put on whole frames.

use imbl::Vector;
use project::track_sound::LinePoint;
use project::{Clip, ClipLines, Command, Project, Rate, Sequence, SequenceId, Time, TrackSound};
use ui_kit::timeline::{LineBeingSet, LineKind, LineOwner, LinePoints};

/// A project value (thousandths of a dB, millionths of a pan) as the widget's.
fn value_of(kind: LineKind, value: i32) -> f32 {
    match kind {
        LineKind::Volume => value as f32 / 1_000.0,
        LineKind::Pan => value as f32 / 1_000_000.0,
    }
}

/// The widget's value as the project keeps it, within range.
fn value_to(kind: LineKind, value: f32) -> i32 {
    match kind {
        LineKind::Volume => {
            let mdb = (value * 1_000.0).round() as i32;
            mdb.clamp(TrackSound::LINE_QUIETEST, TrackSound::LINE_LOUDEST)
        }
        LineKind::Pan => ((value * 1_000_000.0).round() as i32).clamp(-1_000_000, 1_000_000),
    }
}

/// Seconds as a time on a whole frame of `rate`, never before the start.
fn on_frame(t: f64, rate: Rate) -> Time {
    let micros = (t.max(0.0) * 1_000_000.0).round() as i64;
    rate.snap(Time::from_ratio(micros, 1_000_000).unwrap_or(Time::ZERO))
}

fn line_of(sound: &TrackSound, kind: LineKind) -> &Vector<LinePoint> {
    match kind {
        LineKind::Volume => &sound.volume_line,
        LineKind::Pan => &sound.pan_line,
    }
}

fn clip_line(lines: &ClipLines, kind: LineKind) -> &Vector<LinePoint> {
    match kind {
        LineKind::Volume => &lines.volume,
        LineKind::Pan => &lines.pan,
    }
}

/// A track's line as the widget draws it.
pub fn track_points(sound: &TrackSound, kind: LineKind) -> LinePoints {
    line_of(sound, kind)
        .iter()
        .map(|p| (p.at.as_secs_f64(), value_of(kind, p.value)))
        .collect::<Vec<_>>()
        .into()
}

/// A clip's line as the widget draws it: its points' moments of the file
/// brought onto the timeline.
pub fn clip_points(clip: &Clip, kind: LineKind) -> LinePoints {
    clip_line(&clip.lines, kind)
        .iter()
        .map(|p| (clip.timeline_at(p.at).as_secs_f64(), value_of(kind, p.value)))
        .collect::<Vec<_>>()
        .into()
}

/// Points as the project keeps them, on whole frames, in order, one per
/// moment (the later of two on one frame is kept).
fn to_project(points: &[(f64, f32)], kind: LineKind, at: impl Fn(f64) -> Time) -> Vector<LinePoint> {
    let mut out: Vec<LinePoint> = Vec::with_capacity(points.len());
    for &(t, v) in points {
        let point = LinePoint {
            at: at(t),
            value: value_to(kind, v),
        };
        match out.last_mut() {
            Some(last) if last.at >= point.at => *last = point,
            _ => out.push(point),
        }
    }
    out.into_iter().collect()
}

/// The track's sound with its `kind` line set to `points`.
pub fn track_with(sound: &TrackSound, kind: LineKind, points: &[(f64, f32)], rate: Rate) -> TrackSound {
    let line = to_project(points, kind, |t| on_frame(t, rate));
    let mut sound = sound.clone();
    match kind {
        LineKind::Volume => sound.volume_line = line,
        LineKind::Pan => sound.pan_line = line,
    }
    sound
}

/// The clip's lines with its `kind` line set to `points` (seconds on the
/// timeline, taken at the clip's place in its file).
pub fn clip_with(clip: &Clip, kind: LineKind, points: &[(f64, f32)], rate: Rate) -> ClipLines {
    let line = to_project(points, kind, |t| clip.source_at(on_frame(t, rate)));
    let mut lines = clip.lines.clone();
    match kind {
        LineKind::Volume => lines.volume = line,
        LineKind::Pan => lines.pan = line,
    }
    lines
}

/// The command that sets `owner`'s `kind` line to `points` on `seq`.
pub fn command(seq: &Sequence, owner: LineOwner, kind: LineKind, points: &[(f64, f32)]) -> Option<Command> {
    let rate = seq.settings.frame_rate;
    let sequence: SequenceId = seq.id;
    match owner {
        LineOwner::Track(id) => {
            let track = seq.audio_tracks.iter().find(|t| t.id.0 == id)?;
            Some(Command::SetTrackSound {
                sequence,
                track: track.id,
                sound: Box::new(track_with(&track.sound, kind, points, rate)),
            })
        }
        LineOwner::Clip(id) => {
            let clip = seq
                .audio_tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .find(|c| c.id.0 == id)?;
            Some(Command::SetClipLines {
                sequence,
                clip: clip.id,
                lines: Box::new(clip_with(clip, kind, points, rate)),
            })
        }
    }
}

/// The project with a line being dragged where it is now, to be heard.
pub fn with_line_being_set(project: &Project, line: LineBeingSet) -> Option<Project> {
    let seq = project.active()?;
    command(seq, line.owner, line.kind, line.points)?
        .apply(project)
        .ok()
}

/// Fills in each audio track's and clip's `kind` line for the timeline
/// (`audio` in the timeline's order of `seq`'s audio tracks).
pub fn fill(audio: &mut [ui_kit::TimelineTrack], seq: &Sequence, kind: LineKind) {
    for (drawn, track) in audio.iter_mut().zip(&seq.audio_tracks) {
        drawn.line = Some(track_points(&track.sound, kind));
        drawn.role = Some(super::roles::index(track.sound.role));
        // The drawn clips are the track's, in its order: matched in one
        // walk (a search per clip was quadratic in the clips of a track).
        let mut clips = track.clips.iter();
        for clip in drawn.clips.iter_mut() {
            let found = match clips.next() {
                Some(c) if c.id.0 == clip.id => Some(c),
                _ => track.clips.iter().find(|c| c.id.0 == clip.id),
            };
            if let Some(c) = found {
                clip.line = Some(clip_points(c, kind));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_line_goes_to_whole_frames_and_back() {
        let rate = Rate::FPS_60;
        let sound = track_with(
            &TrackSound::default(),
            LineKind::Volume,
            &[(1.004, -6.0), (2.0, 3.25), (2.001, 4.0)],
            rate,
        );
        assert_eq!(sound.volume_line.len(), 2, "one point per frame, the later kept");
        assert_eq!(sound.volume_line[0].at, Time::seconds(1));
        assert_eq!(sound.volume_line[1].value, 4_000);
        assert!(sound.check().is_ok());
        let back = track_points(&sound, LineKind::Volume);
        assert_eq!(&back[..], &[(1.0, -6.0), (2.0, 4.0)]);
        let pan = track_with(&sound, LineKind::Pan, &[(0.5, -0.4)], rate);
        assert_eq!(pan.pan_line[0].value, -400_000);
        assert_eq!(pan.volume_line, sound.volume_line, "the other line stays");
        let loud = track_with(&TrackSound::default(), LineKind::Volume, &[(0.0, 40.0)], rate);
        assert_eq!(loud.volume_line[0].value, TrackSound::LINE_LOUDEST);
    }

    #[test]
    fn a_clip_line_is_kept_at_moments_of_its_file() {
        let clip = Clip {
            id: project::ClipId(7),
            media: project::MediaId(1),
            stream: 0,
            start: Time::seconds(10),
            source_in: Time::seconds(100),
            duration: Time::seconds(5),
            link: None,
            speed: Default::default(),
            look: Default::default(),
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
            lines: Default::default(),
            effects: Default::default(),
            mode: Default::default(),
        };
        let lines = clip_with(&clip, LineKind::Volume, &[(11.0, -3.0)], Rate::FPS_60);
        assert_eq!(lines.volume[0].at, Time::seconds(101), "a moment of the file");
        let moved = Clip {
            start: Time::seconds(20),
            lines,
            ..clip
        };
        assert_eq!(
            &clip_points(&moved, LineKind::Volume)[..],
            &[(21.0, -3.0)],
            "it moved with the clip"
        );
    }
}
