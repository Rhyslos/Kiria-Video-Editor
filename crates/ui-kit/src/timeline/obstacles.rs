//! What a dragged clip is measured against on every pointer move: the
//! edges it snaps to and the clips that could be in its way. They are
//! gathered once, when a drag starts or the clips change under it, so a
//! frame of the drag costs a binary search per edge and per moving clip
//! instead of a pass over every clip of the timeline for each of them.

use super::{TimelineClip, TimelineData, TimelineTrack};

/// The clips that stay put during a drag, sorted for looking up.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Obstacles {
    /// What they were gathered from, one entry per clip in the order the
    /// tracks list them (video tracks, then audio tracks).
    seen: Vec<Seen>,
    /// The edges (start, then end) of every clip that stays, sorted, each
    /// with its first place in the order the clips list them: of two
    /// edges as near, the first listed wins, as it always has.
    edges: Vec<(f64, usize)>,
    /// Per video track, then per audio track (by place in its list).
    video: Vec<Track>,
    audio: Vec<Track>,
}

/// One clip as a drag sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    /// Its track's place among all tracks, video first.
    track: usize,
    start: u64,
    duration: u64,
    moves: bool,
}

impl Seen {
    fn of(track: usize, c: &TimelineClip, moves: &impl Fn(&TimelineClip) -> bool) -> Seen {
        Seen {
            track,
            start: c.start.to_bits(),
            duration: c.duration.to_bits(),
            moves: moves(c),
        }
    }
}

/// The clips of one track that stay, by start: their starts and, at each,
/// the furthest end of any of them up to there.
#[derive(Debug, Clone, Default, PartialEq)]
struct Track {
    starts: Vec<f64>,
    furthest: Vec<f64>,
}

impl Obstacles {
    /// Gathers the clips of `data` that `moves` leaves out, unless that was
    /// done for the same clips already (a pass with no allocation).
    pub fn refresh(&mut self, data: &TimelineData, moves: impl Fn(&TimelineClip) -> bool) {
        if !self.gathered_from(data, &moves) {
            *self = Obstacles::gather(data, &moves);
        }
    }

    fn gathered_from(&self, data: &TimelineData, moves: &impl Fn(&TimelineClip) -> bool) -> bool {
        self.video.len() == data.video.len()
            && self.audio.len() == data.audio.len()
            && all_tracks(data)
                .flat_map(|(i, t)| t.clips.iter().map(move |c| Seen::of(i, c, moves)))
                .eq(self.seen.iter().copied())
    }

    fn gather(data: &TimelineData, moves: &impl Fn(&TimelineClip) -> bool) -> Obstacles {
        let mut seen = Vec::new();
        let mut edges = Vec::new();
        for (i, track) in all_tracks(data) {
            for c in &track.clips {
                let clip = Seen::of(i, c, moves);
                if !clip.moves {
                    edges.push((c.start, seen.len() * 2));
                    edges.push((c.start + c.duration, seen.len() * 2 + 1));
                }
                seen.push(clip);
            }
        }
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        edges.dedup_by(|later, first| later.0.to_bits() == first.0.to_bits());
        let area = |tracks: &[TimelineTrack]| -> Vec<Track> {
            tracks
                .iter()
                .map(|t| Track::of(t.clips.iter().filter(|c| !moves(c))))
                .collect()
        };
        Obstacles {
            seen,
            edges,
            video: area(data.video),
            audio: area(data.audio),
        }
    }

    /// The clip edge nearest to `at` within `reach`, as how far it is
    /// (`edge - at`); of two as near, the first listed.
    pub fn nearest_edge(&self, at: f64, reach: f64) -> Option<f64> {
        let mut best: Option<(f64, usize)> = None;
        let mut consider = |&(edge, order): &(f64, usize)| {
            let d = edge - at;
            let better = best.is_none_or(|(b, o)| d.abs() < b.abs() || (d.abs() == b.abs() && order < o));
            if d.abs() <= reach && better {
                best = Some((d, order));
            }
        };
        // The first edge at or after `at`, and those after it as near
        // (they can round to the same distance); the same before `at`.
        let after = self.edges.partition_point(|(edge, _)| *edge < at);
        let mut run = |edges: &mut dyn Iterator<Item = &(f64, usize)>| {
            let mut first = None;
            for e in edges {
                let d = e.0 - at;
                if first.is_some_and(|f: f64| f != d) {
                    break;
                }
                first = Some(d);
                consider(e);
            }
        };
        run(&mut self.edges[after..].iter());
        run(&mut self.edges[..after].iter().rev());
        best.map(|(d, _)| d)
    }

    /// Whether a clip that stays on the track at `index` of its area (audio
    /// or video) overlaps `start..end` by more than `slack` at both ends.
    pub fn taken(&self, audio: bool, index: usize, start: f64, end: f64, slack: f64) -> bool {
        let tracks = if audio { &self.audio } else { &self.video };
        let Some(track) = tracks.get(index) else {
            return false;
        };
        let before = track.starts.partition_point(|s| *s < end - slack);
        before
            .checked_sub(1)
            .and_then(|last| track.furthest.get(last))
            .is_some_and(|furthest| start < furthest - slack)
    }
}

impl Track {
    fn of<'a>(clips: impl Iterator<Item = &'a TimelineClip>) -> Track {
        let mut spans: Vec<(f64, f64)> = clips.map(|c| (c.start, c.start + c.duration)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut furthest = Vec::with_capacity(spans.len());
        let mut so_far = f64::NEG_INFINITY;
        for (_, end) in &spans {
            so_far = so_far.max(*end);
            furthest.push(so_far);
        }
        Track {
            starts: spans.into_iter().map(|(start, _)| start).collect(),
            furthest,
        }
    }
}

/// Every track with its place among all of them, video first.
fn all_tracks<'a>(data: &TimelineData<'a>) -> impl Iterator<Item = (usize, &'a TimelineTrack)> + use<'a> {
    data.video.iter().chain(data.audio).enumerate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::CacheBar;

    fn clip(id: u64, start: f64, duration: f64, selected: bool) -> TimelineClip {
        TimelineClip {
            id,
            start,
            duration,
            name: String::new(),
            info: String::new(),
            selected,
            missing: false,
            link: None,
            waveform: None,
            pictures: None,
            loading: None,
            sound: Default::default(),
            keyframes: Vec::new(),
            speed_curve: None,
            shows: Default::default(),
            line: None,
            slip_room: None,
            plays_until: None,
            transition_in: None,
            graded: false,
            fades: Default::default(),
        }
    }

    fn track(id: u64, clips: Vec<TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: String::new(),
            clips,
            muted: false,
            solo: false,
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
        }
    }

    fn data<'a>(video: &'a [TimelineTrack], audio: &'a [TimelineTrack]) -> TimelineData<'a> {
        TimelineData {
            video,
            audio,
            length: 100.0,
            fps: 30.0,
            playhead: 0.0,
            needle: None,
            region: None,
            markers: &[],
            highlights: &[],
            spans: &[],
            follow: false,
            cache: CacheBar {
                ranges: &[],
                about: "",
            },
            keyframe_eases: &[],
            roles: &[],
        }
    }

    /// A small deterministic stream of numbers.
    struct Numbers(u64);
    impl Numbers {
        fn next(&mut self, below: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % below
        }
    }

    /// Tracks of clips at whole or half seconds (so edges often meet
    /// exactly), out of order and overlapping now and then, some selected.
    fn mixed(numbers: &mut Numbers, tracks: u64) -> Vec<TimelineTrack> {
        (0..tracks)
            .map(|t| {
                let clips = (0..numbers.next(12))
                    .map(|c| {
                        let start = numbers.next(80) as f64 / 2.0;
                        let duration = 0.5 + numbers.next(12) as f64 / 2.0;
                        clip(t * 100 + c, start, duration, numbers.next(3) == 0)
                    })
                    .collect();
                track(t, clips)
            })
            .collect()
    }

    /// The nearest edge as it was found before: every edge of every clip
    /// that stays, the first of the nearest.
    fn nearest_by_every_edge(data: &TimelineData, at: f64, reach: f64) -> Option<f64> {
        data.video
            .iter()
            .chain(data.audio)
            .flat_map(|t| &t.clips)
            .filter(|c| !c.selected)
            .flat_map(|c| [c.start, c.start + c.duration])
            .map(|p| p - at)
            .filter(|d| d.abs() <= reach)
            .min_by(|a, b| a.abs().total_cmp(&b.abs()))
    }

    /// Whether a clip is in the way as it was found before: every clip of
    /// the track.
    fn taken_by_every_clip(track: &TimelineTrack, start: f64, end: f64, slack: f64) -> bool {
        track
            .clips
            .iter()
            .any(|c| !c.selected && c.start < end - slack && start < c.start + c.duration - slack)
    }

    #[test]
    fn finds_the_same_edges_and_clashes_as_a_look_at_every_clip() {
        let mut numbers = Numbers(7);
        for _ in 0..200 {
            let video = mixed(&mut numbers, 3);
            let audio = mixed(&mut numbers, 2);
            let data = data(&video, &audio);
            let mut room = Obstacles::default();
            room.refresh(&data, |c| c.selected);
            for _ in 0..40 {
                // Quarter seconds: on edges, between them and halfway.
                let at = numbers.next(200) as f64 / 4.0 - 2.0;
                let reach = numbers.next(8) as f64 / 4.0;
                assert_eq!(
                    room.nearest_edge(at, reach),
                    nearest_by_every_edge(&data, at, reach),
                    "at {at}, reach {reach}"
                );
                let (start, end) = (at, at + numbers.next(20) as f64 / 4.0);
                let slack = [0.0, 1.0 / 60.0, 0.25][numbers.next(3) as usize];
                for (audio, tracks) in [(false, &video), (true, &audio)] {
                    for (i, t) in tracks.iter().enumerate() {
                        assert_eq!(
                            room.taken(audio, i, start, end, slack),
                            taken_by_every_clip(t, start, end, slack),
                            "{start}..{end} on track {i} (audio {audio})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn of_two_edges_as_near_the_first_listed_wins() {
        // A clip from 6 s to 9 s and one from 1 s to 4 s: the start of the
        // first and the end of the second are as near to 5 s. The one
        // listed first is snapped to, whichever comes first in time.
        let (later, earlier) = (clip(1, 6.0, 3.0, false), clip(2, 1.0, 3.0, false));
        for (clips, wanted) in [
            (vec![later.clone(), earlier.clone()], 1.0),
            (vec![earlier, later], -1.0),
        ] {
            let video = [track(0, clips)];
            let data = data(&video, &[]);
            let mut room = Obstacles::default();
            room.refresh(&data, |_| false);
            assert_eq!(room.nearest_edge(5.0, 2.0), Some(wanted));
            assert_eq!(nearest_by_every_edge(&data, 5.0, 2.0), Some(wanted));
        }
    }

    #[test]
    fn is_gathered_again_when_the_clips_change() {
        let mut video = vec![track(0, vec![clip(1, 0.0, 2.0, false)])];
        let mut room = Obstacles::default();
        room.refresh(&data(&video, &[]), |c| c.selected);
        assert_eq!(room.nearest_edge(2.5, 1.0), Some(-0.5));
        // Selected now: it moves along and is no longer something to snap to.
        video[0].clips[0].selected = true;
        room.refresh(&data(&video, &[]), |c| c.selected);
        assert_eq!(room.nearest_edge(2.5, 1.0), None);
        video[0].clips[0].selected = false;
        video[0].clips[0].start = 1.0;
        room.refresh(&data(&video, &[]), |c| c.selected);
        assert_eq!(room.nearest_edge(2.5, 1.0), Some(0.5));
        assert!(room.taken(false, 0, 2.0, 4.0, 0.0));
        assert!(!room.taken(false, 1, 2.0, 4.0, 0.0), "no such track");
    }
}
