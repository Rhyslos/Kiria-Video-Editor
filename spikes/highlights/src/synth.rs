//! Synthetic test clips with known events, so the ground truth is exact.
//!
//! Each clip looks and sounds a little like a game recording: moving
//! pictures with grain, a game sound bed, someone talking on the mic now and
//! then, and on some clips voice chat. Events a person would pick are
//! planted at known times (shouts and laughs on the mic, explosions in the
//! game sound, laughs on the chat, hard cuts, whip pans), and so are decoys
//! a person would not pick (a cough, bursts of ordinary gunfire, the music
//! or engine swelling, a slow pan, cuts are never decoys). The events go to
//! `<clip>.csv`, the decoys to comment lines in the same file.
//!
//! The sound is made here, sample by sample; the picture is made by the
//! `ffmpeg` program from its built-in test sources, cropped by a moving
//! window whose path holds the pans and jumps, and everything is encoded
//! like a recorder would: H.265 at 1440p 60 with a fixed key frame
//! distance and no B frames, sound as AAC, in MKV.

use crate::detect::Role;
use std::path::Path;
use std::process::Command;

const SAMPLE_RATE: u32 = 48_000;

/// Small deterministic random numbers.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// 0..1
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    /// -1..1
    fn noise(&mut self) -> f32 {
        (self.unit() * 2.0 - 1.0) as f32
    }
}

/// Something planted at a known time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    // Events: in the ground truth.
    Shout,
    MicLaugh,
    Explosion,
    ChatLaugh,
    /// A hard cut to another scene.
    Cut,
    /// A hard cut to another view of the same scene: colours barely change.
    SameSceneCut,
    /// A whip pan: the view moves most of a screen in 0.4 s.
    Pan,
    /// An explosion and the view jolting with it.
    ExplosionPan,
    // Decoys: not in the ground truth.
    Cough,
    Gunfire,
    Swell,
    SlowPan,
}

impl Kind {
    /// What the ground truth calls it; `None` for decoys.
    fn truth(self) -> Option<&'static str> {
        match self {
            Kind::Shout | Kind::MicLaugh => Some("mic"),
            Kind::Explosion => Some("game"),
            Kind::ChatLaugh => Some("chat"),
            Kind::Cut | Kind::SameSceneCut => Some("cut"),
            Kind::Pan => Some("motion"),
            Kind::ExplosionPan => Some("game+motion"),
            Kind::Cough | Kind::Gunfire | Kind::Swell | Kind::SlowPan => None,
        }
    }
}

/// The kind of game a clip pretends to be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Bed {
    /// Music and steady shooting.
    Arena,
    /// A loud engine.
    Racing,
    /// Quiet ambience.
    Calm,
}

/// One synthetic clip.
#[derive(Clone, Debug)]
pub struct Plan {
    pub name: &'static str,
    seed: u64,
    bed: Bed,
    roles: Vec<Role>,
    /// Stream titles in the file, as some recorders write them.
    titles: bool,
    /// Key frame distance in frames.
    keys: u32,
    /// Events and decoys, with their chances.
    menu: Vec<(Kind, f64)>,
}

pub fn plans() -> Vec<Plan> {
    use Kind::*;
    vec![
        Plan {
            name: "arena",
            seed: 11,
            bed: Bed::Arena,
            roles: vec![Role::Game, Role::Mic, Role::Chat],
            titles: true,
            keys: 120,
            menu: vec![
                (Shout, 3.0),
                (MicLaugh, 1.0),
                (Explosion, 2.0),
                (ExplosionPan, 2.0),
                (ChatLaugh, 1.0),
                (Pan, 2.0),
                (Cut, 1.0),
                (SameSceneCut, 1.0),
                (Gunfire, 1.5),
                (Cough, 1.0),
                (SlowPan, 1.0),
            ],
        },
        Plan {
            name: "racing",
            seed: 23,
            bed: Bed::Racing,
            roles: vec![Role::Game, Role::Mic],
            titles: true,
            keys: 480,
            menu: vec![
                (Shout, 2.0),
                (MicLaugh, 1.0),
                (Explosion, 2.0),
                (ExplosionPan, 1.0),
                (Cut, 2.0),
                (Pan, 1.0),
                (Swell, 2.0),
                (Cough, 1.0),
                (SlowPan, 1.0),
            ],
        },
        Plan {
            name: "calm",
            seed: 37,
            bed: Bed::Calm,
            roles: vec![Role::Game, Role::Mic, Role::Chat],
            titles: false,
            keys: 120,
            menu: vec![
                (Shout, 1.0),
                (MicLaugh, 2.0),
                (ChatLaugh, 2.0),
                (Explosion, 1.0),
                (Cut, 3.0),
                (SameSceneCut, 1.0),
                (Pan, 1.0),
                (Cough, 1.0),
                (SlowPan, 2.0),
                (Swell, 1.0),
            ],
        },
    ]
}

/// Places events and decoys 6 to 11 seconds apart, each kind as often as
/// its share of the menu says, in a shuffled order.
fn timeline(plan: &Plan, seconds: f64, rng: &mut Rng) -> Vec<(f64, Kind)> {
    let mut times = Vec::new();
    let mut t = rng.range(5.0, 8.0);
    while t < seconds - 6.0 {
        times.push((t * 100.0).round() / 100.0);
        t += rng.range(6.0, 11.0);
    }
    // Smooth weighted round robin: every kind gets its share, rounded.
    let total: f64 = plan.menu.iter().map(|m| m.1).sum();
    let mut credit = vec![0.0; plan.menu.len()];
    let mut kinds = Vec::new();
    for _ in &times {
        for (c, m) in credit.iter_mut().zip(&plan.menu) {
            *c += m.1;
        }
        let best = (0..credit.len()).fold(0, |b, i| if credit[i] > credit[b] { i } else { b });
        credit[best] -= total;
        kinds.push(plan.menu[best].0);
    }
    for i in (1..kinds.len()).rev() {
        kinds.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    times.into_iter().zip(kinds).collect()
}

// ---------------------------------------------------------------- sound --

fn from_db(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

fn index(t: f64) -> usize {
    (t * f64::from(SAMPLE_RATE)).max(0.0) as usize
}

/// A burst of noise: sharp attack, then dying away with time constant `tau`;
/// `tone` 0..1 from dull to bright.
fn burst(buf: &mut [f32], rng: &mut Rng, at: f64, length: f64, peak_db: f64, tau: f64, tone: f32) {
    let peak = from_db(peak_db);
    let mut low = 0.0f32;
    let (from, to) = (index(at), index(at + length).min(buf.len()));
    for (n, s) in buf[from..to.max(from)].iter_mut().enumerate() {
        let t = n as f64 / f64::from(SAMPLE_RATE);
        let envelope = (t / 0.004).min(1.0) * (-t / tau).exp();
        low += tone * (rng.noise() - low);
        *s += peak * envelope as f32 * low * 1.8;
    }
}

/// A voice: a buzzing tone at pitch `f0` with harmonics, plus breath noise,
/// shaped into syllables about 4.5 a second.
#[allow(clippy::too_many_arguments)]
fn voice(
    buf: &mut [f32],
    rng: &mut Rng,
    at: f64,
    length: f64,
    level_db: f64,
    f0: f64,
    syllables: f64,
    sharp: bool,
) {
    let level = from_db(level_db);
    let (from, to) = (index(at), index(at + length).min(buf.len()));
    let mut phase = 0.0f64;
    let mut low = 0.0f32;
    let mut band = 0.0f32;
    let wobble = rng.range(0.0, std::f64::consts::TAU);
    for (n, s) in buf[from..to.max(from)].iter_mut().enumerate() {
        let t = n as f64 / f64::from(SAMPLE_RATE);
        let pitch = f0 * (1.0 + 0.06 * (t * 3.1 + wobble).sin());
        phase += pitch / f64::from(SAMPLE_RATE);
        let mut tone = 0.0;
        for k in 1..=8 {
            tone += (2.0 * std::f64::consts::PI * phase * k as f64).sin() / k as f64;
        }
        low += 0.3 * (rng.noise() - low);
        band += 0.03 * (low - band);
        let syllable =
            ((std::f64::consts::PI * t * syllables + wobble).sin().abs()).powf(if sharp { 1.0 } else { 2.0 });
        let edge = (t / 0.03).min(1.0) * ((length - t) / 0.05).clamp(0.0, 1.0);
        *s += level * (syllable * edge) as f32 * (0.5 * tone as f32 + 0.8 * (low - band));
    }
}

/// Ordinary talking: spurts of 2 to 6 seconds with pauses of 1 to 5.
fn talking(buf: &mut [f32], rng: &mut Rng, seconds: f64, level_db: f64, f0: f64, pause: f64) {
    let mut t = rng.range(0.0, 2.0);
    while t < seconds {
        let length = rng.range(2.0, 6.0);
        let (level, pitch) = (level_db + rng.range(-3.0, 3.0), f0 * rng.range(0.9, 1.1));
        voice(buf, rng, t, length, level, pitch, 4.5, false);
        t += length + rng.range(1.0, pause);
    }
}

fn game_bed(buf: &mut [f32], rng: &mut Rng, bed: Bed, swells: &[f64]) {
    let swell = |t: f64| -> f64 {
        // +6 dB over 3 s, held 2 s, back over 3 s.
        swells
            .iter()
            .map(|&s| {
                let x = t - s;
                if !(0.0..=8.0).contains(&x) {
                    0.0
                } else {
                    6.0 * (x / 3.0).min(1.0).min((8.0 - x) / 3.0)
                }
            })
            .fold(0.0, f64::max)
    };
    let mut low = 0.0f32;
    let mut chord = [220.0, 277.0, 330.0];
    let mut phases = [0.0f64; 3];
    let mut engine = 0.0f64;
    for (n, out) in buf.iter_mut().enumerate() {
        let t = n as f64 / f64::from(SAMPLE_RATE);
        let gain = from_db(swell(t));
        low += 0.05 * (rng.noise() - low);
        let s = match bed {
            Bed::Arena => {
                if n % (SAMPLE_RATE as usize * 4) == 0 {
                    for c in &mut chord {
                        *c = 110.0 * 2f64.powf(rng.range(0.0, 24.0).round() / 12.0);
                    }
                }
                let mut music = 0.0;
                for (p, f) in phases.iter_mut().zip(chord) {
                    *p += f / f64::from(SAMPLE_RATE);
                    music += (2.0 * std::f64::consts::PI * *p).sin();
                }
                from_db(-30.0) * music as f32 + from_db(-34.0) * low
            }
            Bed::Racing => {
                let f = 90.0 + 35.0 * (t * 0.21).sin() + 15.0 * (t * 0.67).sin();
                engine = (engine + f / f64::from(SAMPLE_RATE)).fract();
                from_db(-20.0) * (2.0 * engine - 1.0) as f32 * 0.6 + from_db(-30.0) * low
            }
            Bed::Calm => {
                phases[0] += 165.0 / f64::from(SAMPLE_RATE);
                let pad = (2.0 * std::f64::consts::PI * phases[0]).sin() * (0.5 + 0.5 * (t * 0.3).sin());
                from_db(-36.0) * pad as f32 + from_db(-40.0) * low
            }
        };
        *out += s * gain;
    }
    if bed == Bed::Arena {
        // Single shots now and then: part of the game, not highlights.
        let mut t = 1.0;
        while t < buf.len() as f64 / f64::from(SAMPLE_RATE) {
            burst(buf, rng, t, 0.1, -24.0, 0.02, 0.6);
            t += rng.range(1.0, 4.0);
        }
    }
}

fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for &s in samples {
        bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// The sound of one stream.
fn sound(plan: &Plan, role: Role, events: &[(f64, Kind)], seconds: f64, rng: &mut Rng) -> Vec<f32> {
    let mut buf = vec![0.0f32; index(seconds)];
    match role {
        Role::Game => {
            let swells: Vec<f64> = events
                .iter()
                .filter(|e| e.1 == Kind::Swell)
                .map(|e| e.0)
                .collect();
            game_bed(&mut buf, rng, plan.bed, &swells);
            for &(t, kind) in events {
                match kind {
                    Kind::Explosion | Kind::ExplosionPan => {
                        burst(&mut buf, rng, t, 2.5, -3.0, 0.5, 0.15);
                        burst(&mut buf, rng, t + 0.05, 0.6, -9.0, 0.15, 0.7);
                    }
                    Kind::Gunfire => {
                        for shot in 0..20 {
                            burst(&mut buf, rng, t + f64::from(shot) * 0.1, 0.09, -16.0, 0.015, 0.6);
                        }
                    }
                    _ => {}
                }
            }
        }
        Role::Mic => {
            for s in buf.iter_mut() {
                *s = from_db(-62.0) * rng.noise();
            }
            talking(&mut buf, rng, seconds, -30.0, 125.0, 5.0);
            for &(t, kind) in events {
                match kind {
                    Kind::Shout => {
                        let length = rng.range(0.8, 1.5);
                        voice(&mut buf, rng, t, length, -9.0, 260.0, 3.0, true);
                    }
                    Kind::MicLaugh => voice(&mut buf, rng, t, 1.3, -14.0, 240.0, 9.0, true),
                    Kind::Cough => {
                        burst(&mut buf, rng, t, 0.15, -18.0, 0.05, 0.4);
                        burst(&mut buf, rng, t + 0.35, 0.15, -20.0, 0.05, 0.4);
                    }
                    _ => {}
                }
            }
        }
        Role::Chat => {
            for s in buf.iter_mut() {
                *s = from_db(-58.0) * rng.noise();
            }
            talking(&mut buf, rng, seconds, -33.0, 195.0, 9.0);
            for &(t, kind) in events {
                if kind == Kind::ChatLaugh {
                    voice(&mut buf, rng, t, 1.5, -15.0, 300.0, 10.0, true);
                }
            }
        }
    }
    buf
}

// -------------------------------------------------------------- picture --

/// The built-in test sources the scenes are made of, at 1920×1080 60 fps.
/// All have shapes larger than the analysis's 40-pixel blocks: a scene of
/// fine, even texture (an automaton pattern) looked flat grey to it, so
/// whip pans over it went unseen. Games rarely look like that for minutes.
const SCENES: [&str; 7] = [
    "testsrc2=size=1920x1080:rate=60",
    "gradients=size=640x360:rate=60:speed=0.01:seed=SEED,scale=1920:1080",
    "sierpinski=size=1920x1080:rate=60",
    "smptehdbars=size=1920x1080:rate=60",
    "testsrc=size=1920x1080:rate=60",
    "colorspectrum=size=1920x1080:rate=60",
    "rgbtestsrc=size=1920x1080:rate=60",
];

/// A change of the crop window's path: from `start`, over `length`
/// seconds, the position moves by `dx`, `dy` (in units of the room the
/// window has; 1 is the whole way across).
struct Move {
    start: f64,
    length: f64,
    dx: f64,
    dy: f64,
}

/// The crop window's path within one scene that starts at `offset` and
/// lasts `length`: an expression of the scene's own time `t`.
fn path(moves: &[Move], offset: f64, length: f64, base: f64, drift: f64, axis: usize) -> String {
    let mut constant = base + drift * offset;
    let mut terms = vec![format!("{drift}*t")];
    for m in moves {
        let amount = if axis == 0 { m.dx } else { m.dy };
        if amount == 0.0 {
            continue;
        }
        if m.start + m.length <= offset {
            constant += amount;
        } else if m.start < offset + length {
            let local = m.start - offset;
            terms.push(if m.length == 0.0 {
                format!("{amount}*gte(t,{local:.4})")
            } else {
                format!("{amount}*clip((t-{local:.4})/{},0,1)", m.length)
            });
        }
    }
    // Bounce between the edges: the window never jumps when it reaches one.
    format!("abs(mod({constant:.4}+{},2)-1)", terms.join("+"))
}

fn filter_graph(events: &[(f64, Kind)], seconds: f64, seed: u64, size: (u32, u32)) -> String {
    let mut moves = Vec::new();
    for &(t, kind) in events {
        match kind {
            Kind::Pan | Kind::ExplosionPan => moves.push(Move {
                start: t,
                length: 0.4,
                dx: 0.9,
                dy: 0.3,
            }),
            Kind::SlowPan => moves.push(Move {
                start: t,
                length: 3.0,
                dx: 0.6,
                dy: 0.0,
            }),
            Kind::SameSceneCut => moves.push(Move {
                start: t,
                length: 0.0,
                dx: 0.5,
                dy: 0.4,
            }),
            _ => {}
        }
    }
    let mut cuts: Vec<f64> = events.iter().filter(|e| e.1 == Kind::Cut).map(|e| e.0).collect();
    cuts.insert(0, 0.0);
    cuts.push(seconds);
    let mut graph = String::new();
    let n = cuts.len() - 1;
    for i in 0..n {
        let (offset, length) = (cuts[i], cuts[i + 1] - cuts[i]);
        let scene =
            SCENES[(i + seed as usize) % SCENES.len()].replace("SEED", &(seed + i as u64).to_string());
        let x = path(&moves, offset, length, 0.37, 0.03, 0);
        let y = path(&moves, offset, length, 0.5, 0.017, 1);
        let hue = (i * 67 + seed as usize * 13) % 360;
        graph.push_str(&format!(
            "{scene},trim=duration={length:.4},setpts=PTS-STARTPTS,crop=1280:720:x='640*{x}':y='360*{y}',hue=h={hue},noise=alls=10:allf=t[s{i}];"
        ));
    }
    for i in 0..n {
        graph.push_str(&format!("[s{i}]"));
    }
    graph.push_str(&format!(
        "concat=n={n}:v=1:a=0,scale={}:{}:flags=bilinear,format=yuv420p[v]",
        size.0, size.1
    ));
    graph
}

/// How a clip is encoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    /// H.265 at 1440p, like the recordings (slow to make).
    H265,
    /// H.264 at 1440p: the same pictures, several times quicker to make.
    H264,
    /// H.264 at 720p, for slow machines.
    Small,
}

/// Makes `<folder>/<name>.mkv` and `<folder>/<name>.csv` for one plan.
pub fn make(
    plan: &Plan,
    folder: &Path,
    seconds: f64,
    encoding: Encoding,
    ffmpeg: &str,
) -> Result<(), String> {
    let mut rng = Rng(plan.seed);
    let events = timeline(plan, seconds, &mut rng);
    let mut csv = String::from("time,kind\n");
    for &(t, kind) in &events {
        match kind.truth() {
            Some(name) => csv.push_str(&format!("{t:.2},{name}\n")),
            None => csv.push_str(&format!("# decoy at {t:.2}: {kind:?}\n")),
        }
    }
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    std::fs::write(folder.join(format!("{}.csv", plan.name)), csv).map_err(|e| e.to_string())?;

    let mut wavs = Vec::new();
    for role in &plan.roles {
        let path = folder.join(format!("{}.{}.wav", plan.name, role.name()));
        write_wav(&path, &sound(plan, *role, &events, seconds, &mut rng))?;
        wavs.push(path);
    }
    let out = folder.join(format!("{}.mkv", plan.name));
    let size = if encoding == Encoding::Small {
        (1280, 720)
    } else {
        (2560, 1440)
    };
    let mut command = Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-filter_complex")
        .arg(filter_graph(&events, seconds, plan.seed, size));
    for wav in &wavs {
        command.arg("-i").arg(wav);
    }
    command.args(["-map", "[v]"]);
    for i in 0..wavs.len() {
        command.args(["-map", &format!("{i}:a")]);
    }
    let keys = plan.keys.to_string();
    match encoding {
        Encoding::Small => command.args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-bf",
            "0",
            "-sc_threshold",
            "0",
            "-b:v",
            "8M",
        ]),
        Encoding::H264 => command.args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-bf",
            "0",
            "-sc_threshold",
            "0",
            "-b:v",
            "30M",
        ]),
        Encoding::H265 => command.args([
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-b:v",
            "30M",
            "-x265-params",
            "log-level=error:bframes=0:scenecut=0",
        ]),
    };
    command.args([
        "-g",
        &keys,
        "-keyint_min",
        &keys,
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        "-t",
        &seconds.to_string(),
    ]);
    if plan.titles {
        for (i, role) in plan.roles.iter().enumerate() {
            let title = match role {
                Role::Game => "Game",
                Role::Mic => "Microphone",
                Role::Chat => "Voice chat",
            };
            command.args([format!("-metadata:s:a:{i}"), format!("title={title}")]);
        }
    }
    command.arg(&out);
    let result = command
        .output()
        .map_err(|e| format!("the ffmpeg program could not be started ({e})"))?;
    for wav in &wavs {
        let _ = std::fs::remove_file(wav);
    }
    if !result.status.success() {
        let text = String::from_utf8_lossy(&result.stderr);
        return Err(format!("ffmpeg failed making {}: {}", plan.name, text.trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_place_events_apart_and_the_same_every_time() {
        for plan in plans() {
            let a = timeline(&plan, 300.0, &mut Rng(plan.seed));
            let b = timeline(&plan, 300.0, &mut Rng(plan.seed));
            assert_eq!(a, b);
            assert!(a.len() >= 25, "{}: {}", plan.name, a.len());
            assert!(a.windows(2).all(|w| w[1].0 - w[0].0 >= 6.0));
            assert!(a.iter().filter(|e| e.1.truth().is_some()).count() >= 15);
        }
    }

    #[test]
    fn paths_fold_finished_moves_into_the_start() {
        let moves = [
            Move {
                start: 5.0,
                length: 0.4,
                dx: 0.9,
                dy: 0.3,
            },
            Move {
                start: 30.0,
                length: 0.0,
                dx: 0.5,
                dy: 0.4,
            },
        ];
        // A scene from 10 to 40 s: the pan is over, the jump is inside.
        let x = path(&moves, 10.0, 30.0, 0.37, 0.03, 0);
        assert_eq!(x, "abs(mod(1.5700+0.03*t+0.5*gte(t,20.0000),2)-1)");
        let early = path(&moves, 0.0, 10.0, 0.37, 0.03, 0);
        assert!(early.contains("0.9*clip((t-5.0000)/0.4,0,1)"));
        assert!(!early.contains("gte"));
    }

    #[test]
    fn a_wav_has_the_right_length() {
        let dir = std::env::temp_dir().join("highlight-bench-wav-test");
        std::fs::create_dir_all(&dir).expect("temp folder");
        let path = dir.join("a.wav");
        write_wav(&path, &[0.0; 480]).expect("writes");
        assert_eq!(std::fs::metadata(&path).expect("exists").len(), 44 + 960);
        let _ = std::fs::remove_dir_all(dir);
    }
}
