//! How long a jump takes to put a picture on screen, with and without a
//! proxy, through the preview worker the viewer uses; and how fast a proxy
//! builds. A measurement for `docs/research/caching.md`, run by hand on a
//! recording named by `JUMP_CLIP`:
//!
//! ```text
//! JUMP_CLIP=clip.mp4 cargo test -p engine --test jumps -- --ignored --nocapture
//! ```
//!
//! Without `JUMP_CLIP` it does nothing.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used)]

use cache::proxy::{ProxyMode, Shelf};
use engine::{Convert, Preview};
use media::Decoding;
use media::proxy::Steps;
use project::{Command, Project, Time};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Jumps per way of showing.
const JUMPS: usize = 30;
/// The size pictures are asked for at, as in the spike.
const SIZE: (u32, u32) = (1280, 720);

/// Times for one way of showing: from a jump to its first picture, and to
/// its exact picture (the same when nothing came first).
struct Times {
    first: Vec<Duration>,
    exact: Vec<Duration>,
    /// What the first pictures were, as the viewer's corner says.
    said: Vec<String>,
}

fn median_and_worst(times: &[Duration]) -> String {
    let mut ms: Vec<f64> = times.iter().map(|t| t.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let median = ms.get(ms.len() / 2).copied().unwrap_or(0.0);
    let worst = ms.last().copied().unwrap_or(0.0);
    format!("{median:.1} / {worst:.1} ms")
}

/// The same pseudo-random moments every run (seconds), spread over `length`.
fn targets(length: f64) -> Vec<f64> {
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..JUMPS)
        .map(|_| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 * (length - 0.5).max(0.1)
        })
        .collect()
}

fn jumps(project: &Project, shelf: Option<Arc<Shelf>>, length: f64) -> Times {
    let mut preview = Preview::start(
        Decoding::Best,
        Convert::WhileDrawing { largest_side: 8192 },
        None,
        || {},
    );
    preview.set_proxies(shelf);
    let seq = project.active_sequence;
    let mut times = Times {
        first: Vec::new(),
        exact: Vec::new(),
        said: Vec::new(),
    };
    for target in targets(length) {
        let at = Time::from_ratio((target * 1000.0) as i64, 1000).expect("a time");
        let asked = Instant::now();
        preview.show(project, seq, at, SIZE.0, SIZE.1);
        let mut first = None;
        loop {
            let picture = preview.wait(Duration::from_secs(30)).expect("a picture in time");
            let now = asked.elapsed();
            if first.is_none() {
                first = Some(now);
                times.said.push(picture.method.clone().unwrap_or_default());
            }
            if !picture.stand_in {
                times.exact.push(now);
                break;
            }
        }
        times.first.extend(first);
    }
    times
}

#[test]
#[ignore = "a measurement, run by hand with JUMP_CLIP"]
fn jump_to_first_picture() {
    let Ok(clip) = std::env::var("JUMP_CLIP") else {
        eprintln!("JUMP_CLIP names no recording: nothing measured");
        return;
    };
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(std::path::Path::new(&clip)).expect("a recording");
    let length = media.duration.as_secs_f64();
    let fingerprint = media.fingerprint.clone();
    let stream = media.info.video.as_ref().expect("a picture stream").index;
    let project = Command::Import { sequence: seq, media }
        .apply(&project)
        .expect("imported");

    let without = jumps(&project, None, length);

    let folder = tempfile::tempdir().expect("a folder");
    let shelf = Shelf::new(folder.path(), ProxyMode::Auto);
    let built_at = Instant::now();
    let steps = Steps {
        cancelled: &|| false,
        progress: &|_| {},
        pause: &|| {},
    };
    let written = shelf
        .build(
            std::path::Path::new(&clip),
            stream,
            length,
            &fingerprint,
            Decoding::Best,
            1,
            &steps,
        )
        .expect("a proxy");
    let build = built_at.elapsed().as_secs_f64();
    let bytes = shelf.bytes();
    let judged =
        cache::proxy::judge(std::path::Path::new(&clip), stream, length, Decoding::Best).expect("judged");

    let auto = jumps(&project, Some(Arc::clone(&shelf)), length);
    shelf.set_mode(ProxyMode::On);
    let on = jumps(&project, Some(Arc::clone(&shelf)), length);

    println!("clip: {clip}, {length:.1} s");
    println!(
        "Auto's judgement: key frames up to {:.2} s apart, a typical jump {} ms: {}",
        judged.longest_gap,
        judged.typical_jump.as_millis(),
        if judged.needs_proxy { "proxy" } else { "no proxy" }
    );
    println!(
        "proxy: {} frames with {}, built in {build:.1} s = {:.2} times faster than playing, {:.1} MB a minute",
        written.frames,
        written.encoder,
        length / build,
        bytes as f64 / 1e6 / (length / 60.0)
    );
    for (name, times) in [
        ("no proxy (key frame first)", &without),
        ("proxy, Auto", &auto),
        ("proxy, On", &on),
    ] {
        println!(
            "{name}: first picture {} · exact picture {} · first pictures: {:?}",
            median_and_worst(&times.first),
            median_and_worst(&times.exact),
            {
                let mut kinds = times.said.clone();
                kinds.sort();
                kinds.dedup();
                kinds
            }
        );
    }
}
