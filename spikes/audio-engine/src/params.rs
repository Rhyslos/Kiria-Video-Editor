//! Three ways for settings to reach the real-time thread without locks.
//!
//! - S1 atomics: every number of every track is an atomic; a counter per
//!   track says something changed; the real-time thread reads the track's
//!   numbers again when its counter moved.
//! - S2 swap: the interface side builds a whole new strip (coefficients
//!   worked out, memory allocated) and sends it through a lock-free queue;
//!   the real-time thread carries the running state over, swaps it in and
//!   sends the old one back through a second queue to be freed elsewhere.
//! - S3 commands: small plain messages (track, number, value) through one
//!   lock-free queue; the real-time thread applies them at the start of a
//!   block and works out again only what changed.
//!
//! All three use design C's strips (boxed, so S2 can swap them).

use crate::graph::{Graph, Parts, Strip};
use crate::settings::{P, TrackSettings};
use crate::signal::RATE;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub type G = Graph<Box<Strip>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Atomics,
    Swap,
    Commands,
}

impl Way {
    pub const ALL: [Way; 3] = [Way::Atomics, Way::Swap, Way::Commands];
    pub fn name(self) -> &'static str {
        match self {
            Way::Atomics => "S1 atomics per number",
            Way::Swap => "S2 swap pre-built strips",
            Way::Commands => "S3 command queue",
        }
    }
}

/// The interface's end.
pub trait Sender: Send {
    fn change(&mut self, track: usize, param: usize, value: f32);
    /// Frees what the real-time thread sent back (S2 only).
    fn collect(&mut self) {}
}

/// The real-time thread's end: applies whatever arrived. Never waits or
/// allocates.
pub trait Receiver: Send {
    fn poll(&mut self, g: &mut G);
}

// S1

struct TrackAtomics {
    changed: AtomicU32,
    values: [AtomicU32; P],
}

pub struct AtomicSender {
    shared: Arc<Vec<TrackAtomics>>,
}

pub struct AtomicReceiver {
    shared: Arc<Vec<TrackAtomics>>,
    seen: Vec<u32>,
}

impl Sender for AtomicSender {
    fn change(&mut self, track: usize, param: usize, value: f32) {
        let t = &self.shared[track];
        t.values[param].store(value.to_bits(), Ordering::Relaxed);
        t.changed.fetch_add(1, Ordering::Release);
    }
}

impl Receiver for AtomicReceiver {
    fn poll(&mut self, g: &mut G) {
        for (i, t) in self.shared.iter().enumerate() {
            let c = t.changed.load(Ordering::Acquire);
            if c != self.seen[i] {
                self.seen[i] = c;
                let mut a = [0f32; P];
                for (v, s) in a.iter_mut().zip(&t.values) {
                    *v = f32::from_bits(s.load(Ordering::Relaxed));
                }
                g.tracks[i].chain.set(&TrackSettings::from_array(&a));
            }
        }
    }
}

// S2

pub struct SwapSender {
    settings: Vec<[f32; P]>,
    parts: Vec<(Vec<f32>, crate::dsp::fader::Fader)>,
    to: rtrb::Producer<(usize, Box<Strip>)>,
    back: rtrb::Consumer<Box<Strip>>,
}

pub struct SwapReceiver {
    from: rtrb::Consumer<(usize, Box<Strip>)>,
    back: rtrb::Producer<Box<Strip>>,
}

impl Sender for SwapSender {
    fn change(&mut self, track: usize, param: usize, value: f32) {
        self.settings[track][param] = value;
        let s = TrackSettings::from_array(&self.settings[track]);
        let (profile, fader) = &self.parts[track];
        let strip = Box::new(Strip::new(RATE, &s, profile, fader.clone(), None));
        // A full queue would mean the real-time thread is stuck; the
        // newest settings are what matter, so in the app the sender keeps
        // the latest and retries. Here the queue is big enough.
        let _ = self.to.push((track, strip));
    }

    fn collect(&mut self) {
        while let Ok(old) = self.back.pop() {
            drop(old);
        }
    }
}

impl Receiver for SwapReceiver {
    fn poll(&mut self, g: &mut G) {
        // Only take a new strip when the old one can be sent back: the
        // real-time thread never frees.
        while self.back.slots() > 0 {
            let Ok((t, mut strip)) = self.from.pop() else {
                break;
            };
            let chain = &mut g.tracks[t].chain;
            strip.take_state(chain);
            std::mem::swap(chain, &mut strip);
            let _ = self.back.push(strip);
        }
    }
}

// S3

#[derive(Debug, Clone, Copy)]
pub struct Command {
    track: u16,
    param: u16,
    value: f32,
}

pub struct CommandSender {
    to: rtrb::Producer<Command>,
}

pub struct CommandReceiver {
    from: rtrb::Consumer<Command>,
    settings: Vec<[f32; P]>,
    dirty: Vec<bool>,
}

impl Sender for CommandSender {
    fn change(&mut self, track: usize, param: usize, value: f32) {
        let _ = self.to.push(Command {
            track: track as u16,
            param: param as u16,
            value,
        });
    }
}

impl Receiver for CommandReceiver {
    fn poll(&mut self, g: &mut G) {
        while let Ok(c) = self.from.pop() {
            self.settings[c.track as usize][c.param as usize] = c.value;
            self.dirty[c.track as usize] = true;
        }
        for (t, d) in self.dirty.iter_mut().enumerate() {
            if *d {
                *d = false;
                g.tracks[t]
                    .chain
                    .set(&TrackSettings::from_array(&self.settings[t]));
            }
        }
    }
}

/// Both ends of one way, for tracks starting with `settings` and `parts`.
pub fn make(way: Way, settings: &[TrackSettings], parts: &[Parts]) -> (Box<dyn Sender>, Box<dyn Receiver>) {
    let arrays: Vec<[f32; P]> = settings.iter().map(|s| s.to_array()).collect();
    match way {
        Way::Atomics => {
            let shared: Arc<Vec<TrackAtomics>> = Arc::new(
                arrays
                    .iter()
                    .map(|a| TrackAtomics {
                        changed: AtomicU32::new(0),
                        values: a.map(|v| AtomicU32::new(v.to_bits())),
                    })
                    .collect(),
            );
            let n = shared.len();
            (
                Box::new(AtomicSender {
                    shared: Arc::clone(&shared),
                }),
                Box::new(AtomicReceiver {
                    shared,
                    seen: vec![0; n],
                }),
            )
        }
        Way::Swap => {
            let (to, from) = rtrb::RingBuffer::new(64);
            let (back_to, back) = rtrb::RingBuffer::new(64);
            (
                Box::new(SwapSender {
                    settings: arrays,
                    parts: parts
                        .iter()
                        .map(|p| (p.profile.clone(), p.fader.clone()))
                        .collect(),
                    to,
                    back,
                }),
                Box::new(SwapReceiver { from, back: back_to }),
            )
        }
        Way::Commands => {
            let (to, from) = rtrb::RingBuffer::new(1024);
            let n = arrays.len();
            (
                Box::new(CommandSender { to }),
                Box::new(CommandReceiver {
                    from,
                    settings: arrays,
                    dirty: vec![false; n],
                }),
            )
        }
    }
}
