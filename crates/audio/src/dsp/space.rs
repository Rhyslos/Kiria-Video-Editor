//! Space: an echo (a delay that repeats, fading) and a reverb (a room's
//! many reflections).
//!
//! Both keep their memory in buffers made with the effect, off the
//! real-time thread, at the longest they may need, so changing a setting
//! never allocates. Each adds its sound to the track's as it came (the dry
//! sound is never turned down), by its own level.
//!
//! The reverb is a network of eight feedback delays with damping in each
//! loop and four all-pass diffusers after them, per channel, the right's
//! delays a little longer than the left's so the two sides differ: the
//! well-known public-domain design, its delay lengths scaled to the rate.

/// The echo's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EchoSettings {
    pub on: bool,
    /// Time between repeats: 20 to 1 500 ms.
    pub time_ms: f32,
    /// How much of each repeat comes back: 0 to 0.9.
    pub feedback: f32,
    /// The repeats' level: 0 to 1.
    pub mix: f32,
    /// Repeats going left and right in turn.
    pub ping_pong: bool,
}

impl EchoSettings {
    pub const OFF: EchoSettings = EchoSettings {
        on: false,
        time_ms: 350.0,
        feedback: 0.35,
        mix: 0.35,
        ping_pong: false,
    };
}

/// The longest echo, in seconds (the buffer is a little longer).
pub const ECHO_LONGEST: f32 = 1.5;

/// The echo.
#[derive(Debug)]
pub struct Echo {
    settings: EchoSettings,
    rate: f32,
    lines: [Vec<f32>; 2],
    at: usize,
    /// The delay in samples now, gliding to the set one so a change slides
    /// in pitch instead of clicking.
    delay: f32,
}

impl Echo {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: f32, settings: EchoSettings) -> Echo {
        let len = (rate * ECHO_LONGEST) as usize + 4;
        Echo {
            settings,
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            delay: Echo::samples(rate, settings.time_ms),
        }
    }

    fn samples(rate: f32, time_ms: f32) -> f32 {
        (time_ms.clamp(20.0, ECHO_LONGEST * 1_000.0) / 1_000.0 * rate).max(1.0)
    }

    /// Changes its settings. Never allocates; turned on again, it starts
    /// from silence rather than an old tail.
    pub fn set(&mut self, settings: EchoSettings) {
        if settings.on && !self.settings.on {
            for line in &mut self.lines {
                line.fill(0.0);
            }
            self.delay = Echo::samples(self.rate, settings.time_ms);
        }
        self.settings = settings;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = self.settings;
        if !s.on {
            return;
        }
        let len = self.lines[0].len();
        let wanted = Echo::samples(self.rate, s.time_ms);
        // About 50 ms to glide to a new time.
        let glide = 1.0 - (-1.0 / (0.05 * self.rate)).exp();
        let feedback = s.feedback.clamp(0.0, 0.9);
        let mix = s.mix.clamp(0.0, 1.0);
        for (dl, dr) in l.iter_mut().zip(r.iter_mut()) {
            self.delay += (wanted - self.delay) * glide;
            let back = (self.at as f32 - self.delay).rem_euclid(len as f32);
            let read = |line: &[f32]| {
                let i = back.floor() as usize % len;
                let next = (i + 1) % len;
                let f = back - back.floor();
                line[i] + (line[next] - line[i]) * f
            };
            let (yl, yr) = (read(&self.lines[0]), read(&self.lines[1]));
            let (wl, wr) = if s.ping_pong {
                (0.5 * (*dl + *dr) + yr * feedback, yl * feedback)
            } else {
                (*dl + yl * feedback, *dr + yr * feedback)
            };
            self.lines[0][self.at] = wl;
            self.lines[1][self.at] = wr;
            self.at = (self.at + 1) % len;
            *dl += yl * mix;
            *dr += yr * mix;
        }
    }

    /// Carries the repeats under way over from the one this replaces.
    pub fn take_state(&mut self, old: &Echo) {
        if self.lines[0].len() == old.lines[0].len() {
            for (new, was) in self.lines.iter_mut().zip(&old.lines) {
                new.copy_from_slice(was);
            }
            self.at = old.at;
            self.delay = old.delay;
        }
    }
}

/// The reverb's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    pub on: bool,
    /// How big the room: 0 (small) to 1 (a hall).
    pub size: f32,
    /// How quickly high sounds die in it: 0 (bright) to 1 (dull).
    pub damping: f32,
    /// The room's level: 0 to 1.
    pub mix: f32,
}

impl ReverbSettings {
    pub const OFF: ReverbSettings = ReverbSettings {
        on: false,
        size: 0.5,
        damping: 0.5,
        mix: 0.25,
    };
}

/// The feedback delays' lengths at 44.1 kHz.
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
/// The all-pass diffusers' lengths at 44.1 kHz.
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
/// How much longer the right's delays are, at 44.1 kHz.
const SPREAD: usize = 23;
/// The sound into the room, so eight loops added stay in range.
const INTO: f32 = 0.015;
/// The room's sound out, brought back up.
const OUT: f32 = 3.0;

#[derive(Debug)]
struct Comb {
    line: Vec<f32>,
    at: usize,
    /// The loop's last sound after damping.
    kept: f32,
}

impl Comb {
    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.line[self.at];
        self.kept = y * (1.0 - damp) + self.kept * damp;
        self.line[self.at] = x + self.kept * feedback;
        self.at = (self.at + 1) % self.line.len();
        y
    }
}

#[derive(Debug)]
struct AllPass {
    line: Vec<f32>,
    at: usize,
}

impl AllPass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let held = self.line[self.at];
        let y = held - x;
        self.line[self.at] = x + held * 0.5;
        self.at = (self.at + 1) % self.line.len();
        y
    }
}

/// The reverb.
#[derive(Debug)]
pub struct Reverb {
    settings: ReverbSettings,
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<AllPass>; 2],
}

impl Reverb {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: f32, settings: ReverbSettings) -> Reverb {
        let scale = rate / 44_100.0;
        let length = |n: usize, side: usize| (((n + side * SPREAD) as f32 * scale) as usize).max(1);
        let combs = [0, 1].map(|side| {
            COMBS
                .iter()
                .map(|&n| Comb {
                    line: vec![0.0; length(n, side)],
                    at: 0,
                    kept: 0.0,
                })
                .collect()
        });
        let allpasses = [0, 1].map(|side| {
            ALLPASSES
                .iter()
                .map(|&n| AllPass {
                    line: vec![0.0; length(n, side)],
                    at: 0,
                })
                .collect()
        });
        Reverb {
            settings,
            combs,
            allpasses,
        }
    }

    /// Changes its settings. Never allocates; turned on again, the room
    /// starts quiet.
    pub fn set(&mut self, settings: ReverbSettings) {
        if settings.on && !self.settings.on {
            self.clear();
        }
        self.settings = settings;
    }

    fn clear(&mut self) {
        for c in self.combs.iter_mut().flatten() {
            c.line.fill(0.0);
            c.kept = 0.0;
        }
        for a in self.allpasses.iter_mut().flatten() {
            a.line.fill(0.0);
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = self.settings;
        if !s.on {
            return;
        }
        let feedback = 0.7 + 0.28 * s.size.clamp(0.0, 1.0);
        let damp = 0.4 * s.damping.clamp(0.0, 1.0);
        let mix = s.mix.clamp(0.0, 1.0) * OUT;
        for (dl, dr) in l.iter_mut().zip(r.iter_mut()) {
            let into = (*dl + *dr) * INTO;
            let mut out = [0.0f32; 2];
            for (side, o) in out.iter_mut().enumerate() {
                let mut sum = 0.0;
                for c in &mut self.combs[side] {
                    sum += c.tick(into, feedback, damp);
                }
                for a in &mut self.allpasses[side] {
                    sum = a.tick(sum);
                }
                *o = sum;
            }
            *dl += out[0] * mix;
            *dr += out[1] * mix;
        }
    }

    /// Carries the room's sound over from the one this replaces.
    pub fn take_state(&mut self, old: &Reverb) {
        let pairs = self.combs.iter_mut().flatten().zip(old.combs.iter().flatten());
        for (new, was) in pairs {
            if new.line.len() == was.line.len() {
                new.line.copy_from_slice(&was.line);
                new.at = was.at;
                new.kept = was.kept;
            }
        }
        let pairs = self
            .allpasses
            .iter_mut()
            .flatten()
            .zip(old.allpasses.iter().flatten());
        for (new, was) in pairs {
            if new.line.len() == was.line.len() {
                new.line.copy_from_slice(&was.line);
                new.at = was.at;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// A click followed by silence, `seconds` long, through `run`.
    fn click(seconds: f32, mut run: impl FnMut(&mut [f32], &mut [f32])) -> (Vec<f32>, Vec<f32>) {
        let n = (RATE * seconds) as usize;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        l[0] = 1.0;
        r[0] = 1.0;
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            run(a, b);
        }
        (l, r)
    }

    #[test]
    fn an_echo_repeats_at_its_time_fading_by_its_feedback() {
        let settings = EchoSettings {
            on: true,
            time_ms: 100.0,
            feedback: 0.5,
            mix: 1.0,
            ping_pong: false,
        };
        let mut echo = Echo::new(RATE, settings);
        let (l, _) = click(0.5, |a, b| echo.process(a, b));
        let at = |ms: usize| l[ms * 48];
        assert_eq!(at(0), 1.0, "the click as it came");
        assert!((at(100) - 1.0).abs() < 0.01, "the first repeat: {}", at(100));
        assert!((at(200) - 0.5).abs() < 0.01, "the second, half: {}", at(200));
        assert!(at(150).abs() < 0.001, "silence between");
    }

    #[test]
    fn a_ping_pong_echo_goes_left_then_right() {
        let settings = EchoSettings {
            on: true,
            time_ms: 100.0,
            feedback: 0.5,
            mix: 1.0,
            ping_pong: true,
        };
        let mut echo = Echo::new(RATE, settings);
        let (l, r) = click(0.5, |a, b| echo.process(a, b));
        assert!(l[4_800] > 0.9 && r[4_800].abs() < 0.001, "first left");
        assert!(r[9_600] > 0.4 && l[9_600].abs() < 0.001, "then right");
    }

    #[test]
    fn a_reverb_rings_on_and_dies_away_and_off_does_nothing() {
        let mut room = Reverb::new(
            RATE,
            ReverbSettings {
                on: true,
                ..ReverbSettings::OFF
            },
        );
        let (l, r) = click(3.0, |a, b| room.process(a, b));
        let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>();
        let early = energy(&l[4_800..24_000]);
        let late = energy(&l[120_000..144_000]);
        assert!(early > 1e-4, "it rings: {early}");
        assert!(late < early / 100.0, "and dies away: {late} after {early}");
        assert_ne!(l[10_000..10_100], r[10_000..10_100], "the sides differ");
        let mut off = Reverb::new(RATE, ReverbSettings::OFF);
        let (l, _) = click(0.1, |a, b| off.process(a, b));
        assert!(l[1..].iter().all(|&x| x == 0.0));
    }

    #[test]
    fn a_bigger_room_rings_longer() {
        let ring = |size| {
            let mut room = Reverb::new(
                RATE,
                ReverbSettings {
                    on: true,
                    size,
                    ..ReverbSettings::OFF
                },
            );
            let (l, _) = click(2.0, |a, b| room.process(a, b));
            l[48_000..96_000].iter().map(|x| x * x).sum::<f32>()
        };
        assert!(ring(0.9) > ring(0.2) * 10.0);
    }
}
