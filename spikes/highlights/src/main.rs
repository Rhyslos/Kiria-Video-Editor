//! highlight-bench: compares ways of finding highlights in long game
//! recordings (docs/research/highlights.md).
//!
//! Usage:
//!   highlight-bench make-clips FOLDER [--seconds N] [--h264 | --small] [--only NAME] [--ffmpeg PROGRAM]
//!   highlight-bench score FOUND.csv PICKED.csv [--window SECONDS]
//!   highlight-bench [--decode NAME]... [--frames all,keys,nodeblock] [--threads N]
//!                   [--roles game,mic,chat] [--window SECONDS] [--limit SECONDS]
//!                   [--found FOLDER] RECORDING...
//!
//! For every recording it reads the sound of every audio stream once and
//! the picture once per decoding choice, runs every approach at three
//! sensitivities, writes the moments found at medium sensitivity to
//! `FOUND/<recording>.<approach>.csv`, and, when `<recording>.csv` lies
//! beside the recording (the moments a person picked), scores them. It
//! prints Markdown tables to paste into the research note.

mod audio;
mod detect;
mod media;
mod score;
mod synth;
mod video;

use detect::{Moment, Role, Sensitivity};
use score::{Score, Truth};
use std::path::{Path, PathBuf};
use std::time::Instant;

const USAGE: &str = "usage:
  highlight-bench make-clips FOLDER [--seconds N] [--h264 | --small] [--only NAME] [--ffmpeg PROGRAM]
  highlight-bench score FOUND.csv PICKED.csv [--window SECONDS]
  highlight-bench [--decode NAME]... [--frames all,keys,nodeblock] [--threads N]
                  [--roles game,mic,chat] [--window SECONDS] [--limit SECONDS]
                  [--found FOLDER] RECORDING...";

struct Options {
    /// `None` is software decoding.
    decode: Vec<Option<String>>,
    frames: Vec<media::Frames>,
    threads: i32,
    roles: Option<Vec<Role>>,
    window: f64,
    limit: f64,
    found: PathBuf,
    recordings: Vec<String>,
}

fn number<T: std::str::FromStr>(name: &str, value: Option<String>) -> Result<T, String> {
    value
        .ok_or(format!("{name} needs a value"))?
        .parse()
        .map_err(|_| format!("{name} needs a number"))
}

fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut o = Options {
        decode: Vec::new(),
        frames: vec![media::Frames::All, media::Frames::Keys],
        threads: 0,
        roles: None,
        window: 1.0,
        limit: f64::INFINITY,
        found: PathBuf::from("found"),
        recordings: Vec::new(),
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--decode" => {
                let name = args.next().ok_or("--decode needs a value")?;
                o.decode.push((name != "software").then_some(name));
            }
            "--frames" => {
                let list = args.next().ok_or("--frames needs a value")?;
                o.frames = list
                    .split(',')
                    .map(|f| match f {
                        "all" => Ok(media::Frames::All),
                        "keys" => Ok(media::Frames::Keys),
                        "nodeblock" => Ok(media::Frames::AllNoDeblock),
                        _ => Err(format!("--frames: \"{f}\" is not all, keys or nodeblock")),
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--threads" => o.threads = number("--threads", args.next())?,
            "--window" => o.window = number("--window", args.next())?,
            "--limit" => o.limit = number("--limit", args.next())?,
            "--found" => o.found = args.next().ok_or("--found needs a folder")?.into(),
            "--roles" => {
                let list = args.next().ok_or("--roles needs a value")?;
                let roles = list
                    .split(',')
                    .map(|r| Role::parse(r).ok_or(format!("--roles: \"{r}\" is not game, mic or chat")))
                    .collect::<Result<_, _>>()?;
                o.roles = Some(roles);
            }
            "-h" | "--help" => return Err(USAGE.into()),
            _ => o.recordings.push(a),
        }
    }
    if o.recordings.is_empty() {
        return Err(format!("give at least one recording\n{USAGE}"));
    }
    if o.decode.is_empty() {
        o.decode.push(cfg!(windows).then(|| "d3d11va".to_owned()));
    }
    Ok(o)
}

/// What each audio stream carries: from its title when it has one, else by
/// order (game, mic, chat), as recorders usually write them.
fn roles(info: &media::Info, given: &Option<Vec<Role>>) -> Vec<Role> {
    let by_order = [Role::Game, Role::Mic, Role::Chat];
    info.audio
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if let Some(r) = given.as_ref().and_then(|g| g.get(i)) {
                return *r;
            }
            let title = a.title.to_ascii_lowercase();
            if ["chat", "team", "party", "call"]
                .iter()
                .any(|w| title.contains(w))
            {
                Role::Chat
            } else if ["mic", "voice", "commentary"].iter().any(|w| title.contains(w)) {
                Role::Mic
            } else if ["game", "desktop", "system"].iter().any(|w| title.contains(w)) {
                Role::Game
            } else {
                by_order[i.min(2)]
            }
        })
        .collect()
}

struct Extractors(Vec<audio::Extractor>);

impl media::AudioSink for Extractors {
    fn begin(&mut self, stream: usize, time: f64) {
        self.0[stream].features.start = time;
    }
    fn samples(&mut self, stream: usize, samples: &[f32]) {
        self.0[stream].push(samples);
    }
}

/// One approach's result on one recording at one sensitivity.
struct Run {
    approach: String,
    sensitivity: Sensitivity,
    moments: Vec<Moment>,
    /// Wall time of the passes it needs, plus its own detection time.
    seconds: f64,
}

/// A video pass and what it found.
struct VideoPass {
    label: String,
    frames: media::Frames,
    stats: media::VideoStats,
    changes: Vec<video::Change>,
    seconds: f64,
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64())
}

fn video_pass(
    path: &str,
    o: &Options,
    hardware: &Option<String>,
    frames: media::Frames,
    to: f64,
) -> Result<VideoPass, String> {
    let options = media::VideoOptions {
        hardware: if frames == media::Frames::All {
            hardware.clone()
        } else {
            None
        },
        threads: o.threads,
        frames,
    };
    let mut tracker = video::Tracker::default();
    let (stats, seconds) = timed(|| media::read_video(path, &options, None, to, &mut |t| tracker.push(&t)));
    let stats = stats?;
    let label = match frames {
        media::Frames::All => format!("every frame, {}", stats.method),
        media::Frames::AllNoDeblock => "every frame, no deblocking".into(),
        media::Frames::Keys => "key frames".into(),
    };
    Ok(VideoPass {
        label,
        frames,
        stats,
        changes: tracker.changes,
        seconds,
    })
}

/// The key-frame approach: neighbouring key frames with different colours,
/// then only those stretches decoded frame by frame to find the cut.
fn key_frame_runs(path: &str, o: &Options, pass: &VideoPass) -> Result<Vec<Run>, String> {
    let mut refined: Vec<((f64, f64), Option<Moment>)> = Vec::new();
    let mut extra = 0.0;
    for (from, to) in detect::key_cuts(&pass.changes, Sensitivity::High) {
        let options = media::VideoOptions {
            hardware: None,
            threads: o.threads,
            frames: media::Frames::All,
        };
        let mut tracker = video::Tracker::default();
        let (r, s) =
            timed(|| media::read_video(path, &options, Some(from), to + 0.001, &mut |t| tracker.push(&t)));
        r?;
        extra += s;
        refined.push(((from, to), detect::refine(&tracker.changes)));
    }
    Ok(Sensitivity::ALL
        .iter()
        .map(|&s| {
            let wanted = detect::key_cuts(&pass.changes, s);
            let moments = refined
                .iter()
                .filter(|(pair, _)| wanted.contains(pair))
                .filter_map(|(_, m)| m.clone())
                .collect();
            Run {
                approach: "C key frames, then the stretch".into(),
                sensitivity: s,
                moments,
                seconds: pass.seconds + extra,
            }
        })
        .collect())
}

/// Everything about one recording.
struct Report {
    name: String,
    info: media::Info,
    roles: Vec<Role>,
    seconds: f64,
    audio_seconds: f64,
    passes: Vec<(String, f64, u64)>,
    runs: Vec<Run>,
    truth: Option<Vec<Truth>>,
}

fn analyse(path: &str, o: &Options) -> Result<Report, String> {
    let info = media::info(path)?;
    let roles = roles(&info, &o.roles);
    let length = if info.duration > 0.0 {
        info.duration.min(o.limit)
    } else {
        o.limit
    };
    let mut runs = Vec::new();

    // The sound: every stream in one pass.
    let mut sink = Extractors(info.audio.iter().map(|_| audio::Extractor::new()).collect());
    let (read, audio_seconds) = timed(|| media::read_audio(path, length, &mut sink));
    let read = read?;
    let seconds = if length.is_finite() { length } else { read };
    let features: Vec<(Role, &audio::Features)> = roles
        .iter()
        .copied()
        .zip(sink.0.iter().map(|e| &e.features))
        .collect();
    for s in Sensitivity::ALL {
        let (moments, t) = timed(|| {
            detect::merge(
                &features
                    .iter()
                    .map(|(r, f)| detect::loudness(f, *r, s))
                    .collect::<Vec<_>>(),
                1.5,
            )
        });
        runs.push(Run {
            approach: "A loudness".into(),
            sensitivity: s,
            moments,
            seconds: audio_seconds + t,
        });
        let (moments, t) = timed(|| detect::flux(&features, s));
        runs.push(Run {
            approach: "B flux, combined".into(),
            sensitivity: s,
            moments,
            seconds: audio_seconds + t,
        });
    }

    // The picture: once per decoding choice.
    let mut passes = Vec::new();
    let mut first_all: Option<usize> = None;
    let mut keys: Option<usize> = None;
    for &frames in &o.frames {
        let decoders: Vec<Option<String>> = if frames == media::Frames::All {
            o.decode.clone()
        } else {
            vec![None]
        };
        for hardware in &decoders {
            eprintln!(
                "  video: {frames:?} {}",
                hardware.as_deref().unwrap_or("software")
            );
            let pass = video_pass(path, o, hardware, frames, seconds)?;
            passes.push(pass);
        }
    }
    for (i, pass) in passes.iter().enumerate() {
        if pass.frames == media::Frames::Keys {
            keys = keys.or(Some(i));
            runs.extend(key_frame_runs(path, o, pass)?);
        } else {
            first_all = first_all.or(Some(i));
            for s in Sensitivity::ALL {
                let (moments, t) = timed(|| detect::video(&pass.changes, s));
                runs.push(Run {
                    approach: format!("C video, {}", pass.label),
                    sensitivity: s,
                    moments,
                    seconds: pass.seconds + t,
                });
            }
        }
    }

    // Combinations: sound and the first full video pass, or the key frames.
    let combine = |runs: &[Run], audio: &str, video: &str, name: &str| -> Vec<Run> {
        Sensitivity::ALL
            .iter()
            .filter_map(|&s| {
                let a = runs.iter().find(|r| r.approach == audio && r.sensitivity == s)?;
                let v = runs.iter().find(|r| r.approach == video && r.sensitivity == s)?;
                Some(Run {
                    approach: name.into(),
                    sensitivity: s,
                    moments: detect::merge(&[a.moments.clone(), v.moments.clone()], 1.5),
                    seconds: a.seconds + v.seconds,
                })
            })
            .collect()
    };
    if let Some(i) = first_all {
        let v = format!("C video, {}", passes[i].label);
        let mut more = combine(&runs, "A loudness", &v, "A + C");
        more.extend(combine(&runs, "B flux, combined", &v, "B + C"));
        runs.extend(more);
    }
    if keys.is_some() {
        let more = combine(
            &runs,
            "A loudness",
            "C key frames, then the stretch",
            "A + C key frames",
        );
        runs.extend(more);
    }

    let truth_path = Path::new(path).with_extension("csv");
    let truth = match std::fs::read_to_string(&truth_path) {
        Ok(text) => Some(score::parse_truth(&text).map_err(|e| format!("{}: {e}", truth_path.display()))?),
        Err(_) => None,
    };
    let truth = truth.map(|t| t.into_iter().filter(|t| t.time <= seconds).collect());
    let name = Path::new(path)
        .file_stem()
        .map_or(path.to_owned(), |s| s.to_string_lossy().into_owned());
    let passes = passes
        .iter()
        .map(|p| (p.label.clone(), p.seconds, p.stats.frames))
        .collect();
    Ok(Report {
        name,
        info,
        roles,
        seconds,
        audio_seconds,
        passes,
        runs,
        truth,
    })
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn pct(x: f64) -> String {
    format!("{:.0}%", 100.0 * x)
}

fn speed(seconds: f64, wall: f64) -> String {
    if wall <= 0.0 {
        "-".into()
    } else {
        format!("{:.1}×", seconds / wall)
    }
}

fn print_report(r: &Report, o: &Options) {
    println!("## {}\n", r.name);
    let audio: Vec<String> = r
        .info
        .audio
        .iter()
        .zip(&r.roles)
        .map(|(a, role)| {
            let title = if a.title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", a.title)
            };
            format!(
                "{}{title} {} {}ch as {}",
                a.number + 1,
                a.codec,
                a.channels,
                role.name()
            )
        })
        .collect();
    println!(
        "{} {}×{} {:.0} fps, {:.0} s analysed; sound: {}\n",
        r.info.video,
        r.info.width,
        r.info.height,
        r.info.fps,
        r.seconds,
        audio.join(", ")
    );
    println!("| Pass | Seconds | Speed | Frames |\n| --- | ---: | ---: | ---: |");
    println!(
        "| sound, every stream | {:.1} | {} | |",
        r.audio_seconds,
        speed(r.seconds, r.audio_seconds)
    );
    for (label, s, frames) in &r.passes {
        println!(
            "| picture, {label} | {s:.1} | {} | {frames} |",
            speed(r.seconds, *s)
        );
    }
    println!();
    let Some(truth) = &r.truth else {
        println!(
            "No `{}.csv` beside the recording, so nothing is scored. Found at medium:\n",
            r.name
        );
        println!("| Approach | Found | Per hour |\n| --- | ---: | ---: |");
        for run in r.runs.iter().filter(|x| x.sensitivity == Sensitivity::Medium) {
            println!(
                "| {} | {} | {:.0} |",
                run.approach,
                run.moments.len(),
                run.moments.len() as f64 * 3600.0 / r.seconds
            );
        }
        println!();
        return;
    };
    println!(
        "{} picked moments. Medium sensitivity, ±{} s:\n",
        truth.len(),
        o.window
    );
    println!("| Approach | Found | Hits | False | Missed | Precision | Recall | F1 | Speed |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for run in r.runs.iter().filter(|x| x.sensitivity == Sensitivity::Medium) {
        let s = score::score(&run.moments, truth, o.window, r.seconds);
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {:.2} | {} |",
            run.approach,
            s.found,
            s.hits,
            s.false_positives(),
            s.missed(),
            pct(s.precision()),
            pct(s.recall()),
            s.f1(),
            speed(r.seconds, run.seconds)
        );
    }
    println!();
}

/// Scores summed over every recording that has picked moments.
fn print_totals(reports: &[Report], o: &Options) {
    let scored: Vec<&Report> = reports.iter().filter(|r| r.truth.is_some()).collect();
    if scored.is_empty() {
        return;
    }
    let mut approaches: Vec<String> = Vec::new();
    for r in &scored {
        for run in &r.runs {
            if !approaches.contains(&run.approach) {
                approaches.push(run.approach.clone());
            }
        }
    }
    let mut kinds: Vec<String> = Vec::new();
    for r in &scored {
        for t in r.truth.iter().flatten() {
            let k = if t.kind.is_empty() {
                "any".to_owned()
            } else {
                t.kind.clone()
            };
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
    }
    let total = |approach: &str, s: Sensitivity, window: f64| -> (Score, f64, f64) {
        let mut sum = Score::default();
        let (mut media_s, mut wall) = (0.0, 0.0);
        for r in &scored {
            if let (Some(run), Some(truth)) = (
                r.runs
                    .iter()
                    .find(|x| x.approach == approach && x.sensitivity == s),
                &r.truth,
            ) {
                sum.add(&score::score(&run.moments, truth, window, r.seconds));
                media_s += r.seconds;
                wall += run.seconds;
            }
        }
        (sum, media_s, wall)
    };
    println!("## All recordings together\n");
    println!(
        "{} recordings, {:.0} minutes. Medium sensitivity, ±{} s; recall by kind of moment:\n",
        scored.len(),
        scored.iter().map(|r| r.seconds).sum::<f64>() / 60.0,
        o.window
    );
    print!("| Approach | Precision | Recall | F1 | False per hour |");
    for k in &kinds {
        print!(" {k} |");
    }
    println!(" Speed |");
    print!("| --- | ---: | ---: | ---: | ---: |");
    for _ in &kinds {
        print!(" ---: |");
    }
    println!(" ---: |");
    for a in &approaches {
        let (s, media_s, wall) = total(a, Sensitivity::Medium, o.window);
        print!(
            "| {a} | {} | {} | {:.2} | {:.0} |",
            pct(s.precision()),
            pct(s.recall()),
            s.f1(),
            s.false_per_hour()
        );
        for k in &kinds {
            print!(" {} |", s.kind_recall(k));
        }
        println!(" {} |", speed(media_s, wall));
    }
    println!("\nSensitivity, and a wider window (±3 s, for moments marked by hand):\n");
    println!("| Approach | Sensitivity | Precision | Recall | F1 | False per hour | F1 ±3 s |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: |");
    for a in &approaches {
        for s in Sensitivity::ALL {
            let (sc, _, _) = total(a, s, o.window);
            let (wide, _, _) = total(a, s, 3.0);
            println!(
                "| {a} | {} | {} | {} | {:.2} | {:.0} | {:.2} |",
                s.name(),
                pct(sc.precision()),
                pct(sc.recall()),
                sc.f1(),
                sc.false_per_hour(),
                wide.f1()
            );
        }
    }
    println!();
}

fn write_found(r: &Report, o: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&o.found).map_err(|e| format!("making {}: {e}", o.found.display()))?;
    for run in r.runs.iter().filter(|x| x.sensitivity == Sensitivity::Medium) {
        let path = o.found.join(format!("{}.{}.csv", r.name, slug(&run.approach)));
        std::fs::write(&path, score::write_found(&run.moments))
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    Ok(())
}

fn make_clips(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut folder = None;
    let mut seconds = 180.0;
    let mut encoding = synth::Encoding::H265;
    let mut only: Option<String> = None;
    let mut ffmpeg = "ffmpeg".to_owned();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seconds" => seconds = number("--seconds", args.next())?,
            "--small" => encoding = synth::Encoding::Small,
            "--h264" => encoding = synth::Encoding::H264,
            "--only" => only = Some(args.next().ok_or("--only needs a clip name")?),
            "--ffmpeg" => ffmpeg = args.next().ok_or("--ffmpeg needs a program")?,
            _ => folder = Some(PathBuf::from(a)),
        }
    }
    let folder = folder.ok_or(format!("make-clips needs a folder\n{USAGE}"))?;
    for plan in synth::plans() {
        if only.as_ref().is_some_and(|o| o != plan.name) {
            continue;
        }
        if folder.join(format!("{}.mkv", plan.name)).exists() {
            eprintln!("have {}", plan.name);
            continue;
        }
        eprintln!("making {} ({seconds} s, {encoding:?})", plan.name);
        let (r, t) = timed(|| synth::make(&plan, &folder, seconds, encoding, &ffmpeg));
        r?;
        eprintln!("  took {t:.0} s");
    }
    Ok(())
}

fn score_files(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut files = Vec::new();
    let mut window = 1.0;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--window" => window = number("--window", args.next())?,
            _ => files.push(a),
        }
    }
    let [found, picked] = files.as_slice() else {
        return Err(format!("score needs two files\n{USAGE}"));
    };
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("reading {p}: {e}"));
    let found = score::parse_found(&read(found)?)?;
    let truth = score::parse_truth(&read(picked)?)?;
    let s = score::score(&found, &truth, window, 0.0);
    println!(
        "picked {}, found {}, hits {}, false {}, missed {}; precision {}, recall {}, F1 {:.2} (±{window} s)",
        s.picked,
        s.found,
        s.hits,
        s.false_positives(),
        s.missed(),
        pct(s.precision()),
        pct(s.recall()),
        s.f1()
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("make-clips") => return make_clips(args.skip(1)),
        Some("score") => return score_files(args.skip(1)),
        _ => {}
    }
    let o = options(args)?;
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    println!("# Highlight finder measurements\n");
    println!(
        "{threads} hardware threads; decoder threads: {}; video decoding: {}.\n",
        if o.threads == 0 {
            "all".to_owned()
        } else {
            o.threads.to_string()
        },
        o.decode
            .iter()
            .map(|d| d.as_deref().unwrap_or("software"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut reports = Vec::new();
    for path in &o.recordings {
        eprintln!("analysing {path}");
        match analyse(path, &o) {
            Ok(r) => {
                print_report(&r, &o);
                write_found(&r, &o)?;
                reports.push(r);
            }
            Err(e) => println!("## {path}\n\nCould not be analysed: {e}\n"),
        }
    }
    print_totals(&reports, &o);
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(titles: &[&str]) -> media::Info {
        media::Info {
            duration: 60.0,
            video: "hevc".into(),
            width: 2560,
            height: 1440,
            fps: 60.0,
            audio: titles
                .iter()
                .enumerate()
                .map(|(number, t)| media::AudioStream {
                    number,
                    title: (*t).into(),
                    codec: "aac".into(),
                    channels: 2,
                })
                .collect(),
        }
    }

    #[test]
    fn roles_come_from_titles_then_order() {
        assert_eq!(
            roles(&info(&["", "", ""]), &None),
            vec![Role::Game, Role::Mic, Role::Chat]
        );
        assert_eq!(
            roles(&info(&["Microphone", "Voice chat", "Desktop audio"]), &None),
            vec![Role::Mic, Role::Chat, Role::Game]
        );
        assert_eq!(
            roles(&info(&["", ""]), &Some(vec![Role::Mic])),
            vec![Role::Mic, Role::Mic]
        );
    }

    #[test]
    fn options_read() {
        let args = [
            "--decode", "software", "--frames", "keys,all", "--roles", "mic,game", "a.mkv",
        ];
        let o = options(args.iter().map(|s| s.to_string())).expect("reads");
        assert_eq!(o.decode, vec![None]);
        assert_eq!(o.frames, vec![media::Frames::Keys, media::Frames::All]);
        assert_eq!(o.roles, Some(vec![Role::Mic, Role::Game]));
        assert_eq!(o.recordings, vec!["a.mkv".to_owned()]);
        assert!(options(["--frames", "some"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn slugs() {
        assert_eq!(
            slug("C video, every frame, d3d11va"),
            "c-video-every-frame-d3d11va"
        );
        assert_eq!(slug("A + C"), "a-c");
    }
}
