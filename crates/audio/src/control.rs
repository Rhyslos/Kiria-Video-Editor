//! How settings reach the mixer without locks, and how what needs memory
//! gets in and out of it.
//!
//! - **Commands** ([`Command`]): small plain values (a track's section of
//!   settings, the master's, which track the analyser shows, a jump)
//!   through a wait-free queue, taken at the start of the next block: a
//!   change is heard within one block (5.3 ms at 48 kHz).
//! - **Swaps** ([`Swap`]): what needs memory (a new track, its lines, a
//!   learned noise) is built here, on the interface's or a worker's
//!   thread, and sent through a second queue; the mixer puts it in place
//!   and sends what it replaced back through a third ([`Returned`]), so
//!   memory is only ever freed here ([`Controller::collect`]). The mixer
//!   takes a swap only when there is room to send the old one back.
//!
//! All three queues are made once, with room for [`QUEUE`] items; a full
//! queue is refused with an explanation rather than waited on.

use crate::dsp::cleanup::{BinNoise, NoiseProfile};
use crate::dsp::lines::Lines;
use crate::feed::{self, FeedReader, FeedWriter};
use crate::readout::{MasterReadout, TrackReadout};
use crate::settings::{MasterChange, TrackChange, TrackSettings};
use crate::strip::Strip;
use diag::AppError;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Room in each queue.
pub const QUEUE: usize = 1024;

/// A change the mixer takes at the start of its next block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// A section of the settings of the track in `slot`.
    Track { slot: u16, change: TrackChange },
    /// A section of the master's settings.
    Master(MasterChange),
    /// Which track the analyser listens to, if any.
    Analyse(Option<u16>),
    /// Carry on from sample `at` of the timeline; every ring is emptied.
    /// `number` comes back through [`Controller::located`] once done.
    Locate { at: u64, number: u64 },
}

/// A track as the mixer holds it: its strip, its input and its readout.
#[derive(Debug)]
pub struct MixerTrack {
    pub(crate) strip: Strip,
    pub(crate) feed: FeedReader,
    pub(crate) readout: Arc<TrackReadout>,
    /// The timeline sample its ring's first frame is, when it says (a
    /// track added while playing): the mixer lines the ring up with it.
    pub(crate) first: Option<u64>,
}

/// Something that needs memory, put in place by the mixer.
#[derive(Debug)]
pub enum Swap {
    /// A track into `slot`. With `carry`, it takes over the running state
    /// of the one it replaces (a rebuilt strip that should not click).
    Track {
        slot: u16,
        track: Box<MixerTrack>,
        carry: bool,
    },
    /// Takes the track out of `slot`.
    Remove { slot: u16 },
    /// New volume and pan lines for the track in `slot`.
    Lines { slot: u16, lines: Box<Lines> },
    /// New noise for the clean-up of the track in `slot`.
    Noise { slot: u16, noise: Box<BinNoise> },
}

/// What the mixer replaced, sent back to be freed off its thread.
#[derive(Debug)]
pub enum Returned {
    Track(Box<MixerTrack>),
    Lines(Box<Lines>),
    Noise(Box<BinNoise>),
}

/// The mixer's ends of the queues.
#[derive(Debug)]
pub(crate) struct Inbox {
    pub commands: rtrb::Consumer<Command>,
    pub swaps: rtrb::Consumer<Swap>,
    pub returned: rtrb::Producer<Returned>,
    pub located: Arc<AtomicU64>,
}

/// The interface's end: sends changes, builds and swaps in what needs
/// memory, frees what comes back.
#[derive(Debug)]
pub struct Controller {
    rate: u32,
    slots: usize,
    commands: rtrb::Producer<Command>,
    swaps: rtrb::Producer<Swap>,
    returned: rtrb::Consumer<Returned>,
    located: Arc<AtomicU64>,
    locates: u64,
    master: Arc<MasterReadout>,
}

pub(crate) fn queues(rate: u32, slots: usize, master: Arc<MasterReadout>) -> (Controller, Inbox) {
    let (commands, commands_in) = rtrb::RingBuffer::new(QUEUE);
    let (swaps, swaps_in) = rtrb::RingBuffer::new(QUEUE);
    let (returned_out, returned) = rtrb::RingBuffer::new(QUEUE);
    let located = Arc::new(AtomicU64::new(0));
    (
        Controller {
            rate,
            slots,
            commands,
            swaps,
            returned,
            located: Arc::clone(&located),
            locates: 0,
            master,
        },
        Inbox {
            commands: commands_in,
            swaps: swaps_in,
            returned: returned_out,
            located,
        },
    )
}

fn behind() -> AppError {
    AppError::new("A sound change couldn't be passed on")
        .why("The sound engine has fallen behind and its queue of changes is full.")
        .fix("Try again in a moment; if sound has stopped, stop and start playback.")
}

impl Controller {
    /// The rate the mixer runs at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    fn slot(&self, slot: usize) -> diag::Result<u16> {
        u16::try_from(slot)
            .ok()
            .filter(|s| usize::from(*s) < self.slots)
            .ok_or_else(|| {
                AppError::new("That track can't be mixed")
                    .why(format!("The mixer was made for {} tracks.", self.slots))
                    .fix("Stop and start playback so the mixer is made again for every track.")
            })
    }

    /// Sends a command; heard from the mixer's next block.
    pub fn send(&mut self, command: Command) -> diag::Result<()> {
        self.commands.push(command).map_err(|_| behind())
    }

    /// Sends one section of a track's settings.
    pub fn change_track(&mut self, slot: usize, change: TrackChange) -> diag::Result<()> {
        let slot = self.slot(slot)?;
        self.send(Command::Track { slot, change })
    }

    /// Sends one section of the master's settings.
    pub fn change_master(&mut self, change: MasterChange) -> diag::Result<()> {
        self.send(Command::Master(change))
    }

    fn swap(&mut self, swap: Swap) -> diag::Result<()> {
        self.swaps.push(swap).map_err(|_| behind())
    }

    /// Builds a track (here: it allocates) and sends it to `slot`, replacing
    /// any track there. Returns the feeder's end of its input ring (room for
    /// `ring_frames`) and what its meter reads.
    pub fn add_track(
        &mut self,
        slot: usize,
        settings: TrackSettings,
        lines: Box<Lines>,
        noise: Option<&NoiseProfile>,
        ring_frames: usize,
    ) -> diag::Result<(FeedWriter, Arc<TrackReadout>)> {
        self.put_track(slot, (settings, lines, noise), ring_frames, None, false)
    }

    /// As [`Controller::add_track`], for a track added while playing: its
    /// ring's first frame is sample `first` of the timeline, so the mixer
    /// plays silence until then or owes what has gone by (see
    /// [`crate::feed`]), and the track is in step with the others.
    pub fn add_track_at(
        &mut self,
        slot: usize,
        settings: TrackSettings,
        lines: Box<Lines>,
        noise: Option<&NoiseProfile>,
        ring_frames: usize,
        first: u64,
    ) -> diag::Result<(FeedWriter, Arc<TrackReadout>)> {
        self.put_track(slot, (settings, lines, noise), ring_frames, Some(first), false)
    }

    /// As [`Controller::add_track`], but the new track carries on the running
    /// state of the one in `slot` (filters, envelopes, the clean-up's windows,
    /// the meter), so the sound goes on without a click: for a track built
    /// again from scratch. Its input is a new ring all the same.
    pub fn rebuild_track(
        &mut self,
        slot: usize,
        settings: TrackSettings,
        lines: Box<Lines>,
        noise: Option<&NoiseProfile>,
        ring_frames: usize,
    ) -> diag::Result<(FeedWriter, Arc<TrackReadout>)> {
        self.put_track(slot, (settings, lines, noise), ring_frames, None, true)
    }

    fn put_track(
        &mut self,
        slot: usize,
        (settings, lines, noise): (TrackSettings, Box<Lines>, Option<&NoiseProfile>),
        ring_frames: usize,
        first: Option<u64>,
        carry: bool,
    ) -> diag::Result<(FeedWriter, Arc<TrackReadout>)> {
        let slot = self.slot(slot)?;
        let noise = noise.unwrap_or(&NoiseProfile::SILENT).bins(self.rate);
        let (writer, reader) = feed::ring(ring_frames);
        let readout = Arc::new(TrackReadout::default());
        let track = Box::new(MixerTrack {
            strip: Strip::new(self.rate, settings, lines, noise),
            feed: reader,
            readout: Arc::clone(&readout),
            first,
        });
        self.swap(Swap::Track { slot, track, carry })?;
        Ok((writer, readout))
    }

    /// Takes the track out of `slot`.
    pub fn remove_track(&mut self, slot: usize) -> diag::Result<()> {
        let slot = self.slot(slot)?;
        self.swap(Swap::Remove { slot })
    }

    /// New volume and pan lines for the track in `slot` (after an edit).
    pub fn set_lines(&mut self, slot: usize, lines: Box<Lines>) -> diag::Result<()> {
        let slot = self.slot(slot)?;
        self.swap(Swap::Lines { slot, lines })
    }

    /// New noise for the clean-up of the track in `slot` (a profile learned
    /// on a worker, or `None` to let everything through).
    pub fn set_noise(&mut self, slot: usize, noise: Option<&NoiseProfile>) -> diag::Result<()> {
        let slot = self.slot(slot)?;
        let noise = noise.unwrap_or(&NoiseProfile::SILENT).bins(self.rate);
        self.swap(Swap::Noise { slot, noise })
    }

    /// Asks the mixer to carry on from sample `at` of the timeline. Returns
    /// the number [`Controller::located`] says once it has (see
    /// [`crate::feed`] for what the feeder does meanwhile).
    pub fn locate(&mut self, at: u64) -> diag::Result<u64> {
        let number = self.locates + 1;
        self.send(Command::Locate { at, number })?;
        self.locates = number;
        Ok(number)
    }

    /// The number of the last jump the mixer made.
    pub fn located(&self) -> u64 {
        self.located.load(Ordering::Acquire)
    }

    /// A handle on what `located` reads, for the feeder's thread.
    pub fn located_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.located)
    }

    /// Frees what the mixer sent back. Call it now and then (every frame
    /// of the interface is fine). Returns how many things it freed.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while let Ok(item) = self.returned.pop() {
            drop(item);
            n += 1;
        }
        n
    }

    /// What the master's meter reads.
    pub fn master(&self) -> Arc<MasterReadout> {
        Arc::clone(&self.master)
    }
}
