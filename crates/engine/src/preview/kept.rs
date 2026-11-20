//! The frames a worker keeps: on the graphics card, and below that in the
//! computer's memory.
//!
//! Frames decoded on the window's device are kept there ([`FrameCache`],
//! within the memory for playback). When the card's share is full, the
//! frames that make room (those furthest from the playhead, behind it
//! first while playing) are read back into memory, within the memory for
//! frames, instead of being thrown away: going back to them, or playing on
//! into them, then costs an upload instead of a decode. A frame found in
//! memory is handed out as planes, as one decoded on the processor is.
//! Reading one back takes a millisecond or two; a frame the viewer is
//! showing is never read (it simply goes).

use crate::frames::{FrameCache, Gone, Span, stretch_end, stretch_start};
use media::{Decoded, GpuFrame, Reader, WindowDevice, YuvFrame};
use project::MediaId;
use std::sync::Arc;

type Key = (MediaId, u32);

/// What the viewer's corner says for a picture kept in memory.
pub(super) const KEPT_IN_MEMORY: &str = "kept in memory";

/// Frames kept on the card and in memory.
pub(super) struct Kept {
    card: FrameCache<Key, GpuFrame>,
    memory: FrameCache<Key, Arc<YuvFrame>>,
    /// The window's device, which frames are read back from, and the
    /// reader, made the first time one is.
    device: Option<Arc<WindowDevice>>,
    reader: Option<Reader>,
    /// Reading back failed once: frames that make room just go from then on.
    no_reading: bool,
}

impl Kept {
    /// Keeps at most `card` bytes on `device` and `memory` bytes in memory.
    pub(super) fn new(card: u64, memory: u64, device: Option<Arc<WindowDevice>>) -> Kept {
        Kept {
            card: FrameCache::new(card),
            memory: FrameCache::new(memory),
            device,
            reader: None,
            no_reading: false,
        }
    }

    /// Follows changed budgets; what no longer fits on the card goes to memory.
    pub(super) fn set_budgets(&mut self, card: u64, memory: u64) {
        self.memory.set_budget(memory);
        let gone = self.card.set_budget(card);
        self.move_down(gone);
    }

    /// Where the playhead is: what stays longest, in both places.
    pub(super) fn look_at(&mut self, key: Key, moment: f64, playing: bool) {
        self.card.look_at(key, moment, playing);
        self.memory.look_at(key, moment, playing);
    }

    /// The frame on screen at `moment`, with what the viewer's corner says
    /// for it, from the card or (copied out) from memory.
    pub(super) fn get(&mut self, key: Key, moment: f64) -> Option<(Decoded, &'static str)> {
        if let Some(frame) = self.card.get(key, moment) {
            return Some((Decoded::Gpu(frame), super::KEPT));
        }
        let frame = self.memory.get(key, moment)?;
        Some((Decoded::Yuv(frame.copied()), KEPT_IN_MEMORY))
    }

    /// Whether the frame on screen at `moment` is kept somewhere.
    pub(super) fn has(&self, key: Key, moment: f64) -> bool {
        let t = micros(moment);
        self.card.span_at(key, t).is_some() || self.memory.span_at(key, t).is_some()
    }

    /// Keeps a frame on the card, on screen until `until`; frames that make
    /// room there go to memory when there is room for them.
    pub(super) fn put(&mut self, key: Key, until: f64, tick: f64, frame: GpuFrame) {
        let (w, h) = frame.planes.size();
        let bytes = u64::from(w) * u64::from(h) * 3 / 2;
        // On the card now: a copy in memory would only take room.
        self.memory.remove(key, frame.time);
        let gone = self.card.put(key, frame.time, until, tick, bytes, frame);
        self.move_down(gone);
    }

    /// Whether a frame at `moment` would stay if it were kept now, on the
    /// card or in memory.
    pub(super) fn would_keep(&self, key: Key, moment: f64) -> bool {
        self.card.would_keep(key, moment) || (self.reads_back() && self.memory.would_keep(key, moment))
    }

    /// Where the unbroken stretch kept (in either place) around `moment` begins.
    pub(super) fn stretch_start(&self, key: Key, moment: f64) -> Option<f64> {
        stretch_start(moment, |t| self.span_at(key, t), |t| self.span_ending(key, t))
    }

    /// Where the unbroken stretch kept (in either place) from `moment` ends.
    pub(super) fn stretch_end(&self, key: Key, moment: f64) -> Option<f64> {
        stretch_end(moment, |t| self.span_at(key, t))
    }

    fn span_at(&self, key: Key, t: i64) -> Option<Span> {
        self.card.span_at(key, t).or_else(|| self.memory.span_at(key, t))
    }

    fn span_ending(&self, key: Key, t: i64) -> Option<Span> {
        self.card
            .span_ending(key, t)
            .or_else(|| self.memory.span_ending(key, t))
    }

    /// Whether frames that make room on the card can go to memory.
    fn reads_back(&self) -> bool {
        self.memory.budget() > 0 && self.device.is_some() && !self.no_reading
    }

    /// Reads frames that made room on the card back into memory, those
    /// that would stay there; the rest just go.
    fn move_down(&mut self, gone: Vec<Gone<Key, GpuFrame>>) {
        for gone in gone {
            if !self.reads_back() || !self.memory.would_keep(gone.file, gone.time) {
                continue;
            }
            // Being shown: not read while the window may draw it.
            if Arc::strong_count(&gone.picture.planes) != 1 {
                continue;
            }
            let Some(device) = &self.device else {
                return;
            };
            let reader = match self.reader.take().map_or_else(|| device.reader(), Ok) {
                Ok(reader) => self.reader.insert(reader),
                Err(err) => {
                    tracing::info!("frames are not kept in memory: {}", err.report());
                    self.no_reading = true;
                    return;
                }
            };
            match device.read_back(reader, gone.picture) {
                Ok(frame) => {
                    let bytes = frame.bytes();
                    // Whatever makes room in memory just goes.
                    let _ = self.memory.put(
                        gone.file,
                        gone.time,
                        gone.until,
                        gone.tick,
                        bytes,
                        Arc::new(frame),
                    );
                }
                Err(err) => {
                    tracing::info!("frames are no longer kept in memory: {}", err.report());
                    self.no_reading = true;
                    self.reader = None;
                    return;
                }
            }
        }
    }

    /// Frames kept on the card and in memory, for tests.
    #[cfg(test)]
    pub(super) fn counts(&self) -> (usize, usize) {
        (self.card.len(), self.memory.len())
    }
}

fn micros(seconds: f64) -> i64 {
    (seconds * 1_000_000.0).round() as i64
}
