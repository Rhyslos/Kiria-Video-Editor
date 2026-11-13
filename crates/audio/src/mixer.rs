//! The mixer: the real-time end, owned by the sound card's callback (or by
//! export's thread, which runs it the same way without a device).
//!
//! Each [`Mixer::process`] makes one block of [`BLOCK`] frames:
//!
//! 1. Takes every waiting swap while there is room to send the old thing
//!    back, then every waiting command (so a command sent after adding a
//!    track reaches that track).
//! 2. Runs the dialogue tracks (the ducking key) first, summing their sound
//!    into the key; works out the key's speech level; then runs every
//!    other track, ducked by it. Each track's sound comes from its ring;
//!    a dry ring is silence, owed to the feeder.
//! 3. Mute and solo: a track is heard when not muted, and, while any track
//!    is soloed, only if it is soloed.
//! 4. The master: volume, compressor, limiter, meter.
//!
//! Nothing here allocates, frees, locks or waits; memory is made once in
//! [`mixer`].

use crate::BLOCK;
use crate::analyser::{self, Analyser, Tap};
use crate::control::{self, Command, Controller, Inbox, MixerTrack, Returned, Swap};
use crate::dsp::cleanup;
use crate::dsp::duck::Key;
use crate::master::Master;
use crate::readout::MasterReadout;
use crate::settings::MasterSettings;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The real-time end.
#[derive(Debug)]
pub struct Mixer {
    rate: u32,
    /// The timeline sample the next block starts at.
    pos: u64,
    tracks: Vec<Option<Box<MixerTrack>>>,
    inbox: Inbox,
    key: Key,
    key_sum: [[f32; BLOCK]; 2],
    key_db: [f32; BLOCK],
    work: [[f32; BLOCK]; 2],
    sum: [[f32; BLOCK]; 2],
    tap: Tap,
    analysed: Option<u16>,
    master: Master,
    master_readout: Arc<MasterReadout>,
    /// Frames of silence played for dry rings, summed over tracks, since made.
    dry: u64,
}

/// Makes a mixer with room for `slots` tracks at `rate`, the controller
/// that drives it, and the analyser's worker end. Allocates: before the
/// sound starts.
pub fn mixer(rate: u32, slots: usize, master: MasterSettings) -> (Mixer, Controller, Analyser) {
    let master_readout = Arc::new(MasterReadout::default());
    let (controller, inbox) = control::queues(rate, slots, Arc::clone(&master_readout));
    let (tap, analyser) = analyser::analyser(rate);
    let mixer = Mixer {
        rate,
        pos: 0,
        tracks: (0..slots).map(|_| None).collect(),
        inbox,
        key: Key::new(rate as f32),
        key_sum: [[0.0; BLOCK]; 2],
        key_db: [0.0; BLOCK],
        work: [[0.0; BLOCK]; 2],
        sum: [[0.0; BLOCK]; 2],
        tap,
        analysed: None,
        master: Master::new(rate, master),
        master_readout,
        dry: 0,
    };
    (mixer, controller, analyser)
}

impl Mixer {
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// How late the mix comes out, in frames, whatever is on: the
    /// clean-up's delay on every track plus the limiter's. The clock the
    /// picture follows takes this off.
    pub fn latency(&self) -> usize {
        cleanup::LATENCY + self.master.latency()
    }

    /// The timeline sample the next block starts at.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Frames of silence played because a ring was dry, summed over tracks.
    pub fn dry_frames(&self) -> u64 {
        self.dry
    }

    /// The settings of the track in `slot`, if there is one (for tests and logs).
    pub fn track_settings(&self, slot: usize) -> Option<&crate::settings::TrackSettings> {
        self.tracks.get(slot)?.as_ref().map(|t| t.strip.settings())
    }

    /// The master's settings now.
    pub fn master_settings(&self) -> &MasterSettings {
        self.master.settings()
    }

    /// Makes the next block: `out` is interleaved stereo.
    pub fn process(&mut self, out: &mut [f32; 2 * BLOCK]) {
        // Swaps first: a command sent after adding a track is meant for it.
        self.take_swaps();
        self.take_commands();
        let any_solo = self.tracks.iter().flatten().any(|t| t.strip.settings().solo);
        for s in &mut self.sum {
            s.fill(0.0);
        }
        for s in &mut self.key_sum {
            s.fill(0.0);
        }
        // Dialogue first: its sound is the key the others duck under.
        for key_pass in [true, false] {
            if !key_pass {
                let [kl, kr] = &self.key_sum;
                self.key.listen(kl, kr, &mut self.key_db);
            }
            for (slot, track) in self.tracks.iter_mut().enumerate() {
                let Some(track) = track else { continue };
                if track.strip.is_key() != key_pass {
                    continue;
                }
                let [l, r] = &mut self.work;
                self.dry += track.feed.read(l, r) as u64;
                let s = track.strip.settings();
                let audible = !s.mute && (!any_solo || s.solo);
                let tap = (self.analysed == Some(slot as u16)).then_some(&mut self.tap);
                track
                    .strip
                    .process(self.pos, &self.key_db, audible, tap, l, r, &track.readout);
                for (sum, part) in self.sum.iter_mut().zip(&self.work) {
                    for (s, p) in sum.iter_mut().zip(part) {
                        *s += p;
                    }
                }
                if key_pass {
                    for (sum, part) in self.key_sum.iter_mut().zip(&self.work) {
                        for (s, p) in sum.iter_mut().zip(part) {
                            *s += p;
                        }
                    }
                }
            }
        }
        let [l, r] = &mut self.sum;
        self.master.process(l, r, out, &self.master_readout);
        self.pos += BLOCK as u64;
    }

    fn take_commands(&mut self) {
        while let Ok(command) = self.inbox.commands.pop() {
            match command {
                Command::Track { slot, change } => {
                    if let Some(Some(t)) = self.tracks.get_mut(usize::from(slot)) {
                        t.strip.apply(change);
                    }
                }
                Command::Master(change) => self.master.apply(change),
                Command::Analyse(slot) => self.analysed = slot,
                Command::Locate { at, number } => {
                    self.pos = at;
                    for t in self.tracks.iter_mut().flatten() {
                        t.feed.clear();
                    }
                    self.inbox.located.store(number, Ordering::Release);
                }
            }
        }
    }

    /// Takes waiting swaps, one at a time, while the old thing can be sent
    /// back (nothing is ever freed here).
    fn take_swaps(&mut self) {
        while self.inbox.returned.slots() > 0 {
            let Ok(swap) = self.inbox.swaps.pop() else {
                return;
            };
            let back = match swap {
                Swap::Track {
                    slot,
                    mut track,
                    carry,
                } => match self.tracks.get_mut(usize::from(slot)) {
                    Some(place) => {
                        if carry && let Some(old) = place.as_ref() {
                            track.strip.take_state(&old.strip);
                        }
                        place.replace(track).map(Returned::Track)
                    }
                    // No such slot: straight back, never freed here.
                    None => Some(Returned::Track(track)),
                },
                Swap::Remove { slot } => self
                    .tracks
                    .get_mut(usize::from(slot))
                    .and_then(Option::take)
                    .map(Returned::Track),
                Swap::Lines { slot, mut lines } => {
                    if let Some(Some(t)) = self.tracks.get_mut(usize::from(slot)) {
                        t.strip.swap_lines(&mut lines);
                    }
                    Some(Returned::Lines(lines))
                }
                Swap::Noise { slot, mut noise } => {
                    if let Some(Some(t)) = self.tracks.get_mut(usize::from(slot)) {
                        t.strip.swap_noise(&mut noise);
                    }
                    Some(Returned::Noise(noise))
                }
            };
            if let Some(back) = back {
                // There was room: checked above, and only this thread pushes.
                let _ = self.inbox.returned.push(back);
            }
        }
    }
}
