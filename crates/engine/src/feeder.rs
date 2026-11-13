//! The feeder: each sound track's clips mixed ahead of time into that
//! track's input ring of the real-time mixer (`crates/audio`).
//!
//! Everything that can wait is done here, never in the sound card's
//! callback: opening and decoding files, seeking, each clip's volume, pan
//! and fades, speed and speed ramps, nested timelines (all of it
//! [`crate::mix::Mixer::mix_track`], sample-exact). The track's own volume,
//! mute, solo and effects, and the master, are the real-time mixer's, so a
//! change to them is heard within one block instead of after what is
//! already in the rings.
//!
//! A track's ring is written a piece ([`PIECE`] frames) at a time, from the
//! timeline sample it is at; when the mixer found a ring dry it played
//! silence and owes it, and the feeder skips as much, so a late file comes
//! back in step. Playback and export write the same pieces at the same
//! places, so they make the same sound.
//!
//! After an edit, [`Feeder::update`] works out what changed per track
//! (`audio::TrackSettings::changes_from`) and sends only that; new lines
//! and noise profiles are built here and swapped in; tracks added while
//! playing come in lined up to the sample ([`audio::Controller::add_track_at`]),
//! tracks taken out leave.

use crate::mix;
use audio::settings::{TrackSettings, lines_from_project, noise_from_project};
use audio::{Controller, FeedWriter};
use diag::AppError;
use media::audio::CHANNELS;
use project::{Project, Sequence, SequenceId, Track, TrackId, TrackSound};

/// Frames mixed at a time per track: four of the mixer's blocks.
pub const PIECE: usize = 4 * audio::BLOCK;

/// How many tracks a mixer has room for: every track of the timeline and
/// plenty more to add while playing.
pub fn slots_for(sequence: &Sequence) -> usize {
    (sequence.audio_tracks.len() * 2).max(64)
}

/// A track as the feeder knows it.
struct Fed {
    id: TrackId,
    /// Its clips' decoders and what they read last: a mixer per track, so
    /// tracks never take each other's decoders away.
    clips: mix::Mixer,
    slot: usize,
    writer: FeedWriter,
    settings: TrackSettings,
    /// What the lines and noise were built from.
    sound: TrackSound,
    /// The timeline sample the next frame written is.
    next: u64,
}

pub struct Feeder {
    rate: u32,
    sequence: SequenceId,
    project: Project,
    /// A timeline sample and the same moment in seconds: where mixing
    /// started or last jumped. Seconds are worked out from here as export
    /// always has, so the sound read is the same to the sample.
    origin: (u64, f64),
    tracks: Vec<Fed>,
    slots: usize,
    /// Frames each track's ring holds.
    ring: usize,
    piece: Vec<f32>,
    /// Mixing has begun: tracks added from now on come in lined up with it.
    live: bool,
    /// Tracks nobody can hear are not decoded (export, where nothing
    /// changes while it runs; while playing they are, so unmuting is
    /// heard at once).
    skip_unheard: bool,
}

impl std::fmt::Debug for Feeder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Feeder({} tracks at {} Hz)", self.tracks.len(), self.rate)
    }
}

fn no_timeline() -> AppError {
    AppError::new("The timeline's sound can't be mixed")
        .why("The timeline is not in the project any more.")
        .fix("Open another timeline and play again.")
}

impl Feeder {
    /// Adds every sound track of `sequence` to the mixer `controller`
    /// drives (each with a ring of `ring` frames), ready to mix from
    /// timeline sample `at`, which is `at_secs` seconds. The caller then
    /// sends the mixer there (`Controller::locate`) before the first fill.
    pub fn new(
        project: Project,
        sequence: SequenceId,
        controller: &mut Controller,
        (slots, ring): (usize, usize),
        (at, at_secs): (u64, f64),
        skip_unheard: bool,
    ) -> diag::Result<Feeder> {
        let rate = controller.rate();
        let mut feeder = Feeder {
            rate,
            sequence,
            project: Project::new_automatic(""),
            origin: (at, at_secs),
            tracks: Vec::new(),
            slots,
            ring: ring.max(PIECE),
            piece: vec![0.0; PIECE * CHANNELS],
            live: false,
            skip_unheard,
        };
        feeder.update(project, controller)?;
        feeder.live = true;
        Ok(feeder)
    }

    /// Mixing goes on from timeline sample `at` (`at_secs` seconds): after
    /// the mixer has jumped there (`Controller::located`), which emptied
    /// every ring and forgot what was owed.
    pub fn jump(&mut self, at: u64, at_secs: f64) {
        self.origin = (at, at_secs);
        for t in &mut self.tracks {
            t.next = at;
            // Owed before the jump is forgotten by the mixer too.
            let _ = t.writer.take_owed();
        }
    }

    /// Writes a piece into every ring with room for one, again and again
    /// while `go_on` says so, until no ring has room. Returns the frames
    /// written.
    pub fn fill(&mut self, go_on: &dyn Fn() -> bool) -> usize {
        let mut written = 0;
        loop {
            let round = self.fill_round();
            written += round;
            if round == 0 || !go_on() {
                return written;
            }
        }
    }

    /// Fills until every ring holds at least `frames` (or is full), for the
    /// first sound after starting or jumping.
    pub fn prime(&mut self, frames: usize, go_on: &dyn Fn() -> bool) {
        let frames = frames.min(self.ring);
        while go_on() && self.tracks.iter().any(|t| self.ring - t.writer.room() < frames) {
            if self.fill_round() == 0 {
                return;
            }
        }
    }

    /// One piece into each ring with room for it.
    fn fill_round(&mut self) -> usize {
        let Some(seq) = self.project.sequence(self.sequence).cloned() else {
            return 0;
        };
        let soloing = seq.audio_tracks.iter().any(|t| t.solo);
        let mut written = 0;
        for i in 0..self.tracks.len() {
            let fed = &mut self.tracks[i];
            fed.next += fed.writer.take_owed();
            if fed.writer.room() < PIECE {
                continue;
            }
            let at = fed.next;
            let id = fed.id;
            let track = seq.audio_tracks.iter().find(|t| t.id == id);
            let heard = track.is_some_and(|t| !t.muted && (!soloing || t.solo));
            match track {
                Some(track) if heard || !self.skip_unheard => {
                    let start =
                        self.origin.1 + at.saturating_sub(self.origin.0) as f64 / f64::from(self.rate);
                    let fed = &mut self.tracks[i];
                    fed.clips.mix_track(&self.project, track, start, &mut self.piece);
                }
                _ => self.piece.fill(0.0),
            }
            let fed = &mut self.tracks[i];
            let frames = fed.writer.write(&self.piece);
            fed.next += frames as u64;
            written += frames;
        }
        written
    }

    /// Takes the project as it is now: changed track settings travel as
    /// changes, new lines and noise are swapped in, new tracks are added
    /// in step and removed ones taken out. The clips' sound follows the
    /// new project from what is mixed next.
    pub fn update(&mut self, project: Project, controller: &mut Controller) -> diag::Result<()> {
        let seq = project.sequence(self.sequence).ok_or_else(no_timeline)?;
        // Taken out.
        let mut gone = Vec::new();
        self.tracks.retain(|t| {
            let kept = seq.audio_tracks.iter().any(|a| a.id == t.id);
            if !kept {
                gone.push(t.slot);
            }
            kept
        });
        for slot in gone {
            controller.remove_track(slot)?;
        }
        // Where a new track's first frame goes: in step with the others.
        let first = self.tracks.iter().map(|t| t.next).min().unwrap_or(self.origin.0);
        for track in &seq.audio_tracks {
            match self.tracks.iter_mut().find(|t| t.id == track.id) {
                Some(fed) => changed(fed, track, controller)?,
                None => {
                    let fed = self.add(track, controller, self.live.then_some(first))?;
                    self.tracks.push(fed);
                }
            }
        }
        // In the timeline's order (only the order sound is summed in follows
        // the mixer's slots, which tracks keep).
        let order: Vec<TrackId> = seq.audio_tracks.iter().map(|t| t.id).collect();
        self.tracks
            .sort_by_key(|t| order.iter().position(|id| *id == t.id).unwrap_or(usize::MAX));
        self.project = project;
        Ok(())
    }

    /// Adds a track to the mixer in the first free slot. `first` is the
    /// timeline sample its ring starts at when added while mixing.
    fn add(&self, track: &Track, controller: &mut Controller, first: Option<u64>) -> diag::Result<Fed> {
        let slot = (0..self.slots)
            .find(|s| self.tracks.iter().all(|t| t.slot != *s))
            .ok_or_else(|| {
                AppError::new("A new track can't be heard yet")
                    .why(format!(
                        "Playback was started with room for {} tracks.",
                        self.slots
                    ))
                    .fix("Stop and start playback again.")
            })?;
        let settings = TrackSettings::from_project(track);
        let lines = lines_from_project(&track.sound, self.rate);
        let noise = noise_from_project(&track.sound);
        let (writer, _readout) = match first {
            Some(at) => controller.add_track_at(slot, settings, lines, noise.as_ref(), self.ring, at)?,
            None => controller.add_track(slot, settings, lines, noise.as_ref(), self.ring)?,
        };
        Ok(Fed {
            id: track.id,
            clips: mix::Mixer::new(self.rate),
            slot,
            writer,
            settings,
            sound: track.sound.clone(),
            next: first.unwrap_or(self.origin.0),
        })
    }
}

/// Sends what changed about a track the mixer already has.
fn changed(fed: &mut Fed, track: &Track, controller: &mut Controller) -> diag::Result<()> {
    let settings = TrackSettings::from_project(track);
    for change in settings.changes_from(&fed.settings) {
        controller.change_track(fed.slot, change)?;
    }
    fed.settings = settings;
    let sound = &track.sound;
    if sound.volume_line != fed.sound.volume_line || sound.pan_line != fed.sound.pan_line {
        controller.set_lines(fed.slot, lines_from_project(sound, controller.rate()))?;
    }
    if sound.cleanup.profile != fed.sound.cleanup.profile {
        controller.set_noise(fed.slot, noise_from_project(sound).as_ref())?;
    }
    fed.sound = sound.clone();
    Ok(())
}
