//! How long one frame of the timeline takes on the interface thread
//! (laying out, painting and tessellating), for growing projects.
//! Run with `cargo test --release -p ui-kit --test timeline_cost -- --ignored --nocapture`.

#![allow(clippy::unwrap_used)]

use std::time::Instant;
use ui_kit::timeline::{CacheBar, CacheRange, CacheState};
use ui_kit::{Theme, TimelineClip, TimelineData, TimelineEvent, TimelineMarker, TimelineTrack, TimelineView};

fn tracks(count: usize, clips_each: usize, clip_len: f64) -> Vec<TimelineTrack> {
    (0..count)
        .map(|t| TimelineTrack {
            id: t as u64,
            name: format!("Track {t}"),
            clips: (0..clips_each)
                .map(|c| TimelineClip {
                    id: (t * clips_each + c) as u64,
                    start: c as f64 * clip_len,
                    duration: clip_len * 0.9,
                    name: format!("clip {c}.mkv"),
                    info: String::new(),
                    selected: c == 3,
                    missing: false,
                    link: None,
                    waveform: None,
                    pictures: None,
                    loading: None,
                    sound: Default::default(),
                    keyframes: Default::default(),
                    speed_curve: None,
                    shows: Default::default(),
                    line: None,
                    slip_room: None,
                    plays_until: None,
                    transition_in: None,
                    graded: false,
                    fades: Default::default(),
                })
                .collect(),
            muted: false,
            solo: false,
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
            colour: None,
            locked: false,
            pan: 0.0,
            effects_on: false,
            meter: None,
            unsynced: false,
        })
        .collect()
}

#[test]
#[ignore = "a measurement, not a check; run by hand"]
fn timeline_frame_cost() {
    let theme = Theme::ASH;
    for (clips_each, zoomed_out) in [
        (25, false),
        (250, false),
        (2_500, false),
        (25, true),
        (250, true),
        (2_500, true),
    ] {
        let video = tracks(2, clips_each, 4.0);
        let audio = tracks(2, clips_each, 4.0);
        let length = clips_each as f64 * 4.0;
        // A cache range under every clip of a track, in three states.
        let cache: Vec<CacheRange> = (0..clips_each)
            .map(|c| CacheRange {
                start: c as f64 * 4.0,
                end: c as f64 * 4.0 + 3.6,
                state: [CacheState::Ready, CacheState::Preparing, CacheState::Waiting][c % 3],
            })
            .collect();
        let ctx = egui::Context::default();
        let mut view = TimelineView::default();
        if zoomed_out {
            view.fit(length, 1780.0);
        }
        let mut times = Vec::new();
        for frame in 0..60 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 400.0),
                )),
                ..Default::default()
            };
            let started = Instant::now();
            let output = ctx.run_ui(input, |ui| {
                let data = TimelineData {
                    video: &video,
                    audio: &audio,
                    length,
                    fps: 60.0,
                    playhead: frame as f64 * 0.5,
                    needle: Some(10.0),
                    region: None,
                    highlights: &[],
                    spans: &[],
                    markers: &[],
                    follow: !zoomed_out,
                    cache: CacheBar {
                        ranges: &cache,
                        about: "",
                    },
                    keyframe_eases: &["Linear", "Hold", "Smooth"],
                    roles: &[],
                };
                view.show(ui, &theme, &data, egui::vec2(1920.0, 400.0));
            });
            let _meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{:>6} clips{}: median {:.2} ms, worst {:.2} ms per frame",
            clips_each * 4,
            if zoomed_out { ", all on screen" } else { "" },
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

/// Every clip selected (Ctrl+A) and dragged along, with snapping on, on
/// timelines of growing size: each pointer move snaps the dragged edges and
/// asks whether a clip is in the way, which must not grow with the clips
/// off screen.
#[test]
#[ignore = "a measurement, not a check; run by hand"]
fn drag_everything_frame_cost() {
    let theme = Theme::ASH;
    let size = egui::vec2(1920.0, 400.0);
    for (clips_each, labels) in [(25, 10), (250, 100), (2_500, 1_000)] {
        let mut video = tracks(2, clips_each, 4.0);
        let mut audio = tracks(2, clips_each, 4.0);
        for c in video.iter_mut().chain(&mut audio).flat_map(|t| &mut t.clips) {
            c.selected = true;
            c.transition_in = Some(0.5);
        }
        let markers: Vec<TimelineMarker> = (0..labels)
            .map(|i| TimelineMarker {
                id: i as u64,
                at: i as f64 * 10.0 + 0.3,
                name: format!("Label {i}"),
                colour: egui::Color32::YELLOW,
                selected: false,
                until: None,
            })
            .collect();
        let length = clips_each as f64 * 4.0;
        let ctx = egui::Context::default();
        let mut view = TimelineView::default();
        // On the first clip of V1 (it runs from x = 140 to 356).
        let from = egui::pos2(200.0, 190.0);
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let started = Instant::now();
            let mut output = ctx.run_ui(input, |ui| {
                let data = TimelineData {
                    video: &video,
                    audio: &audio,
                    length,
                    fps: 60.0,
                    playhead: 2.0,
                    needle: Some(10.0),
                    region: None,
                    highlights: &[],
                    spans: &[],
                    markers: &markers,
                    follow: false,
                    cache: CacheBar {
                        ranges: &[],
                        about: "",
                    },
                    keyframe_eases: &["Linear", "Hold", "Smooth"],
                    roles: &[],
                };
                out = view.show(ui, &theme, &data, size);
            });
            let _meshes = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
            let took = started.elapsed().as_secs_f64() * 1000.0;
            // Nothing draws these frames.
            output.drop_without_applying_deltas();
            (took, out)
        };
        frame(vec![egui::Event::PointerMoved(from)]);
        frame(vec![egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let mut times = Vec::new();
        let mut to = from;
        for step in 1..=60 {
            to = from + egui::vec2(step as f32 * 3.0, 0.0);
            times.push(frame(vec![egui::Event::PointerMoved(to)]).0);
        }
        let (_, events) = frame(vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(
            events.iter().any(|e| matches!(e, TimelineEvent::Move { .. })),
            "the drag moved the clips: {events:?}"
        );
        times.sort_by(f64::total_cmp);
        println!(
            "{:>6} clips, all dragged: median {:.2} ms, worst {:.2} ms per frame",
            clips_each * 4,
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

/// Two tracks of a two-hour recording each, its waveform (100 peaks a
/// second) all on screen: the frame's cost follows the width, not the hours.
#[test]
#[ignore = "a measurement, not a check; run by hand"]
fn long_waveform_frame_cost() {
    let theme = Theme::ASH;
    let hours = 2.0 * 3600.0;
    let peaks: std::sync::Arc<[f32]> = (0..(hours * 100.0) as usize)
        .map(|i| ((i * 7_919) % 1_009) as f32 / 1_009.0)
        .collect();
    let mut audio = tracks(2, 1, hours);
    for t in &mut audio {
        for c in &mut t.clips {
            c.duration = hours;
            c.waveform = Some(ui_kit::ClipWaveform {
                id: egui::Id::unique("a two-hour recording"),
                peaks: std::sync::Arc::clone(&peaks),
                per_second: 100.0,
                source_in: 0.0,
                speed: 1.0,
                map: None,
            });
        }
    }
    let ctx = egui::Context::default();
    let mut view = TimelineView::default();
    view.fit(hours, 1780.0);
    let mut times = Vec::new();
    for frame in 0..60 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 400.0),
            )),
            ..Default::default()
        };
        let started = Instant::now();
        let output = ctx.run_ui(input, |ui| {
            let data = TimelineData {
                video: &[],
                audio: &audio,
                length: hours,
                fps: 60.0,
                playhead: frame as f64,
                needle: None,
                region: None,
                highlights: &[],
                spans: &[],
                markers: &[],
                follow: false,
                cache: CacheBar {
                    ranges: &[],
                    about: "",
                },
                keyframe_eases: &["Linear", "Hold", "Smooth"],
                roles: &[],
            };
            view.show(ui, &theme, &data, egui::vec2(1920.0, 400.0));
        });
        let _meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let first = times[0];
    times.sort_by(f64::total_cmp);
    println!(
        "two 2-hour waveforms: first frame {first:.2} ms, then median {:.2} ms, worst {:.2} ms",
        times[times.len() / 2],
        times[times.len() - 1]
    );
}
