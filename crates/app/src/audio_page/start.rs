//! How the Audio page is opened from the command line, for screenshots and
//! the responsiveness test: a tool, a bottom view, a track, and a project
//! made to have many audio tracks (`--audio-tracks 30`).

use super::{Bottom, SplitWay, Tool};
use project::{ClipLines, Project, Time, TrackKind, TrackRole};

/// What the command line asked of the Audio page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Start {
    pub tool: Option<Tool>,
    pub view: Option<(Bottom, Option<SplitWay>)>,
    /// The track the tools start on, counted from 1.
    pub track: Option<usize>,
    /// Make the open timeline have this many audio tracks (from the first
    /// audio track's clips), once its imports are in.
    pub tracks: Option<usize>,
}

/// `timeline`, `mixer`, `split` or `stacked`.
pub fn view_named(name: &str) -> Option<(Bottom, Option<SplitWay>)> {
    match name.to_lowercase().as_str() {
        "timeline" => Some((Bottom::Timeline, None)),
        "mixer" => Some((Bottom::Mixer, None)),
        "split" => Some((Bottom::Split, Some(SplitWay::SideBySide))),
        "stacked" => Some((Bottom::Split, Some(SplitWay::Stacked))),
        _ => None,
    }
}

/// Names and roles the demo tracks go through, like a real mix.
const DEMO: [(&str, TrackRole); 10] = [
    ("Ingrid lav", TrackRole::Dialogue),
    ("Ola lav", TrackRole::Dialogue),
    ("Music: theme", TrackRole::Music),
    ("SFX: wind", TrackRole::Effects),
    ("Boom mic", TrackRole::Dialogue),
    ("Music: piano", TrackRole::Music),
    ("Foley: steps", TrackRole::Effects),
    ("Room tone", TrackRole::Ambience),
    ("Amb: harbour", TrackRole::Ambience),
    ("VO: narrator", TrackRole::Dialogue),
];

/// The project with `n` audio tracks on its open timeline, each a copy of
/// the first audio track's clips, named and given roles and a few effects
/// as a real mix has them. `None` when there is no audio clip to copy or
/// there are that many tracks already.
pub fn many_tracks(project: &Project, n: usize) -> Option<Project> {
    let mut p = project.clone();
    let seq = p.active()?.clone();
    let source = seq.audio_tracks.iter().find(|t| !t.clips.is_empty())?.clone();
    if seq.audio_tracks.len() >= n {
        return None;
    }
    let mut tracks = imbl::Vector::new();
    for i in 0..n {
        let (name, role) = DEMO[i % DEMO.len()];
        let name = if i < DEMO.len() {
            name.to_owned()
        } else {
            format!("{name} {}", i / DEMO.len() + 1)
        };
        let mut track = p.new_track(TrackKind::Audio, &name);
        for clip in &source.clips {
            let mut c = clip.clone();
            c.id = p.allocate_id();
            c.link = None;
            track.clips.push_back(c);
        }
        track.gain_db = -((i % 7) as f32) * 2.0;
        let s = &mut track.sound;
        s.role = role;
        match role {
            TrackRole::Dialogue => {
                s.high_pass.on = true;
                s.eq.on = i % 2 == 0;
                s.eq.bands[1].gain_mdb = -3_000;
                s.eq.bands[3].gain_mdb = 2_500;
                s.compressor.on = true;
            }
            TrackRole::Music => {
                s.ducking.on = true;
                if i == 2 {
                    // A swell in its volume line.
                    let point = |sec: i64, value| project::track_sound::LinePoint {
                        at: Time::seconds(sec),
                        value,
                    };
                    s.volume_line = [point(1, -6_000), point(3, 0), point(5, -4_000)]
                        .into_iter()
                        .collect();
                }
            }
            TrackRole::Ambience => s.loudness.on = true,
            _ => {}
        }
        tracks.push_back(track);
    }
    // A dip in the first dialogue clip's own line, at moments of its file.
    if let Some(first) = tracks.get_mut(0).and_then(|t| t.clips.get_mut(0)) {
        let at = |sec: i64| first.source_in + Time::seconds(sec);
        first.lines = ClipLines {
            volume: [(1, 0), (2, -9_000), (3, 0)]
                .into_iter()
                .map(|(s, value)| project::track_sound::LinePoint { at: at(s), value })
                .collect(),
            pan: imbl::Vector::new(),
        };
    }
    let id = seq.id;
    let target = p.sequences.iter_mut().find(|s| s.id == id)?;
    target.audio_tracks = tracks;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_are_named_and_a_project_gets_many_tracks() {
        assert_eq!(
            view_named("Stacked"),
            Some((Bottom::Split, Some(SplitWay::Stacked)))
        );
        assert_eq!(view_named("mixer"), Some((Bottom::Mixer, None)));
        assert_eq!(view_named("nothing"), None);
        let p = Project::new("T", project::SequenceSettings::GAME_CLIP);
        assert!(many_tracks(&p, 30).is_none(), "no clip to copy");
    }
}
