//! Finding a steadier view for a shaky clip (the Stabilise effect). Each
//! pair of frames, read small and in grey, gives how the picture moved:
//! a grid of boxes is followed from one to the next (`track::step`) and
//! the middle of their moves is the camera's (things moving on their own
//! are outvoted). Added up, those moves are the camera's path; smoothed
//! over a stretch set by the effect's Smoothing, it is the path the view
//! keeps to. Where the view's middle must be at each frame for that, and
//! how far in it must go so the edges stay out of sight, are what it
//! finds: keyframes and a zoom for the effect.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use crate::track::{Grey, PixelBox, Place, step, thinned};
use media::{Decoding, VideoDecoder};
use project::{Rate, Time};

/// The widest a frame is read at.
const READ_WIDTH: u32 = 320;

/// Boxes followed across and down.
const GRID: (i64, i64) = (4, 3);

/// How the picture moved from `before` to `picture`, in its pixels: the
/// middle of the moves of a grid of boxes. `None` when too few could be
/// followed (a cut, a blank picture).
pub fn moved(before: &Grey, picture: &Grey) -> Option<(f64, f64)> {
    let (w, h) = (before.width as i64, before.height as i64);
    let (bw, bh) = ((w / 8).max(8), (h / 6).max(8));
    let mut across = Vec::new();
    let mut down = Vec::new();
    for gy in 0..GRID.1 {
        for gx in 0..GRID.0 {
            let x = (w * (2 * gx + 1)) / (2 * GRID.0) - bw / 2;
            let y = (h * (2 * gy + 1)) / (2 * GRID.1) - bh / 2;
            let b = PixelBox { x, y, w: bw, h: bh };
            if let Some(now) = step(before, b, picture) {
                across.push((now.x - b.x) as f64);
                down.push((now.y - b.y) as f64);
            }
        }
    }
    (across.len() >= 3).then(|| (middle(&mut across), middle(&mut down)))
}

/// The middle value.
fn middle(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// `path` smoothed: each point the weighted mean of those around it, by a
/// bell curve `spread` points wide each side.
pub fn smoothed(path: &[(f64, f64)], spread: f64) -> Vec<(f64, f64)> {
    let reach = (spread * 3.0).ceil() as usize;
    (0..path.len())
        .map(|i| {
            let (from, to) = (i.saturating_sub(reach), (i + reach + 1).min(path.len()));
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for (j, (x, y)) in path.iter().enumerate().take(to).skip(from) {
                let d = (j as f64 - i as f64) / spread.max(1e-6);
                let weight = (-0.5 * d * d).exp();
                sx += x * weight;
                sy += y * weight;
                sw += weight;
            }
            (sx / sw, sy / sw)
        })
        .collect()
}

/// What the analysis finds: where the view's middle is at each moment
/// (0 to 1 of the picture, a half being the middle), thinned, and the zoom
/// that leaves room for it (0 to 1, as the effect keeps it).
#[derive(Debug, Clone, PartialEq)]
pub struct Steadied {
    pub places: Vec<Place>,
    pub zoom: f64,
}

/// The view for a camera that went along `path` (in shares of the
/// picture, one point per frame at `times`), smoothed by `smoothing` (0 to
/// 1) at `fps` frames a second.
pub fn view(times: &[Time], path: &[(f64, f64)], smoothing: f64, fps: f64) -> Steadied {
    let spread = fps * (0.1 + 1.9 * smoothing.clamp(0.0, 1.0));
    let smooth = smoothed(path, spread);
    // The view moves with what the camera did beyond its smoothed path.
    let off: Vec<(f64, f64)> = path
        .iter()
        .zip(&smooth)
        .map(|(p, s)| (p.0 - s.0, p.1 - s.1))
        .collect();
    let most = off.iter().fold(0.0f64, |m, (x, y)| m.max(x.abs()).max(y.abs()));
    // Room for the view's middle to move `most` each way: scale such that
    // 0.5 - 0.5 / scale ≥ most; the effect zooms up to half as large again.
    let scale = (1.0 / (1.0 - 2.0 * most.min(0.16))).max(1.02);
    let zoom = ((scale - 1.0) / 0.5).clamp(0.0, 1.0);
    let places: Vec<Place> = times
        .iter()
        .zip(&off)
        .map(|(t, (x, y))| (*t, (0.5 + x, 0.5 + y)))
        .collect();
    Steadied {
        places: thinned(&places, 0.0005),
        zoom,
    }
}

/// What to analyse: a stretch of a file's picture.
#[derive(Debug, Clone)]
pub struct Analyse {
    pub path: PathBuf,
    pub stream: u32,
    pub rotation: u16,
    pub rate: Rate,
    pub from: Time,
    pub to: Time,
    /// The effect's Smoothing, 0 to 1.
    pub smoothing: f64,
}

/// What a running analysis says.
#[derive(Debug, Clone, PartialEq)]
pub enum Analysed {
    Going(f32),
    Done(Steadied),
    /// Stopped before the end: nothing is set.
    Stopped,
    Failed(String),
}

/// An analysis on a thread of its own.
#[derive(Debug)]
pub struct Analysing {
    news: Receiver<Analysed>,
    stop: Arc<AtomicBool>,
}

impl Analysing {
    pub fn start(asked: Analyse) -> diag::Result<Analysing> {
        let (send, news) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("stabilise".into())
            .spawn(move || {
                let said = match run(&asked, &stopped, &|done| {
                    let _ = send.send(Analysed::Going(done));
                }) {
                    Ok(Some(found)) => Analysed::Done(found),
                    Ok(None) => Analysed::Stopped,
                    Err(err) => Analysed::Failed(err.to_string()),
                };
                let _ = send.send(said);
            })
            .map_err(|err| {
                diag::AppError::new("The analysis couldn't start")
                    .why(format!("A thread for it couldn't be made: {err}."))
            })?;
        Ok(Analysing { news, stop })
    }

    /// The newest news, if any came.
    pub fn news(&self) -> Option<Analysed> {
        let mut newest = None;
        while let Ok(news) = self.news.try_recv() {
            let done = !matches!(news, Analysed::Going(_));
            newest = Some(news);
            if done {
                break;
            }
        }
        newest
    }

    /// Stops it; nothing is set.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Analysing {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(asked: &Analyse, stop: &AtomicBool, going: &dyn Fn(f32)) -> diag::Result<Option<Steadied>> {
    let mut decoder = VideoDecoder::open(&asked.path, asked.stream, asked.rotation, Decoding::Software)?;
    let rate = asked.rate;
    let (first, last) = (
        rate.frame_at(asked.from),
        rate.frame_at(asked.to).max(rate.frame_at(asked.from)),
    );
    let read = |decoder: &mut VideoDecoder, frame: i64| -> diag::Result<Grey> {
        let picture = decoder.frame_at(rate.frame_start(frame).as_secs_f64(), READ_WIDTH, READ_WIDTH)?;
        Ok(Grey::of_rgba(picture.width, picture.height, &picture.rgba))
    };
    let mut before = read(&mut decoder, first)?;
    let (w, h) = (before.width as f64, before.height as f64);
    let mut times = vec![rate.frame_start(first)];
    let mut path = vec![(0.0, 0.0)];
    let count = (last - first).max(1);
    for frame in first + 1..=last {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let picture = read(&mut decoder, frame)?;
        let step = match (picture.width, picture.height) == (before.width, before.height) {
            true => moved(&before, &picture).unwrap_or((0.0, 0.0)),
            false => (0.0, 0.0),
        };
        let (x, y) = path.last().copied().unwrap_or((0.0, 0.0));
        path.push((x + step.0 / w, y + step.1 / h));
        times.push(rate.frame_start(frame));
        before = picture;
        if (frame - first) % 10 == 0 {
            going((frame - first) as f32 / count as f32);
        }
    }
    let fps = rate.as_f64();
    Ok(Some(view(&times, &path, asked.smoothing, fps)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy picture shifted by `by` pixels.
    fn picture(by: (usize, usize)) -> Grey {
        let (width, height) = (160, 90);
        let pattern = |x: usize, y: usize| {
            let (x, y) = (x + 40 - by.0, y + 40 - by.1);
            let mut h = ((x / 3) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ ((y / 3) as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
            h ^= h >> 31;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 29;
            (h % 1000) as f32 / 1000.0
        };
        Grey {
            width,
            height,
            values: (0..height)
                .flat_map(|y| (0..width).map(move |x| pattern(x, y)))
                .collect(),
        }
    }

    #[test]
    fn the_whole_picture_moving_is_found() {
        let moved_by = moved(&picture((10, 10)), &picture((13, 8))).expect("found");
        assert_eq!(moved_by, (3.0, -2.0));
    }

    #[test]
    fn a_steady_path_gives_a_still_view_and_jitter_is_taken_out() {
        let times: Vec<Time> = (0..120).map(|f| Rate::FPS_60.frame_start(f)).collect();
        let still = vec![(0.0, 0.0); 120];
        let view_of_still = view(&times, &still, 0.5, 60.0);
        assert_eq!(view_of_still.places.len(), 2, "one line: its ends");
        assert!(view_of_still.places.iter().all(|(_, p)| *p == (0.5, 0.5)));
        // Every other frame a step to the right and back: the view follows
        // each step, the steady part of it being zero.
        let shaky: Vec<(f64, f64)> = (0..120)
            .map(|f| (if f % 2 == 0 { 0.0 } else { 0.01 }, 0.0))
            .collect();
        let found = view(&times, &shaky, 0.5, 60.0);
        let middle = &found.places[found.places.len() / 2];
        assert!((middle.1.0 - 0.5).abs() > 0.004, "the view moves with the jitter");
        assert!(found.zoom > 0.0);
    }

    #[test]
    fn smoothing_keeps_a_straight_path() {
        let path: Vec<(f64, f64)> = (0..50).map(|i| (i as f64, 0.0)).collect();
        let smooth = smoothed(&path, 3.0);
        assert!((smooth[25].0 - 25.0).abs() < 1e-9);
    }
}
