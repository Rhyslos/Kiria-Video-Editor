//! A clip's speed changing over time: speed ramps.
//!
//! A ramp is a list of speed keyframes ([`SpeedKey`]) at moments of the
//! clip on the timeline, counted from the clip's start. Before the first
//! keyframe the clip plays at the first one's speed, after the last at the
//! last one's, and in between the speed goes from one to the next as the
//! earlier keyframe's [`Ease`] says: evenly (linear), gently at both ends
//! (smooth), or not at all until it jumps (hold).
//!
//! Where in its file a clip is at a moment is the speed added up from the
//! clip's start to that moment. Each stretch of the ramp is added up in
//! whole numbers and brought to the nearest *flick* (1/705 600 000 of a
//! second): every common frame rate (23.976 to 240, the NTSC ones included)
//! and sound rate has a whole number of flicks a frame, so a clip that plays
//! a 120 fps recording at half speed on a 60 fps timeline lands exactly on
//! each recorded frame, ramp or not, and the viewer and an export always
//! agree. No floats are involved; the `_f64` functions are for drawing and
//! for sound, which works in seconds as floats anyway.
//!
//! The keyframes are kept in timeline time because that is what a ramp is
//! about: "slow down here, for a second". A clip keeps its whole ramp
//! through trims, splits and slides (the keyframes are moved with the
//! picture, so what is on screen at each moment stays), and keyframes
//! outside what the clip shows still shape it, as look keyframes do.

use crate::keyframes::Ease;
use crate::time::{Rate, Speed, Time};
use serde::{Deserialize, Serialize};

/// Flicks in a second: the unit ramps are added up in.
pub const FLICKS: i64 = 705_600_000;

/// The largest numerator or denominator a ramp's speed may have: room
/// for every speed the interface offers (whole thousandths), and small
/// enough that adding up a ramp never overflows.
pub const MOST_PARTS: u32 = 10_000;

/// One whole in the fixed-point shares used in between keyframes.
const SHARE_ONE: i128 = 1 << 32;

/// The speed at a moment of a clip on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpeedKey {
    /// From the clip's start, on the timeline (may be before it or after
    /// its end: such a keyframe still shapes the ramp).
    pub at: Time,
    pub speed: Speed,
    /// How the speed goes from here to the next keyframe.
    #[serde(default)]
    pub ease: Ease,
}

/// A clip's speed keyframes, in time order, never two at one moment. Empty
/// for a clip that plays at one speed (its `speed`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ramp(Vec<SpeedKey>);

impl Ramp {
    /// A ramp of `keys`, put in time order. Two at one moment: the later in
    /// the list wins.
    pub fn new(keys: impl IntoIterator<Item = SpeedKey>) -> Ramp {
        let mut ramp = Ramp::default();
        for key in keys {
            ramp.put(key);
        }
        ramp
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> &[SpeedKey] {
        &self.0
    }

    /// Adds `key`, replacing one at the same moment.
    fn put(&mut self, key: SpeedKey) {
        match self.0.binary_search_by(|k| k.at.cmp(&key.at)) {
            Ok(i) => self.0[i] = key,
            Err(i) => self.0.insert(i, key),
        }
    }

    /// The same ramp with every keyframe `by` later (earlier when negative):
    /// what a clip whose start moved by `-by` without moving its picture needs.
    pub fn shifted(&self, by: Time) -> Ramp {
        Ramp(self.0.iter().map(|k| SpeedKey { at: k.at + by, ..*k }).collect())
    }

    /// Whether the speed changes anywhere: two keyframes with different speeds.
    pub fn changes(&self) -> bool {
        self.0.windows(2).any(|pair| pair[0].speed != pair[1].speed)
    }

    /// Sets the speed of the keyframe in `near` (from the clip's start,
    /// end excluded) to `speed`, or adds one at `at` when there is none
    /// there; a new one takes the ease of the stretch it lands in.
    pub fn set(&mut self, at: Time, near: (Time, Time), speed: Speed) {
        if let Some(key) = self.0.iter_mut().find(|k| near.0 <= k.at && k.at < near.1) {
            key.speed = speed;
            return;
        }
        let index = self.0.partition_point(|k| k.at < at);
        let ease = index
            .checked_sub(1)
            .and_then(|i| self.0.get(i))
            .map_or(Ease::Linear, |before| before.ease);
        self.0.insert(index, SpeedKey { at, speed, ease });
    }

    /// Removes the keyframes in `near` (end excluded). Returns whether any was.
    pub fn remove(&mut self, near: (Time, Time)) -> bool {
        let before = self.0.len();
        self.0.retain(|k| !(near.0 <= k.at && k.at < near.1));
        self.0.len() != before
    }

    /// Sets the ease of the keyframes in `near` (end excluded). Returns
    /// whether there was one.
    pub fn set_ease(&mut self, near: (Time, Time), ease: Ease) -> bool {
        let mut found = false;
        for key in self.0.iter_mut().filter(|k| near.0 <= k.at && k.at < near.1) {
            key.ease = ease;
            found = true;
        }
        found
    }

    /// The speed at `at` (from the clip's start). Between two keyframes of
    /// different speeds it is brought to the nearest thousandth. `None`
    /// without keyframes.
    pub fn speed_at(&self, at: Time) -> Option<Speed> {
        let (before, after) = self.around(flicks(at))?;
        let Some((after, x)) = after else {
            return Some(before.speed);
        };
        let (n0, d0) = parts(before.speed);
        let (n1, d1) = parts(after.speed);
        let u = share(x - flicks(before.at), flicks(after.at) - flicks(before.at));
        let shape = match before.ease {
            Ease::Hold => 0,
            Ease::Linear => u,
            Ease::Smooth => smooth(u),
        };
        // (n0/d0 + (n1/d1 - n0/d0) * shape) in thousandths.
        let num = n0 * d1 * SHARE_ONE + (n1 * d0 - n0 * d1) * shape;
        let thousandths = div_round(num * 1000, d0 * d1 * SHARE_ONE).clamp(1, i128::from(u32::MAX));
        Speed::new(thousandths as u32, 1000).ok()
    }

    /// The speed at `at` seconds from the clip's start, for drawing.
    pub fn speed_at_f64(&self, at: f64) -> f64 {
        let Some(first) = self.0.first() else {
            return 1.0;
        };
        let next = self.0.partition_point(|k| k.at.as_secs_f64() <= at);
        let Some(before) = next.checked_sub(1).and_then(|i| self.0.get(i)) else {
            return first.speed.as_f64();
        };
        let Some(after) = self.0.get(next) else {
            return before.speed.as_f64();
        };
        let (a, b) = (before.at.as_secs_f64(), after.at.as_secs_f64());
        let u = ((at - a) / (b - a).max(1e-12)).clamp(0.0, 1.0);
        let shape = match before.ease {
            Ease::Hold => 0.0,
            Ease::Linear => u,
            Ease::Smooth => u * u * (3.0 - 2.0 * u),
        };
        before.speed.as_f64() + (after.speed.as_f64() - before.speed.as_f64()) * shape
    }

    /// How much of its file the clip plays from its start to `at` (from
    /// its start; negative before it), exactly to the flick. Zero without
    /// keyframes.
    pub fn source_offset(&self, at: Time) -> Time {
        if self.0.is_empty() {
            return Time::ZERO;
        }
        let played = self.added_up(flicks(at)) - self.added_up(0);
        Time::from_flicks(played)
    }

    /// [`Ramp::source_offset`] in seconds, for drawing and for sound.
    pub fn source_offset_f64(&self, at: f64) -> f64 {
        self.added_up_f64(at) - self.added_up_f64(0.0)
    }

    /// The keyframe at or before `x` flicks (the first when `x` is before
    /// it), and the one after it with `x`, when `x` is between two.
    fn around(&self, x: i128) -> Option<(&SpeedKey, Option<(&SpeedKey, i128)>)> {
        let first = self.0.first()?;
        let next = self.0.partition_point(|k| flicks(k.at) <= x);
        let Some(before) = next.checked_sub(1).and_then(|i| self.0.get(i)) else {
            return Some((first, None));
        };
        Some((before, self.0.get(next).map(|after| (after, x))))
    }

    /// The file played from the first keyframe to `x` flicks (negative
    /// before it), in flicks: whole stretches added up, then the part of
    /// the one `x` is in.
    fn added_up(&self, x: i128) -> i128 {
        let Some(first) = self.0.first() else {
            return 0;
        };
        let start = flicks(first.at);
        if x <= start {
            return constant(first.speed, x - start);
        }
        let mut total = 0;
        for pair in self.0.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let (from, to) = (flicks(a.at), flicks(b.at));
            if x <= to {
                return total + stretch(a, b, x - from, to - from);
            }
            total += stretch(a, b, to - from, to - from);
        }
        let last = self.0.last().unwrap_or(first);
        total + constant(last.speed, x - flicks(last.at))
    }

    /// [`Ramp::added_up`] in seconds, with floats.
    fn added_up_f64(&self, x: f64) -> f64 {
        let Some(first) = self.0.first() else {
            return 0.0;
        };
        let start = first.at.as_secs_f64();
        if x <= start {
            return first.speed.as_f64() * (x - start);
        }
        let mut total = 0.0;
        for pair in self.0.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let (from, to) = (a.at.as_secs_f64(), b.at.as_secs_f64());
            let length = (to - from).max(1e-12);
            let x = x.min(to) - from;
            let u = x / length;
            let (v0, v1) = (a.speed.as_f64(), b.speed.as_f64());
            let part = match a.ease {
                Ease::Hold => v0 * x,
                Ease::Linear => v0 * x + (v1 - v0) * length * u * u / 2.0,
                Ease::Smooth => v0 * x + (v1 - v0) * length * (u * u * u - u * u * u * u / 2.0),
            };
            total += part;
            if x + from < to {
                return total;
            }
        }
        let last = self.0.last().unwrap_or(first);
        total + last.speed.as_f64() * (x - last.at.as_secs_f64())
    }

    /// "Ramp 100→25→100%": the speeds the keyframes go through, in order.
    pub fn summary(&self) -> String {
        let mut speeds: Vec<String> = Vec::new();
        for key in &self.0 {
            let percent = key.speed.percent();
            let number = percent.trim_end_matches('%').to_owned();
            if speeds.last() != Some(&number) {
                speeds.push(number);
            }
        }
        match speeds.len() {
            0 => String::new(),
            1 => format!("{}%", speeds[0]),
            _ => format!("Ramp {}%", speeds.join("→")),
        }
    }

    /// What is wrong with this ramp, in plain words, each starting with `place`.
    pub fn problems(&self, place: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if self.0.windows(2).any(|pair| pair[0].at >= pair[1].at) {
            problems.push(format!("{place} has speed keyframes out of order"));
        }
        if self.0.iter().any(|k| !fits(k.speed)) {
            problems.push(format!(
                "{place} has a speed keyframe that is not a plain fraction"
            ));
        }
        if self
            .0
            .iter()
            .any(|k| flicks(k.at).unsigned_abs() > MOST_FLICKS as u128)
        {
            problems.push(format!("{place} has a speed keyframe too far from it"));
        }
        problems
    }
}

/// How far from its clip's start a speed keyframe may be: about four days,
/// which keeps every sum inside the numbers used.
const MOST_FLICKS: i128 = 1 << 58;

/// Whether `speed` can be a ramp's: a fraction of at most [`MOST_PARTS`].
pub fn fits(speed: Speed) -> bool {
    speed.num() <= MOST_PARTS && speed.den() <= MOST_PARTS
}

/// A ramp made for the inspector's ramp buttons: from `from` the clip eases
/// into `slow` over `ease_in` starting at `at` (from the clip's start), and
/// when `back_after` is given, stays there that long and eases back to
/// `from` over `ease_in` again. Both eases are smooth.
pub fn slow_motion(at: Time, from: Speed, slow: Speed, ease_in: Time, back_after: Option<Time>) -> Ramp {
    let mut keys = vec![
        SpeedKey {
            at,
            speed: from,
            ease: Ease::Smooth,
        },
        SpeedKey {
            at: at + ease_in,
            speed: slow,
            ease: Ease::Linear,
        },
    ];
    if let Some(hold) = back_after {
        let leave = at + ease_in + hold;
        keys.push(SpeedKey {
            at: leave,
            speed: slow,
            ease: Ease::Smooth,
        });
        keys.push(SpeedKey {
            at: leave + ease_in,
            speed: from,
            ease: Ease::Linear,
        });
    }
    Ramp::new(keys)
}

/// The clean slow motion for a recording of `source` frames a second on a
/// timeline of `timeline`: the speed that shows every recorded frame once
/// (half speed for 120 fps on a 60 fps timeline, 5/12 for 144 fps), or
/// half speed when the recording has no more frames than the timeline.
/// Never slower than a quarter.
pub fn clean_slow(source: Rate, timeline: Rate) -> Speed {
    let half = Speed::new(1, 2).unwrap_or_default();
    // timeline / source = (t.num * s.den) / (t.den * s.num).
    let num = u64::from(timeline.num()) * u64::from(source.den());
    let den = u64::from(timeline.den()) * u64::from(source.num());
    if num >= den {
        return half;
    }
    if num * 4 < den {
        return Speed::new(1, 4).unwrap_or(half);
    }
    let g = gcd(num, den);
    match (u32::try_from(num / g), u32::try_from(den / g)) {
        (Ok(n), Ok(d)) => Speed::new(n, d).ok().filter(|s| fits(*s)).unwrap_or(half),
        _ => half,
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl Time {
    /// `flicks` flicks ([`FLICKS`] a second).
    pub fn from_flicks(flicks: i128) -> Time {
        Time::from_i128(flicks, i128::from(FLICKS))
    }
}

/// `t` in flicks, to the nearest.
pub fn flicks(t: Time) -> i128 {
    let n = i128::from(t.num()).saturating_mul(i128::from(FLICKS));
    div_round(n, i128::from(t.den())).clamp(-MOST_FLICKS * 4, MOST_FLICKS * 4)
}

/// `a / b` to the nearest whole number (halves up). `b` is positive.
fn div_round(a: i128, b: i128) -> i128 {
    (a.saturating_mul(2).saturating_add(b)).div_euclid(b.saturating_mul(2).max(1))
}

/// A speed's numerator and denominator, wide.
fn parts(speed: Speed) -> (i128, i128) {
    (i128::from(speed.num()), i128::from(speed.den()))
}

/// The file played in `length` flicks at `speed`, to the nearest flick.
fn constant(speed: Speed, length: i128) -> i128 {
    let (n, d) = parts(speed);
    div_round(n.saturating_mul(length), d)
}

/// The file played in the first `x` flicks of the stretch from `a` to `b`
/// (`length` flicks long), to the nearest flick. The speed is
/// `v0 + (v1 - v0) * shape(u)` with `u = x / length`; added up, that is
/// `v0 * x + (v1 - v0) * length * area(u)`, where `area` is `u² / 2` for
/// linear and `u³ - u⁴ / 2` for smooth. `u` is a fixed-point share, so the
/// whole stretch (`u` = 1) is exact: `(v0 + v1) / 2 * length`.
fn stretch(a: &SpeedKey, b: &SpeedKey, x: i128, length: i128) -> i128 {
    if a.ease == Ease::Hold || a.speed == b.speed || length <= 0 {
        return constant(a.speed, x);
    }
    let (n0, d0) = parts(a.speed);
    let (n1, d1) = parts(b.speed);
    let u = share(x, length);
    let area = match a.ease {
        Ease::Smooth => {
            let u2 = u * u / SHARE_ONE;
            let u3 = u2 * u / SHARE_ONE;
            let u4 = u3 * u / SHARE_ONE;
            u3 - u4 / 2
        }
        _ => u * u / SHARE_ONE / 2,
    };
    // Over the common denominator d0 * d1 * SHARE_ONE. With speeds of at
    // most MOST_PARTS parts and lengths within MOST_FLICKS, every product
    // stays far inside 128 bits.
    let steady = n0.saturating_mul(d1).saturating_mul(x).saturating_mul(SHARE_ONE);
    let change = (n1 * d0 - n0 * d1).saturating_mul(length).saturating_mul(area);
    div_round(steady.saturating_add(change), d0 * d1 * SHARE_ONE)
}

/// How far `x` is through `length`, in [`SHARE_ONE`]ths, rounding down.
fn share(x: i128, length: i128) -> i128 {
    if length <= 0 || x <= 0 {
        return 0;
    }
    if x >= length {
        return SHARE_ONE;
    }
    x.saturating_mul(SHARE_ONE) / length
}

/// Slow, faster, slow: 3u² - 2u³, in [`SHARE_ONE`]ths.
fn smooth(u: i128) -> i128 {
    let squared = u * u / SHARE_ONE;
    squared * (3 * SHARE_ONE - 2 * u) / SHARE_ONE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speed(num: u32, den: u32) -> Speed {
        Speed::new(num, den).unwrap()
    }

    fn key(at: Time, speed: Speed, ease: Ease) -> SpeedKey {
        SpeedKey { at, speed, ease }
    }

    fn frames(n: i64) -> Time {
        Rate::FPS_60.frame_start(n)
    }

    #[test]
    fn a_constant_ramp_lands_exactly_on_every_recorded_frame() {
        // Half speed on 60 fps: every timeline frame is one frame of 120 fps.
        let ramp = Ramp::new([key(Time::ZERO, speed(1, 2), Ease::Linear)]);
        for n in 0..600 {
            assert_eq!(ramp.source_offset(frames(n)), Rate::FPS_120.frame_start(n));
        }
        // 5/12 on 60 fps is 144 fps, one recorded frame a timeline frame.
        let ramp = Ramp::new([key(frames(-7), speed(5, 12), Ease::Smooth)]);
        for n in 0..600 {
            assert_eq!(ramp.source_offset(frames(n)), Rate::FPS_144.frame_start(n));
        }
        // NTSC too.
        let ramp = Ramp::new([key(Time::ZERO, speed(1, 2), Ease::Linear)]);
        assert_eq!(
            ramp.source_offset(Rate::NTSC_60.frame_start(1001)),
            Rate::new(120_000, 1001).unwrap().frame_start(1001)
        );
    }

    #[test]
    fn a_linear_ramp_plays_the_average_of_its_ends() {
        // From 100% to 50% over 2 s: 1.5 s of the file, then half speed.
        let ramp = Ramp::new([
            key(Time::ZERO, speed(1, 1), Ease::Linear),
            key(Time::seconds(2), speed(1, 2), Ease::Linear),
        ]);
        assert_eq!(
            ramp.source_offset(Time::seconds(2)),
            Time::from_ratio(3, 2).unwrap()
        );
        assert_eq!(
            ramp.source_offset(Time::seconds(4)),
            Time::from_ratio(5, 2).unwrap()
        );
        // Half way through the ramp: 1 - 0.25/2 ... v0 x + dv x² / (2 L)
        // = 1 - 0.5 / 4 = 0.875 s.
        assert_eq!(
            ramp.source_offset(Time::seconds(1)),
            Time::from_ratio(7, 8).unwrap()
        );
        // Before the first keyframe it goes on at the first speed, backwards.
        assert_eq!(ramp.source_offset(Time::seconds(-1)), Time::seconds(-1));
    }

    #[test]
    fn a_smooth_ramp_covers_as_much_as_a_linear_one_but_eases() {
        let linear = Ramp::new([
            key(Time::ZERO, speed(1, 1), Ease::Linear),
            key(Time::seconds(1), speed(1, 4), Ease::Linear),
        ]);
        let smooth = Ramp::new([
            key(Time::ZERO, speed(1, 1), Ease::Smooth),
            key(Time::seconds(1), speed(1, 4), Ease::Linear),
        ]);
        for s in [1, 2, 5] {
            assert_eq!(
                linear.source_offset(Time::seconds(s)),
                smooth.source_offset(Time::seconds(s))
            );
        }
        // Early on the smooth one is still nearly at full speed: ahead.
        let tenth = Time::from_ratio(1, 10).unwrap();
        assert!(smooth.source_offset(tenth) > linear.source_offset(tenth));
        // The speed in the middle is the middle, at the ends the ends.
        let half = Time::from_ratio(1, 2).unwrap();
        assert_eq!(smooth.speed_at(half), Some(speed(5, 8)));
        assert_eq!(smooth.speed_at(Time::ZERO), Some(speed(1, 1)));
        assert_eq!(smooth.speed_at(Time::seconds(3)), Some(speed(1, 4)));
        assert!((smooth.speed_at_f64(0.5) - 0.625).abs() < 1e-9);
    }

    #[test]
    fn played_file_never_goes_back_and_matches_the_float_version() {
        let ramp = Ramp::new([
            key(Time::ZERO, speed(1, 1), Ease::Smooth),
            key(frames(30), speed(1, 4), Ease::Linear),
            key(frames(90), speed(1, 4), Ease::Smooth),
            key(frames(120), speed(3, 2), Ease::Hold),
            key(frames(150), speed(1, 1), Ease::Linear),
        ]);
        let mut last = Time::seconds(-100);
        for n in -30..400 {
            let at = ramp.source_offset(frames(n));
            assert!(at > last, "frame {n}");
            let float = ramp.source_offset_f64(frames(n).as_secs_f64());
            assert!((at.as_secs_f64() - float).abs() < 1e-6, "frame {n}: {at} {float}");
            last = at;
        }
    }

    #[test]
    fn hold_keeps_the_speed_until_the_next_keyframe() {
        let ramp = Ramp::new([
            key(Time::ZERO, speed(1, 2), Ease::Hold),
            key(Time::seconds(2), speed(2, 1), Ease::Linear),
        ]);
        assert_eq!(ramp.source_offset(Time::seconds(2)), Time::seconds(1));
        assert_eq!(ramp.source_offset(Time::seconds(3)), Time::seconds(3));
        assert_eq!(ramp.speed_at(Time::seconds(1)), Some(speed(1, 2)));
    }

    #[test]
    fn shifting_keeps_the_same_curve_from_a_new_start() {
        let ramp = Ramp::new([
            key(Time::seconds(1), speed(1, 1), Ease::Smooth),
            key(Time::seconds(2), speed(1, 4), Ease::Linear),
        ]);
        // A clip trimmed by 1.5 s at its start: the part from 1.5 s on.
        let cut = Time::from_ratio(3, 2).unwrap();
        let trimmed = ramp.shifted(-cut);
        for n in 0..240 {
            let at = frames(n);
            assert_eq!(
                trimmed.source_offset(at),
                ramp.source_offset(at + cut) - ramp.source_offset(cut),
                "frame {n}"
            );
        }
    }

    #[test]
    fn keys_are_set_removed_and_eased_in_a_frame() {
        let mut ramp = Ramp::default();
        let frame = |n| (frames(n), frames(n + 1));
        ramp.set(frames(0), frame(0), speed(1, 1));
        ramp.set(frames(30), frame(30), speed(1, 4));
        ramp.set(frames(30), frame(30), speed(1, 2));
        assert_eq!(ramp.keys().len(), 2);
        assert_eq!(ramp.keys()[1].speed, speed(1, 2));
        assert!(ramp.set_ease(frame(0), Ease::Smooth));
        ramp.set(frames(10), frame(10), speed(3, 4));
        assert_eq!(
            ramp.keys()[1].ease,
            Ease::Smooth,
            "a new one keeps the stretch's ease"
        );
        assert!(ramp.remove(frame(10)));
        assert!(!ramp.remove(frame(10)));
        assert_eq!(ramp.summary(), "Ramp 100→50%");
        assert!(ramp.changes());
    }

    #[test]
    fn slow_motion_eases_in_and_out_and_clean_slow_fits_the_recording() {
        let ramp = slow_motion(
            Time::seconds(1),
            speed(1, 1),
            speed(1, 4),
            Time::from_ratio(1, 2).unwrap(),
            Some(Time::seconds(1)),
        );
        let speeds: Vec<String> = ramp.keys().iter().map(|k| k.speed.percent()).collect();
        assert_eq!(speeds, ["100%", "25%", "25%", "100%"]);
        assert_eq!(ramp.summary(), "Ramp 100→25→100%");
        assert_eq!(clean_slow(Rate::FPS_120, Rate::FPS_60), speed(1, 2));
        assert_eq!(clean_slow(Rate::FPS_144, Rate::FPS_60), speed(5, 12));
        assert_eq!(clean_slow(Rate::FPS_60, Rate::FPS_60), speed(1, 2));
        assert_eq!(clean_slow(Rate::whole(480), Rate::FPS_60), speed(1, 4));
        assert_eq!(
            clean_slow(Rate::new(120_000, 1001).unwrap(), Rate::NTSC_60),
            speed(1, 2)
        );
    }

    #[test]
    fn problems_are_found_and_huge_values_dont_overflow() {
        let ramp = Ramp(vec![
            key(Time::seconds(2), speed(1, 1), Ease::Linear),
            key(Time::seconds(1), speed(9_999, 10_000), Ease::Linear),
        ]);
        assert!(ramp.problems("clip").join(" ").contains("out of order"));
        let odd = Ramp::new([key(Time::ZERO, speed(1_000_001, 1_000_000), Ease::Linear)]);
        assert!(odd.problems("clip").join(" ").contains("plain fraction"));
        // A very long ramp between extreme speeds, far away: no panic.
        let long = Ramp::new([
            key(Time::seconds(-80_000), speed(1, 100), Ease::Smooth),
            key(Time::seconds(80_000), speed(100, 1), Ease::Linear),
        ]);
        let mid = long.source_offset(Time::seconds(40_000));
        assert!(mid > Time::ZERO);
        assert!(long.problems("clip").is_empty());
    }

    #[test]
    fn ramps_are_written_as_a_plain_list() {
        let ramp = Ramp::new([key(Time::seconds(1), speed(1, 2), Ease::Smooth)]);
        let text = ron::to_string(&ramp).unwrap();
        assert_eq!(ron::from_str::<Ramp>(&text).unwrap(), ramp);
        let short: Ramp = ron::from_str("[(at: (1, 1), speed: (num: 1, den: 4))]").unwrap();
        assert_eq!(short.keys()[0].ease, Ease::Linear);
    }
}
