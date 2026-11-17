//! Following a box through a recording: where what it covers moves to,
//! frame by frame, so a blurred or pixelated box can stay on a name or a
//! face. The picture is read small and in grey; each frame, the part of it
//! that looks most like what the box covered a frame before (normalised
//! cross-correlation over a window around where it was, coarse then fine)
//! is where it went. When nothing looks enough like it, it is lost and
//! the following stops there.
//!
//! What it finds is a list of places at moments of the file, thinned to
//! the ones a straight line between doesn't already give (`thinned`),
//! for the box's Left and Top keyframes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use media::{Decoding, VideoDecoder};
use project::{Rate, Time};

/// The widest a frame is read at to follow a box.
const READ_WIDTH: u32 = 640;

/// How alike the best match must be (normalised cross-correlation, -1 to
/// 1) for the box to count as still found.
const LEAST_ALIKE: f32 = 0.55;

/// A grey picture, row by row, values 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Grey {
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
}

impl Grey {
    /// The grey of an RGBA picture.
    pub fn of_rgba(width: u32, height: u32, rgba: &[u8]) -> Grey {
        let values = rgba
            .chunks_exact(4)
            .map(|p| (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0)
            .collect();
        Grey {
            width: width as usize,
            height: height as usize,
            values,
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.values[y * self.width + x]
    }

    /// Half the size, each value the mean of four.
    fn halved(&self) -> Grey {
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut values = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (x2, y2) = ((2 * x).min(self.width - 1), (2 * y).min(self.height - 1));
                let (x3, y3) = ((x2 + 1).min(self.width - 1), (y2 + 1).min(self.height - 1));
                values.push((self.at(x2, y2) + self.at(x3, y2) + self.at(x2, y3) + self.at(x3, y3)) / 4.0);
            }
        }
        Grey {
            width,
            height,
            values,
        }
    }
}

/// A box in a picture, in its pixels: left, top, width, height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelBox {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl PixelBox {
    fn halved(self) -> PixelBox {
        PixelBox {
            x: self.x / 2,
            y: self.y / 2,
            w: (self.w / 2).max(2),
            h: (self.h / 2).max(2),
        }
    }

    fn inside(self, picture: &Grey) -> bool {
        self.x >= 0
            && self.y >= 0
            && self.w > 0
            && self.h > 0
            && (self.x + self.w) as usize <= picture.width
            && (self.y + self.h) as usize <= picture.height
    }
}

/// How alike the part of `picture` at `at` is to `template` (the part of
/// `before` in `was`): normalised cross-correlation, -1 to 1. `None` when
/// `at` is not inside the picture.
fn alike(before: &Grey, was: PixelBox, picture: &Grey, at: (i64, i64)) -> Option<f32> {
    let here = PixelBox {
        x: at.0,
        y: at.1,
        ..was
    };
    if !here.inside(picture) || !was.inside(before) {
        return None;
    }
    let n = (was.w * was.h) as f32;
    let (mut sum_a, mut sum_b, mut sum_aa, mut sum_bb, mut sum_ab) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for dy in 0..was.h as usize {
        let row_a = (was.y as usize + dy) * before.width + was.x as usize;
        let row_b = (here.y as usize + dy) * picture.width + here.x as usize;
        let a = &before.values[row_a..row_a + was.w as usize];
        let b = &picture.values[row_b..row_b + was.w as usize];
        for (a, b) in a.iter().zip(b) {
            sum_a += a;
            sum_b += b;
            sum_aa += a * a;
            sum_bb += b * b;
            sum_ab += a * b;
        }
    }
    let cov = sum_ab - sum_a * sum_b / n;
    let var = (sum_aa - sum_a * sum_a / n) * (sum_bb - sum_b * sum_b / n);
    // A flat box (no detail) matches anything as well as anything else.
    if var <= 1e-6 {
        return Some(if (sum_a - sum_b).abs() / n < 0.02 {
            1.0
        } else {
            0.0
        });
    }
    Some(cov / var.sqrt())
}

/// The best place for `was` (in `before`) in `picture` within `reach`
/// pixels of `around`, and how alike it is there.
fn best(
    before: &Grey,
    was: PixelBox,
    picture: &Grey,
    around: (i64, i64),
    reach: i64,
) -> Option<((i64, i64), f32)> {
    let mut found: Option<((i64, i64), f32)> = None;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let at = (around.0 + dx, around.1 + dy);
            if let Some(score) = alike(before, was, picture, at)
                && found.is_none_or(|(_, s)| score > s)
            {
                found = Some((at, score));
            }
        }
    }
    found
}

/// Where `was` (a box of `before`) is in `picture`, the next frame: found
/// on half-size pictures first, then refined; `None` when it is lost.
pub fn step(before: &Grey, was: PixelBox, picture: &Grey) -> Option<PixelBox> {
    let reach = (was.w.max(was.h) / 2).clamp(12, 64);
    let (small_before, small_picture) = (before.halved(), picture.halved());
    let small = was.halved();
    let (coarse, _) = best(
        &small_before,
        small,
        &small_picture,
        (small.x, small.y),
        reach / 2,
    )?;
    // The half-size box starts on an even pixel: put back what halving
    // dropped, then look a few pixels around.
    let around = (coarse.0 * 2 + was.x % 2, coarse.1 * 2 + was.y % 2);
    let (fine, score) = best(before, was, picture, around, 3)?;
    (score >= LEAST_ALIKE).then_some(PixelBox {
        x: fine.0,
        y: fine.1,
        ..was
    })
}

/// A place found: the moment of the file, and the box's left and top in
/// 0 to 1 of the picture.
pub type Place = (Time, (f64, f64));

/// The places a straight line between their neighbours doesn't already
/// give within `within` (of the picture's size): the first and last
/// always kept.
pub fn thinned(places: &[Place], within: f64) -> Vec<Place> {
    if places.len() <= 2 {
        return places.to_vec();
    }
    let mut keep = vec![false; places.len()];
    keep[0] = true;
    keep[places.len() - 1] = true;
    let mut stack = vec![(0usize, places.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let (t0, (x0, y0)) = places[first];
        let (t1, (x1, y1)) = places[last];
        let span = (t1 - t0).as_secs_f64().max(1e-9);
        let (mut far, mut at) = (0.0, first);
        for (i, (t, (x, y))) in places.iter().enumerate().take(last).skip(first + 1) {
            let k = (*t - t0).as_secs_f64() / span;
            let off = (x - (x0 + (x1 - x0) * k))
                .abs()
                .max((y - (y0 + (y1 - y0) * k)).abs());
            if off > far {
                (far, at) = (off, i);
            }
        }
        if far > within {
            keep[at] = true;
            stack.push((first, at));
            stack.push((at, last));
        }
    }
    places
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// What to follow: a box of a file's picture from one moment to another.
#[derive(Debug, Clone)]
pub struct Follow {
    pub path: PathBuf,
    pub stream: u32,
    pub rotation: u16,
    /// The file's frame rate: one place per recorded frame.
    pub rate: Rate,
    /// Moments of the file, from and to.
    pub from: Time,
    pub to: Time,
    /// The box at `from`: left, top, width, height, 0 to 1 of the picture.
    pub area: [f64; 4],
}

/// What a running follow says.
#[derive(Debug, Clone, PartialEq)]
pub enum Followed {
    /// How far it is, 0 to 1.
    Going(f32),
    /// Finished: the places found, and the moment it lost the box when it
    /// did (it stops there).
    Done {
        places: Vec<Place>,
        lost_at: Option<Time>,
    },
    /// The file couldn't be read.
    Failed(String),
}

/// A follow running on a thread of its own.
#[derive(Debug)]
pub struct Following {
    news: Receiver<Followed>,
    stop: Arc<AtomicBool>,
}

impl Following {
    /// Starts following.
    pub fn start(follow: Follow) -> diag::Result<Following> {
        let (send, news) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("follow a box".into())
            .spawn(move || {
                let said = match run(&follow, &stopped, &|done| {
                    let _ = send.send(Followed::Going(done));
                }) {
                    Ok((places, lost_at)) => Followed::Done { places, lost_at },
                    Err(err) => Followed::Failed(err.to_string()),
                };
                let _ = send.send(said);
            })
            .map_err(|err| {
                diag::AppError::new("Following the box couldn't start")
                    .why(format!("A thread for it couldn't be made: {err}."))
            })?;
        Ok(Following { news, stop })
    }

    /// The newest news, if any came.
    pub fn news(&self) -> Option<Followed> {
        let mut newest = None;
        while let Ok(news) = self.news.try_recv() {
            let done = matches!(news, Followed::Done { .. } | Followed::Failed(_));
            newest = Some(news);
            if done {
                break;
            }
        }
        newest
    }

    /// Stops it: what it found so far comes as `Done`.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Follows the box through the file, frame by frame.
fn run(follow: &Follow, stop: &AtomicBool, going: &dyn Fn(f32)) -> diag::Result<(Vec<Place>, Option<Time>)> {
    let mut decoder = VideoDecoder::open(&follow.path, follow.stream, follow.rotation, Decoding::Software)?;
    let rate = follow.rate;
    let (first, last) = (rate.frame_at(follow.from), rate.frame_at(follow.to));
    let read = |decoder: &mut VideoDecoder, frame: i64| -> diag::Result<Grey> {
        let at = rate.frame_start(frame).as_secs_f64();
        let picture = decoder.frame_at(at, READ_WIDTH, READ_WIDTH)?;
        Ok(Grey::of_rgba(picture.width, picture.height, &picture.rgba))
    };
    let mut before = read(&mut decoder, first)?;
    let [l, t, w, h] = follow.area;
    let (pw, ph) = (before.width as f64, before.height as f64);
    let mut was = PixelBox {
        x: (l * pw).round() as i64,
        y: (t * ph).round() as i64,
        w: (w * pw).round().max(4.0) as i64,
        h: (h * ph).round().max(4.0) as i64,
    };
    // A box partly off the picture follows its part on it.
    was.x = was.x.clamp(0, (before.width as i64 - was.w).max(0));
    was.y = was.y.clamp(0, (before.height as i64 - was.h).max(0));
    was.w = was.w.min(before.width as i64);
    was.h = was.h.min(before.height as i64);
    let to_place =
        |frame: i64, b: PixelBox| -> Place { (rate.frame_start(frame), (b.x as f64 / pw, b.y as f64 / ph)) };
    let mut places = vec![(rate.frame_start(first), (l, t))];
    let mut lost_at = None;
    let count = (last - first).max(1);
    for frame in first + 1..=last {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let picture = read(&mut decoder, frame)?;
        if (picture.width, picture.height) != (before.width, before.height) {
            lost_at = Some(rate.frame_start(frame));
            break;
        }
        match step(&before, was, &picture) {
            Some(now) => {
                places.push(to_place(frame, now));
                was = now;
                before = picture;
            }
            None => {
                lost_at = Some(rate.frame_start(frame));
                break;
            }
        }
        if (frame - first) % 10 == 0 {
            going((frame - first) as f32 / count as f32);
        }
    }
    Ok((thinned(&places, 0.002), lost_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy picture (a fixed pattern) with a bright textured square at `at`.
    fn picture(at: (usize, usize)) -> Grey {
        let (width, height) = (200, 120);
        let mut values: Vec<f32> = (0..width * height)
            .map(|i| ((i * 2_654_435_761usize) % 1000) as f32 / 4000.0)
            .collect();
        for y in 0..20 {
            for x in 0..30 {
                values[(at.1 + y) * width + at.0 + x] = 0.6 + ((x * 7 + y * 13) % 10) as f32 / 30.0;
            }
        }
        Grey {
            width,
            height,
            values,
        }
    }

    #[test]
    fn a_box_follows_what_it_covers_as_it_moves() {
        let mut was = PixelBox {
            x: 50,
            y: 40,
            w: 30,
            h: 20,
        };
        let mut before = picture((50, 40));
        for (i, at) in [(54, 41), (59, 43), (65, 44), (70, 47)].into_iter().enumerate() {
            let now = picture(at);
            let found = step(&before, was, &now).unwrap_or_else(|| panic!("lost at step {i}"));
            assert_eq!((found.x, found.y), (at.0 as i64, at.1 as i64), "step {i}");
            (was, before) = (found, now);
        }
    }

    #[test]
    fn a_box_whose_picture_is_gone_is_lost() {
        let was = PixelBox {
            x: 50,
            y: 40,
            w: 30,
            h: 20,
        };
        let before = picture((50, 40));
        let flat = Grey {
            width: 200,
            height: 120,
            values: (0..200 * 120).map(|i| ((i * 7919) % 97) as f32 / 97.0).collect(),
        };
        assert_eq!(step(&before, was, &flat), None);
    }

    #[test]
    fn a_box_on_a_still_part_of_a_recording_stays_put() {
        // The test recording's counter sits in its top left corner.
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let follow = Follow {
            path,
            stream: 0,
            rotation: 0,
            rate: Rate::FPS_60,
            from: Time::ZERO,
            to: Time::from_ratio(1, 2).expect("half a second"),
            area: [0.02, 0.05, 0.4, 0.2],
        };
        let mut found = None;
        let following = Following::start(follow).expect("starts");
        for _ in 0..600 {
            match following.news() {
                Some(Followed::Done { places, lost_at }) => {
                    found = Some((places, lost_at));
                    break;
                }
                Some(Followed::Failed(why)) => panic!("{why}"),
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
        let (places, lost_at) = found.expect("done within half a minute");
        assert_eq!(lost_at, None);
        for (_, (x, y)) in &places {
            assert!((x - 0.02).abs() < 0.02 && (y - 0.05).abs() < 0.03, "{x} {y}");
        }
    }

    #[test]
    fn places_on_a_straight_line_are_thinned_to_its_ends() {
        let t = |f: i64| Rate::FPS_60.frame_start(f);
        let line: Vec<Place> = (0..30).map(|f| (t(f), (0.1 + f as f64 * 0.01, 0.5))).collect();
        let thin = thinned(&line, 0.002);
        assert_eq!(thin, vec![line[0], line[29]]);
        // A turn keeps its corner.
        let mut bent = line.clone();
        bent.extend((30..40).map(|f| (t(f), (0.39, 0.5 + (f - 29) as f64 * 0.02))));
        let thin = thinned(&bent, 0.002);
        assert_eq!(thin.len(), 3);
        assert_eq!(thin[1].0, t(29));
    }
}
