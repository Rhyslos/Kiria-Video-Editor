//! Learning a track's noise for its clean-up, on a worker: the track's
//! clips over a stretch of the timeline are mixed as the feeder mixes them
//! (before the track's own effects) and handed to
//! `audio::dsp::cleanup::NoiseProfile`.
//!
//! A short stretch (up to [`ALL_NOISE`]) is taken to be noise alone, as
//! when a pause is marked; a longer one is someone talking with pauses,
//! and the quietest quarter of it is the noise.

use crate::mix;
use audio::dsp::cleanup::NoiseProfile;
use audio::settings::noise_to_project;
use diag::AppError;
use media::audio::CHANNELS;
use project::{Project, SequenceId, Time, TrackId};

/// The rate the noise is learned at (a profile works at any rate).
const RATE: u32 = 48_000;
/// Up to this long, a stretch is all noise.
pub const ALL_NOISE: f64 = 3.0;
/// The shortest stretch to learn from, seconds.
pub const SHORTEST: f64 = 0.1;
/// The most that is read, seconds.
pub const LONGEST: f64 = 60.0;

/// Learns the noise of `track` between `from` and `to` on `sequence`.
/// Allocates and decodes: for a worker.
pub fn learn(
    project: &Project,
    sequence: SequenceId,
    track: TrackId,
    (from, to): (Time, Time),
) -> diag::Result<project::track_sound::NoiseProfile> {
    let seq = project.sequence(sequence).ok_or_else(|| {
        AppError::new("The noise can't be learned")
            .why("The timeline is not in the project any more.")
            .fix("Open the timeline and learn it again.")
    })?;
    let track = seq.audio_tracks.iter().find(|t| t.id == track).ok_or_else(|| {
        AppError::new("The noise can't be learned")
            .why("The track is not on the timeline any more.")
            .fix("Pick a track and learn it again.")
    })?;
    let seconds = (to - from).as_secs_f64().min(LONGEST);
    if seconds < SHORTEST {
        return Err(AppError::new("The noise can't be learned")
            .why("The stretch to learn from is too short.")
            .fix("Mark a region (or select a clip) over a pause where only the noise is heard."));
    }
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut out = vec![0f32; frames * CHANNELS];
    mix::Mixer::new(RATE).mix_track(project, track, from.as_secs_f64(), &mut out);
    let (l, r): (Vec<f32>, Vec<f32>) = out.chunks_exact(CHANNELS).map(|f| (f[0], f[1])).unzip();
    if l.iter().chain(&r).all(|s| *s == 0.0) {
        return Err(AppError::new("The noise can't be learned")
            .why(format!("{} is silent there.", track.name))
            .fix(
                "Mark a region (or select a clip) where the track has sound, over a pause in the talking.",
            ));
    }
    let profile = if seconds <= ALL_NOISE {
        NoiseProfile::learn(RATE, &l, &r)
    } else {
        NoiseProfile::learn_quietest(RATE, &l, &r)
    };
    Ok(noise_to_project(&profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Command;
    use std::path::Path;

    fn project() -> Project {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture).unwrap();
        Command::Import { sequence: seq, media }.apply(&p).unwrap()
    }

    #[test]
    fn the_noise_of_a_track_is_learned_and_refused_where_silent() {
        let p = project();
        let seq = p.active().unwrap();
        let track = seq.audio_tracks[0].id;
        let half = Time::from_ratio(1, 2).unwrap();
        let profile = learn(&p, seq.id, track, (Time::ZERO, half)).unwrap();
        assert_eq!(profile.levels_mdb.len(), project::track_sound::NOISE_BANDS);
        assert!(
            profile.levels_mdb.iter().any(|l| *l > -150_000),
            "something heard"
        );
        let far = seq.length() + Time::seconds(10);
        assert!(
            learn(&p, seq.id, track, (far, far + half)).is_err(),
            "nothing there"
        );
        assert!(learn(&p, seq.id, track, (half, half)).is_err(), "too short");
    }
}
