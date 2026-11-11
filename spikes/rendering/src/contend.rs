//! Does composing pictures disturb the interface? A loop stands in for the
//! interface: every frame it draws the newest composed picture onto a
//! stand-in window and hands that to the card, as the editor's window does.
//! Pictures are composed meanwhile by nobody, by the same thread before its
//! own drawing (approach B), or by a worker on the same device (approach C).

use crate::compose::{Compositor, Place, Source, Targets, Way, begin, window};
use crate::gpu::{Done, Gpu};
use crate::measure::{median, ms, worst};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composing {
    Nobody,
    SameThread,
    Worker,
}

impl Composing {
    pub const ALL: [Composing; 3] = [Composing::Nobody, Composing::SameThread, Composing::Worker];

    pub fn name(self) -> &'static str {
        match self {
            Composing::Nobody => "nobody (the interface alone)",
            Composing::SameThread => "B the interface thread",
            Composing::Worker => "C a worker",
        }
    }
}

pub struct Outcome {
    pub frames: usize,
    /// Frames the card finished later than one frame's time after the frame began.
    pub late: usize,
    /// Frames not seen finished within a quarter of a second.
    pub lost: usize,
    /// The interface thread's own time per frame (median, worst), in ms:
    /// everything until its commands are handed over.
    pub thread: (f64, f64),
    /// From the frame's start until the card has finished it (median, worst).
    pub finished: (f64, f64),
    pub pictures: usize,
    /// The composing thread's own time per picture (median, worst), in ms.
    pub composing: Option<(f64, f64)>,
}

/// As long as a frame or picture is waited for before it counts as lost.
const LIMIT: Duration = Duration::from_millis(250);

pub struct Run<'a> {
    pub gpu: &'a Gpu,
    pub compositor: &'a Compositor,
    pub sources: &'a [Source],
    pub frame: (u32, u32),
    pub frames_a_second: f64,
    pub pictures_a_second: f64,
    pub seconds: f64,
}

pub fn run(run: &Run<'_>, composing: Composing) -> Result<Outcome, String> {
    let Run { gpu, compositor, .. } = *run;
    // Two frames in turn, so the one being shown is never the one being drawn.
    let targets = [
        Targets::new(gpu, compositor, Way::Float, run.frame),
        Targets::new(gpu, compositor, Way::Float, run.frame),
    ];
    let window = window(gpu, run.frame).create_view(&wgpu::TextureViewDescriptor::default());
    let places = Place::standard(run.sources.len());
    let layers: Vec<(&Source, Place)> = run.sources.iter().zip(places).collect();
    let compose = |into: usize| {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        compositor.draw(gpu, &mut encoder, Way::Float, &targets[into], &layers, None, None);
        let done = Done::of(&encoder);
        gpu.queue.submit([encoder.finish()]);
        done
    };
    // Something to show from the first frame on, and everything warm.
    compose(0);
    compose(1);
    gpu.wait()?;

    let newest = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let frame_time = Duration::from_secs_f64(1.0 / run.frames_a_second);
    let picture_time = Duration::from_secs_f64(1.0 / run.pictures_a_second);
    let length = Duration::from_secs_f64(run.seconds);

    let (mut thread, mut finished) = (Vec::new(), Vec::new());
    let (mut late, mut lost) = (0, 0);
    let mut composed: Vec<f64> = Vec::new();

    std::thread::scope(|scope| {
        let worker = (composing == Composing::Worker).then(|| {
            scope.spawn(|| {
                let mut times = Vec::new();
                let mut next = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    sleep_until(next);
                    let start = Instant::now();
                    let into = 1 - newest.load(Ordering::Acquire);
                    let done = compose(into);
                    newest.store(into, Ordering::Release);
                    times.push(ms(start));
                    // Never more pictures than the card has finished.
                    done.watch(gpu, LIMIT, Some(Duration::from_micros(300)));
                    next = (next + picture_time).max(Instant::now());
                }
                times
            })
        });

        let start = Instant::now();
        let mut next = start;
        let mut next_picture = start;
        while start.elapsed() < length {
            sleep_until(next);
            let frame_start = Instant::now();
            if composing == Composing::SameThread && frame_start >= next_picture {
                let into = 1 - newest.load(Ordering::Acquire);
                compose(into);
                newest.store(into, Ordering::Release);
                composed.push(ms(frame_start));
                next_picture = (next_picture + picture_time).max(frame_start);
            }
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = begin(&mut encoder, &window, Some(wgpu::Color::BLACK), None);
                compositor.show_in(&mut pass, &targets[newest.load(Ordering::Acquire)]);
            }
            let done = Done::of(&encoder);
            gpu.queue.submit([encoder.finish()]);
            thread.push(ms(frame_start));
            match done.watch(gpu, LIMIT, None) {
                Some(at) => {
                    let took = at.duration_since(frame_start);
                    finished.push(took.as_secs_f64() * 1000.0);
                    late += usize::from(took > frame_time);
                }
                None => lost += 1,
            }
            next = (next + frame_time).max(Instant::now());
        }
        stop.store(true, Ordering::Relaxed);
        if let Some(worker) = worker {
            composed = worker.join().unwrap_or_default();
        }
    });
    gpu.wait()?;

    Ok(Outcome {
        frames: thread.len(),
        late,
        lost,
        thread: (median(thread.clone()), worst(&thread)),
        finished: (median(finished.clone()), worst(&finished)),
        pictures: composed.len(),
        composing: (!composed.is_empty()).then(|| (median(composed.clone()), worst(&composed))),
    })
}

/// Sleeps most of the way and spins the rest, to start on time.
fn sleep_until(at: Instant) {
    let spin = Duration::from_micros(1500);
    let now = Instant::now();
    if at > now + spin {
        std::thread::sleep(at - now - spin);
    }
    while Instant::now() < at {
        std::hint::spin_loop();
    }
}
