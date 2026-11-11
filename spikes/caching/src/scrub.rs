//! The preview worker, modelled: one worker answers a trace of playhead
//! positions with pictures, using one of the approaches. Only the newest
//! request counts, as in the editor.
//!
//! The clock is simulated. The work (seeking, decoding, scaling) is real and
//! timed; waiting is skipped. So a run takes as long as its work, and the
//! result doesn't depend on sleeping accurately.

use crate::cache::Cache;
use crate::decoder::{Decoder, KeyIndex, Pixels, Result, Stamp, picture_bytes};
use crate::trace::Request;
use std::time::Instant;

/// What goes on screen first after a move, before the exact frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum First {
    Nothing,
    /// Approach A: the key frame before the target, from the original file.
    KeyFrame,
    /// Approach D: the nearest small picture from the picture index.
    Index,
    /// Approach C: the same frame from the proxy.
    Proxy,
}

pub struct Strategy {
    pub name: &'static str,
    pub first: First,
    /// Approach B: keep every decoded frame in memory and fill around the playhead when idle.
    pub cache: bool,
    /// Seek by the key frame index, drop a decode a newer request can't use,
    /// and show frames passed on the way to a far target.
    pub smart: bool,
    /// Follow the first picture with the exact frame from the original.
    pub refine: bool,
}

const fn strategy(name: &'static str, first: First, cache: bool, smart: bool, refine: bool) -> Strategy {
    Strategy {
        name,
        first,
        cache,
        smart,
        refine,
    }
}

pub const STRATEGIES: [Strategy; 9] = [
    strategy("today", First::Nothing, false, false, true),
    strategy("cancelling only", First::Nothing, false, true, true),
    strategy("A key frame first", First::KeyFrame, false, true, true),
    strategy("B memory", First::Nothing, true, true, true),
    strategy("A+B", First::KeyFrame, true, true, true),
    strategy("C proxy first", First::Proxy, false, true, true),
    strategy("B+C", First::Proxy, true, true, true),
    strategy("C proxy only", First::Proxy, false, true, false),
    strategy("B+D", First::Index, true, true, true),
];

/// The editor's rule today: decode on when the target is this close ahead, else seek.
const DECODE_ON_LIMIT: f64 = 3.0;
/// While racing to a far target, put a frame passed on the way on screen this often.
const PROGRESS_EVERY: f64 = 1.0 / 30.0;
/// Seconds kept filled around the playhead: in the direction of travel, and behind it.
const FILL_AHEAD: f64 = 1.0;
const FILL_BEHIND: f64 = 0.5;
/// The playhead counts as resting when no request arrived for this long.
const RESTING_AFTER: f64 = 0.25;
/// Stops a run whose cache can never be filled (a broken file).
const MOST_IDLE_STEPS: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The right frame, from the original file.
    Exact,
    /// The right frame, from the proxy (smaller).
    Proxy,
    /// Another frame: a key frame, an index picture, or one passed on the way.
    Rough,
}

/// A picture that went on screen.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    /// When, on the trace's clock.
    pub at: f64,
    /// The request it answers.
    pub request: usize,
    pub kind: Kind,
    /// It was in memory already: nothing was decoded for it.
    pub from_memory: bool,
}

struct Screen {
    shown: Vec<Shown>,
    /// Start time of the frame on screen.
    time: Option<f64>,
}

impl Screen {
    fn show(&mut self, at: f64, request: usize, stamp: Stamp, kind: Kind) {
        self.time = Some(stamp.time);
        self.shown.push(Shown {
            at,
            request,
            kind,
            from_memory: false,
        });
    }

    /// Shows the exact frame, found in the cache.
    fn show_from_memory(&mut self, at: f64, request: usize, stamp: Stamp) {
        self.time = Some(stamp.time);
        self.shown.push(Shown {
            at,
            request,
            kind: Kind::Exact,
            from_memory: true,
        });
    }

    /// Would a frame starting at `candidate` be nearer to `target` than what is on screen?
    fn closer(&self, target: f64, candidate: f64) -> bool {
        self.time
            .is_none_or(|on_screen| (target - candidate).abs() + 1e-4 < (target - on_screen).abs())
    }
}

/// A proxy or a picture index: a small file that is cheap to fetch from.
pub struct Small {
    pub decoder: Decoder,
    /// Every frame is a key frame, so a fetch decodes exactly one.
    pub all_keys: bool,
}

impl Small {
    /// Decodes the frame on screen at `t` (the nearest one before it in a picture index).
    pub fn fetch(&mut self, t: f64) -> Result<Option<Stamp>> {
        if self.all_keys {
            self.decoder.key_frame_at(t)
        } else {
            self.decoder.frame_at(t)
        }
    }
}

/// Everything a run decodes from.
pub struct Rig<'a> {
    /// The original file: exact frames.
    pub main: Decoder,
    /// A second decoder on the original for approach A's key frames, so the
    /// main one keeps its place.
    pub peek: Decoder,
    pub proxy: Option<Small>,
    pub index: Option<Small>,
    pub keys: &'a KeyIndex,
    /// Pictures are fitted into this.
    pub size: (u32, u32),
    pub cache: Cache,
    pub cache_budget: usize,
    /// Seconds of the clip in use.
    pub span: f64,
    /// Where the main decoder's next frame starts.
    at: f64,
    /// The frame decoded just before the last one, when they follow each other.
    previous: Option<Stamp>,
}

impl<'a> Rig<'a> {
    pub fn new(
        main: Decoder,
        peek: Decoder,
        keys: &'a KeyIndex,
        size: (u32, u32),
        cache_budget: usize,
        span: f64,
    ) -> Rig<'a> {
        Rig {
            main,
            peek,
            proxy: None,
            index: None,
            keys,
            size,
            cache: Cache::new(cache_budget),
            cache_budget,
            span,
            at: 0.0,
            previous: None,
        }
    }

    /// Every run starts the same: nothing cached, the decoder at the start of the file.
    pub fn reset(&mut self) -> Result<()> {
        self.cache.clear();
        self.main.seek(0.0)?;
        self.at = 0.0;
        self.previous = None;
        Ok(())
    }

    /// Gets the decoder ready to decode on to `target`: seeks when it can't
    /// get there from where it is (`smart`: judged by the key frame index;
    /// else by the editor's rule today). Decided once per target, as the
    /// editor does: after a seek the decoder only moves forward.
    pub fn aim(&mut self, target: f64, smart: bool) -> Result<()> {
        let seek = if smart {
            target < self.at - 1e-6 || self.keys.before(target) > self.at + 1e-6
        } else {
            let held = self.main.last.map_or(0.0, |l| l.time);
            target < held || target - held > DECODE_ON_LIMIT
        };
        if seek {
            self.main.seek(target)?;
            self.at = self.keys.before(target);
        }
        Ok(())
    }

    /// Decodes the next frame.
    pub fn step(&mut self) -> Result<Option<Stamp>> {
        self.previous = self.main.last;
        let stamp = self.main.next()?;
        if let Some(s) = stamp {
            self.at = s.end;
        }
        Ok(stamp)
    }

    /// A jump: decodes to the frame on screen at `target` and copies it out.
    pub fn exact(&mut self, target: f64, smart: bool) -> Result<()> {
        if !self.main.last.is_some_and(|l| l.covers(target)) {
            self.aim(target, smart)?;
            while let Some(stamp) = self.step()? {
                if target < stamp.end {
                    break;
                }
            }
        }
        self.main.picture(self.size, Pixels::Yuv420).map(|_| ())
    }

    /// Copies the frame just decoded into the cache, unless it is there already.
    fn keep(&mut self) -> Result<()> {
        let Some(stamp) = self.main.last else {
            return Ok(());
        };
        if let Some(p) = self.previous.filter(|p| p.time < stamp.time) {
            self.cache.set_end(p.time, stamp.time);
        }
        if self.cache.covered_until(stamp.time + 1e-6).is_none() {
            let picture = self.main.picture(self.size, Pixels::Yuv420)?;
            self.cache.insert(picture);
        }
        Ok(())
    }

    /// The stretch of the clip kept in memory around the playhead at
    /// `around`: (from, to). While the playhead moves, only the side it
    /// moves towards; once it rests, a little of the other side too.
    ///
    /// Moving backwards it reaches back to the start of the group of
    /// pictures before this one, as far as the budget allows: decoding only
    /// runs forwards, so the frames behind have to be there before they are needed.
    fn window(&self, around: f64, backward: bool, moving: bool) -> (f64, f64) {
        let (w, h) = crate::decoder::fit(self.main.width, self.main.height, self.size);
        let per_second = picture_bytes(w, h, Pixels::Yuv420) as f64 * self.main.fps;
        let room = 0.4 * self.cache_budget as f64 / per_second;
        let other_side = if moving { 0.0 } else { FILL_BEHIND.min(room) };
        let (before, after) = if backward {
            let group = self.keys.before(around);
            let group_before = self.keys.before((group - 1e-3).max(0.0));
            ((around - group_before).min(room), other_side)
        } else {
            (other_side, FILL_AHEAD.min(room))
        };
        ((around - before).max(0.0), (around + after).min(self.span))
    }

    /// The first time from `from` up to `to` with no picture in the cache.
    fn hole(&self, from: f64, to: f64) -> Option<f64> {
        let mut t = from;
        while t < to {
            match self.cache.covered_until(t) {
                Some(end) => t = end + 1e-6,
                None => return Some(t),
            }
        }
        None
    }

    /// Idle work: decodes one more frame of the window around the playhead
    /// into the cache, the side ahead of the playhead first. Returns false
    /// when there is nothing left to do.
    fn fill_one(&mut self, around: f64, backward: bool, moving: bool) -> Result<bool> {
        let (from, to) = self.window(around, backward, moving);
        self.cache.protect(from, to);
        let hole = if backward {
            self.hole(from, to)
        } else {
            self.hole(around, to).or_else(|| self.hole(from, around))
        };
        let Some(hole) = hole else { return Ok(false) };
        self.aim(hole, true)?;
        let Some(stamp) = self.step()? else {
            return Ok(false);
        };
        self.keep()?;
        // A frame that starts after the hole means no frame covers it: give up.
        Ok(stamp.time <= hole)
    }
}

fn newest(trace: &[Request], now: f64) -> Option<usize> {
    trace.partition_point(|r| r.at <= now).checked_sub(1)
}

/// Replays `trace` with `strategy`. Returns the pictures shown, in order.
pub fn run(rig: &mut Rig, strategy: &Strategy, trace: &[Request]) -> Result<Vec<Shown>> {
    rig.reset()?;
    let mut clock = 0.0;
    let mut answered: Option<usize> = None;
    let mut screen = Screen {
        shown: Vec::new(),
        time: None,
    };
    let mut idle_steps = 0;
    loop {
        let latest = newest(trace, clock);
        let started = Instant::now();
        match latest.filter(|n| answered.is_none_or(|a| a < *n)) {
            Some(n) => {
                if serve(rig, strategy, trace, n, clock, &mut screen)? {
                    answered = Some(n);
                }
            }
            None => {
                let next = trace.get(latest.map_or(0, |n| n + 1)).map(|r| r.at);
                let worked = match latest {
                    Some(n) if strategy.cache && idle_steps < MOST_IDLE_STEPS => {
                        idle_steps += 1;
                        let backward = n > 0 && trace[n].pos < trace[n - 1].pos;
                        let moving = clock - trace[n].at < RESTING_AFTER;
                        rig.fill_one(trace[n].pos, backward, moving)?
                    }
                    _ => false,
                };
                if !worked {
                    match next {
                        Some(at) => {
                            clock = clock.max(at);
                            continue;
                        }
                        None => break,
                    }
                }
            }
        }
        clock += started.elapsed().as_secs_f64();
    }
    Ok(screen.shown)
}

/// Answers request `n`. Returns false when it gave up because a newer
/// request arrived; the decoder keeps its place for the next one.
fn serve(
    rig: &mut Rig,
    strategy: &Strategy,
    trace: &[Request],
    n: usize,
    clock: f64,
    screen: &mut Screen,
) -> Result<bool> {
    let started = Instant::now();
    let now = || clock + started.elapsed().as_secs_f64();
    let target = trace[n].pos;
    let size = rig.size;

    // The exact frame is in memory already.
    if strategy.cache {
        let backward = n > 0 && target < trace[n - 1].pos;
        let (from, to) = rig.window(target, backward, true);
        rig.cache.protect(from, to);
        if let Some(stamp) = rig.cache.get(target).map(|p| p.stamp) {
            screen.show_from_memory(now(), n, stamp);
            return Ok(true);
        }
    }
    if let Some(held) = rig.main.last.filter(|l| l.covers(target)) {
        rig.main.picture(size, Pixels::Yuv420)?;
        screen.show(now(), n, held, Kind::Exact);
        return Ok(true);
    }

    // Something to look at straight away.
    match strategy.first {
        First::Nothing => {}
        First::Proxy => {
            if let Some(proxy) = rig.proxy.as_mut()
                && let Some(stamp) = proxy.fetch(target)?
            {
                proxy.decoder.picture(size, Pixels::Yuv420)?;
                // Timestamps in the proxy's container can be rounded by up to a millisecond.
                let right = stamp.time - 2e-3 <= target && target < stamp.end + 2e-3;
                screen.show(now(), n, stamp, if right { Kind::Proxy } else { Kind::Rough });
            }
        }
        First::Index => {
            if let Some(index) = rig.index.as_mut()
                && let Some(stamp) = index.fetch(target)?
                && screen.closer(target, stamp.time)
            {
                index.decoder.picture(size, Pixels::Yuv420)?;
                screen.show(now(), n, stamp, Kind::Rough);
            }
        }
        First::KeyFrame => {
            let key = rig.keys.before(target);
            let cached = rig.cache.nearest_before(target).filter(|t| *t > key);
            if let Some(time) = cached {
                // A frame in memory is nearer than the key frame, and free.
                if screen.closer(target, time)
                    && let Some(stamp) = rig.cache.get(time + 1e-6).map(|p| p.stamp)
                {
                    screen.show(now(), n, stamp, Kind::Rough);
                }
            } else if screen.closer(target, key)
                && let Some(stamp) = rig.peek.key_frame_at(target)?
            {
                rig.peek.picture(size, Pixels::Yuv420)?;
                screen.show(now(), n, stamp, Kind::Rough);
            }
        }
    }
    if !strategy.refine {
        return Ok(true);
    }

    // The exact frame from the original.
    rig.aim(target, strategy.smart)?;
    let mut progress_at = now();
    loop {
        let Some(stamp) = rig.step()? else {
            return Ok(true); // the end of the file: nothing more to show
        };
        if strategy.cache {
            rig.keep()?;
        }
        if target < stamp.end {
            if !strategy.cache {
                rig.main.picture(size, Pixels::Yuv420)?;
            }
            screen.show(now(), n, stamp, Kind::Exact);
            return Ok(true);
        }
        if strategy.smart {
            if now() - progress_at >= PROGRESS_EVERY && screen.closer(target, stamp.time) {
                if !strategy.cache {
                    rig.main.picture(size, Pixels::Yuv420)?;
                }
                screen.show(now(), n, stamp, Kind::Rough);
                progress_at = now();
            }
            if newest(trace, now()) != Some(n) {
                return Ok(false);
            }
        }
    }
}

/// What a run looked like from the user's side.
#[derive(Debug, Default)]
pub struct Summary {
    pub pictures_per_second: f64,
    /// Milliseconds from a request to the first picture for it or a newer one.
    pub response_median: f64,
    pub response_worst: f64,
    pub within_50ms_percent: f64,
    /// Share of requests answered first with the right frame (original or proxy).
    pub right_frame_percent: f64,
    /// Share answered first with the right frame from the original.
    pub exact_percent: f64,
    /// Share answered first with the exact frame straight from memory (approach B's hit rate).
    pub from_memory_percent: f64,
    /// Milliseconds from the last request until its final picture.
    pub settle: f64,
}

pub fn median(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else {
        sorted[sorted.len() / 2]
    }
}

pub fn summarise(trace: &[Request], shown: &[Shown], refine: bool) -> Summary {
    let Some(last) = trace.last() else {
        return Summary::default();
    };
    let end = shown.last().map_or(last.at, |s| s.at.max(last.at));
    let mut responses = Vec::with_capacity(trace.len());
    let (mut right, mut exact, mut hits) = (0usize, 0usize, 0usize);
    let mut from = 0;
    for (i, request) in trace.iter().enumerate() {
        while from < shown.len() && shown[from].request < i {
            from += 1;
        }
        match shown.get(from) {
            Some(s) => {
                responses.push((s.at - request.at).max(0.0) * 1000.0);
                right += usize::from(s.kind != Kind::Rough);
                exact += usize::from(s.kind == Kind::Exact);
                hits += usize::from(s.from_memory);
            }
            None => responses.push((end - request.at) * 1000.0),
        }
    }
    let within = responses.iter().filter(|r| **r <= 50.0).count();
    let final_kind = if refine { Kind::Exact } else { Kind::Proxy };
    let settle = shown
        .iter()
        .find(|s| s.request == trace.len() - 1 && (s.kind == final_kind || s.kind == Kind::Exact))
        .map_or(f64::NAN, |s| (s.at - last.at) * 1000.0);
    responses.sort_by(f64::total_cmp);
    let percent = |n: usize| 100.0 * n as f64 / trace.len() as f64;
    let seconds = (end - trace[0].at).max(1e-3);
    Summary {
        pictures_per_second: shown.len() as f64 / seconds,
        response_median: median(&responses),
        response_worst: responses.last().copied().unwrap_or(0.0),
        within_50ms_percent: percent(within),
        right_frame_percent: percent(right),
        exact_percent: percent(exact),
        from_memory_percent: percent(hits),
        settle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(at: f64) -> Request {
        Request { at, pos: at }
    }

    fn shown(at: f64, request: usize, kind: Kind) -> Shown {
        Shown {
            at,
            request,
            kind,
            from_memory: false,
        }
    }

    #[test]
    fn a_request_is_answered_by_the_first_picture_for_it_or_a_newer_one() {
        let trace = [request(0.0), request(0.1), request(0.2)];
        // Request 0 gets a rough picture after 30 ms; request 1 is never
        // answered itself; request 2's exact frame arrives at 0.26 s.
        let pictures = [shown(0.03, 0, Kind::Rough), shown(0.26, 2, Kind::Exact)];
        let s = summarise(&trace, &pictures, true);
        assert!((s.response_median - 60.0).abs() < 1e-6, "{s:?}");
        assert!((s.response_worst - 160.0).abs() < 1e-6, "{s:?}");
        assert!((s.within_50ms_percent - 100.0 / 3.0).abs() < 1e-6);
        assert!((s.exact_percent - 200.0 / 3.0).abs() < 1e-6);
        assert!((s.settle - 60.0).abs() < 1e-6);
        assert_eq!(s.from_memory_percent, 0.0);
    }

    #[test]
    fn a_hit_is_a_request_answered_from_memory() {
        let trace = [request(0.0), request(0.1)];
        let mut hit = shown(0.1, 1, Kind::Exact);
        hit.from_memory = true;
        let s = summarise(&trace, &[shown(0.04, 0, Kind::Exact), hit], true);
        assert_eq!(s.from_memory_percent, 50.0);
        assert_eq!(s.exact_percent, 100.0);
    }

    #[test]
    fn a_proxy_picture_is_the_right_frame_but_not_the_exact_one() {
        let trace = [request(0.0)];
        let s = summarise(&trace, &[shown(0.004, 0, Kind::Proxy)], false);
        assert_eq!(s.right_frame_percent, 100.0);
        assert_eq!(s.exact_percent, 0.0);
        assert!((s.settle - 4.0).abs() < 1e-6);
    }

    #[test]
    fn newest_request_by_time() {
        let trace = [request(0.0), request(0.1)];
        assert_eq!(newest(&trace, -0.1), None);
        assert_eq!(newest(&trace, 0.05), Some(0));
        assert_eq!(newest(&trace, 0.1), Some(1));
    }
}
