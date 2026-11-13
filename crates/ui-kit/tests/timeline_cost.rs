//! How long one frame of the timeline takes on the interface thread
//! (laying out, painting and tessellating), for growing projects.
//! Run with `cargo test --release -p ui-kit --test timeline_cost -- --ignored --nocapture`.

#![allow(clippy::unwrap_used)]

use std::time::Instant;
use ui_kit::timeline::{CacheBar, CacheRange, CacheState};
use ui_kit::{Theme, TimelineClip, TimelineData, TimelineTrack, TimelineView};

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
                })
                .collect(),
            muted: false,
            solo: false,
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
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
