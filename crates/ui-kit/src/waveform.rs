//! A whole file's waveform in a rectangle: the Library's audio rows, cards
//! and the large waveform in its detail panel. Drawn as the timeline draws
//! sound (`paint_file`, `crate::timeline::wave_look`: the same shapes,
//! colours, side and scale, which are settings), so a recording looks the
//! same on every page.
//!
//! Also `Levels`, which every waveform drawn (here and on the timeline)
//! takes its columns' loudest peaks from: a column over many peaks costs a
//! few steps, not one per peak, so zoomed out over hours of sound a frame
//! costs what its width does, not what the recording's length does.

use egui::{Context, Id, Painter, Rect};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// The loudest of each two peaks, of each two of those, and so on: worked
/// out once per set of peaks, so the loudest between any two is found in a
/// few steps.
#[derive(Debug)]
pub struct Levels {
    /// `above[0][i]` is the loudest of peaks `2i` and `2i + 1`, `above[1][i]`
    /// of `above[0][2i]` and `above[0][2i + 1]`, and so on.
    above: Vec<Vec<f32>>,
}

impl Levels {
    pub fn of(peaks: &[f32]) -> Levels {
        let mut above: Vec<Vec<f32>> = Vec::new();
        loop {
            let below: &[f32] = above.last().map_or(peaks, Vec::as_slice);
            if below.len() <= 1 {
                break;
            }
            let next = below
                .chunks(2)
                .map(|pair| pair.iter().fold(0f32, |m, v| m.max(*v)))
                .collect();
            above.push(next);
        }
        Levels { above }
    }

    /// The loudest of `peaks[from..to]` (0 for none), `peaks` being what
    /// these levels were made of.
    pub fn loudest(&self, peaks: &[f32], from: usize, to: usize) -> f32 {
        let (mut lo, mut hi) = (from.min(peaks.len()), to.min(peaks.len()));
        let mut m = 0f32;
        let mut level: &[f32] = peaks;
        let mut up = 0;
        // Bottom up: the ends that don't make a whole pair are taken at
        // this level, the rest from the level above.
        while lo < hi {
            if !lo.is_multiple_of(2) {
                m = m.max(level[lo]);
                lo += 1;
            }
            if !hi.is_multiple_of(2) {
                hi -= 1;
                m = m.max(level[hi]);
            }
            lo /= 2;
            hi /= 2;
            let Some(next) = self.above.get(up) else {
                break;
            };
            level = next;
            up += 1;
        }
        m
    }
}

/// Fewer peaks than this are folded as they are: quicker than the levels.
const FEW: usize = 4_096;

/// Seconds a waveform's levels are kept after it was last drawn.
const KEPT_FOR: f64 = 60.0;

/// Seconds before levels whose thread could not start are tried again.
const RETRY_AFTER: f64 = 1.0;

/// The levels of every waveform drawn lately, one entry per waveform (the
/// `Id` its page gives it, the same while its file is read), so a waveform
/// whose peaks grow while they are read has one entry, not one per copy.
/// Levels are made on a thread of their own (a few milliseconds for hours
/// of sound), one at a time per waveform: peaks that arrive while one is
/// being made wait, and only the newest of them is made next. A thread
/// that can't start is tried again `RETRY_AFTER` later (the peaks are
/// folded as they are meanwhile), never made on the interface thread.
#[derive(Clone)]
struct Kept {
    waveforms: HashMap<Id, Entry>,
    /// Workers making levels right now (for tests).
    workers: Arc<AtomicUsize>,
    /// Starts a worker (a named thread; tests make it refuse).
    spawn: Spawn,
}

impl Default for Kept {
    fn default() -> Self {
        Kept {
            waveforms: HashMap::new(),
            workers: Arc::default(),
            spawn: Arc::new(spawn_thread),
        }
    }
}

/// Starts a worker running the work handed to it, or says why it couldn't.
type Spawn = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> std::io::Result<()> + Send + Sync>;

/// Runs `work` on a thread of its own.
fn spawn_thread(work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("waveform levels".into())
        .spawn(work)
        .map(drop)
}

/// Peaks, and the cell their levels go in once made.
type Making = (Arc<[f32]>, Arc<OnceLock<Arc<Levels>>>);

/// One waveform's levels. Each set of peaks is kept with what was made of
/// it, so the peaks are known by an `Arc` that can't be another's.
#[derive(Clone, Default)]
struct Entry {
    /// The newest levels made, and of which peaks.
    made: Option<(Arc<[f32]>, Arc<Levels>)>,
    /// Levels being made, and of which peaks.
    making: Option<Making>,
    /// When it was last drawn (the interface's clock, seconds).
    used: f64,
    /// Not before then is a worker started (one couldn't start before).
    retry_at: f64,
}

impl Kept {
    /// The levels of `peaks` drawn as waveform `id` at `now`, if made.
    fn levels(&mut self, ctx: &Context, id: Id, peaks: &Arc<[f32]>, now: f64) -> Option<Arc<Levels>> {
        self.let_go(now);
        let entry = self.waveforms.entry(id).or_default();
        entry.used = now;
        // Levels just made take the place of the ones before.
        if let Some((of, cell)) = &entry.making
            && let Some(levels) = cell.get()
        {
            entry.made = Some((Arc::clone(of), Arc::clone(levels)));
            entry.making = None;
        }
        if let Some((of, levels)) = &entry.made {
            if Arc::ptr_eq(of, peaks) {
                return Some(Arc::clone(levels));
            }
            // Made of other peaks (fewer, read earlier): no use any more.
            entry.made = None;
        }
        // Peaks drawn while others are being made wait for the draw after
        // those are made (the worker asks for one): the newest drawn then is
        // the next made, the ones in between never.
        if entry.making.is_none() && now >= entry.retry_at {
            entry.making = make(ctx, peaks, &self.workers, &self.spawn);
            if entry.making.is_none() {
                entry.retry_at = now + RETRY_AFTER;
            }
        }
        None
    }

    /// Lets go of the waveforms not drawn for `KEPT_FOR` seconds.
    fn let_go(&mut self, now: f64) {
        self.waveforms.retain(|_, entry| entry.used + KEPT_FOR >= now);
    }
}

/// Starts making the levels of `peaks` on a worker started by `spawn`; the
/// cell they go in, or `None` when the worker couldn't start.
fn make(ctx: &Context, peaks: &Arc<[f32]>, workers: &Arc<AtomicUsize>, spawn: &Spawn) -> Option<Making> {
    let cell = Arc::new(OnceLock::new());
    let (made, of, ctx, busy) = (
        Arc::clone(&cell),
        Arc::clone(peaks),
        ctx.clone(),
        Arc::clone(workers),
    );
    busy.fetch_add(1, Ordering::SeqCst);
    let spawned = spawn(Box::new(move || {
        let levels = Arc::new(Levels::of(&of));
        // Counted out before the levels can be seen, so the interface
        // never finds them made while this worker still counts.
        busy.fetch_sub(1, Ordering::SeqCst);
        let _ = made.set(levels);
        ctx.request_repaint();
    }));
    match spawned {
        Ok(()) => Some((Arc::clone(peaks), cell)),
        Err(err) => {
            workers.fetch_sub(1, Ordering::SeqCst);
            tracing::debug!("waveform levels not made, tried again shortly: {err}");
            None
        }
    }
}

/// Whether the context lets go of unused levels at the end of each pass.
#[derive(Clone, Copy, Default)]
struct Hooked(bool);

/// Where the levels are kept in the context's memory.
fn kept_id() -> Id {
    Id::unique("waveform levels")
}

/// The levels of `peaks`, drawn as waveform `id` (the same `Id` for one
/// stream of one file, also while its peaks grow as it is read), made in
/// the background the first time these peaks are drawn and kept while they
/// are; `None` until they are ready (the peaks are then folded as they are)
/// and for a few peaks. Waveforms not drawn for a minute are let go, at the
/// end of every pass.
pub fn levels_for(ctx: &Context, id: Id, peaks: &Arc<[f32]>) -> Option<Arc<Levels>> {
    if peaks.len() < FEW {
        return None;
    }
    let now = ctx.input(|input| input.time);
    let (levels, first) = ctx.data_mut(|data| {
        let hooked = data.get_temp_mut_or_default::<Hooked>(kept_id());
        let first = !std::mem::replace(&mut hooked.0, true);
        let kept = data.get_temp_mut_or_default::<Kept>(kept_id());
        (kept.levels(ctx, id, peaks, now), first)
    });
    if first {
        // Let go of what is no longer drawn even when nothing is drawn.
        ctx.on_end_pass(
            "waveform levels",
            Arc::new(|ui: &mut egui::Ui| {
                let now = ui.input(|input| input.time);
                ui.ctx().data_mut(|data| {
                    data.get_temp_mut_or_default::<Kept>(kept_id()).let_go(now);
                });
            }),
        );
    }
    levels
}

/// The loudest of `peaks[from..to]` (0 for none, as when `from` is past
/// `to`), through `levels` when there are any.
pub fn loudest(peaks: &[f32], levels: Option<&Levels>, from: usize, to: usize) -> f32 {
    match levels {
        Some(levels) => levels.loudest(peaks, from, to),
        None => peaks[from.min(to).min(peaks.len())..to.min(peaks.len())]
            .iter()
            .fold(0f32, |m, v| m.max(*v)),
    }
}

/// Draws a whole file's sound across `rect` as the timeline draws a clip's
/// (`look`: its shape, colours and side; `fitted`: on the file's own
/// scale, else full scale at the edge), its body first: `peaks` (the
/// loudest per stretch) with `sides` (the highest and lowest sample per
/// stretch) when known, `per_second` of them per second of a file `length`
/// seconds long; `id` says which stream of which file it is. Peaks still
/// being read leave the rest empty.
#[allow(clippy::too_many_arguments)]
pub fn paint_file(
    painter: &Painter,
    rect: Rect,
    id: Id,
    peaks: &Arc<[f32]>,
    sides: Option<[&Arc<[f32]>; 2]>,
    per_second: f32,
    length: f64,
    look: crate::timeline::wave_look::WaveLook,
    fitted: bool,
) {
    use crate::timeline::wave_look;
    wave_look::draw_body(painter, rect, look);
    if rect.width() < 1.0 || rect.height() < 4.0 || peaks.is_empty() || length <= 0.0 || per_second <= 0.0 {
        return;
    }
    let inner = rect.shrink2(egui::vec2(1.0, 3.0));
    let levels = levels_for(painter.ctx(), id, peaks);
    let sides = sides.filter(|[up, down]| up.len() == peaks.len() && down.len() == peaks.len());
    let side_levels = sides.map(|[up, down]| {
        (
            levels_for(painter.ctx(), id.with("up"), up),
            levels_for(painter.ctx(), id.with("down"), down),
        )
    });
    let count = inner.width().floor().max(1.0) as usize;
    let mut columns = Vec::with_capacity(count);
    for column in 0..count {
        let span = (column as f64 / count as f64 * length * f64::from(per_second)).floor() as usize;
        let end = (((column + 1) as f64 / count as f64 * length * f64::from(per_second)).ceil() as usize)
            .max(span + 1)
            .min(peaks.len());
        if span >= peaks.len() {
            break;
        }
        let peak = loudest(peaks, levels.as_deref(), span, end);
        let (up, down) = match (sides, &side_levels) {
            (Some([up, down]), Some((up_levels, down_levels))) => (
                loudest(up, up_levels.as_deref(), span, end),
                loudest(down, down_levels.as_deref(), span, end),
            ),
            _ => (peak, peak),
        };
        columns.push(wave_look::Column {
            x: inner.left().floor() + column as f32,
            peak,
            body: wave_look::body(peaks, span, end),
            up,
            down,
        });
    }
    let scale = match fitted {
        true => wave_look::fitted_scale(painter.ctx(), id, peaks),
        false => wave_look::WaveScale::straight(),
    };
    wave_look::draw(
        &painter.with_clip_rect(inner.intersect(painter.clip_rect())),
        inner,
        &columns,
        look,
        scale,
    );
}

/// The loudest peak under column `column` of `columns` spread over the
/// file, or `None` past the peaks read so far.
pub fn column_peak(
    peaks: &[f32],
    levels: Option<&Levels>,
    (per_second, length): (f32, f64),
    column: usize,
    columns: usize,
) -> Option<f32> {
    let columns = columns.max(1) as f64;
    let per_second = f64::from(per_second);
    let from = (column as f64 / columns * length * per_second).floor() as usize;
    let to = (((column + 1) as f64 / columns * length * per_second).ceil() as usize).max(from + 1);
    if from >= peaks.len() {
        return None;
    }
    let to = to.min(peaks.len());
    Some(loudest(peaks, levels, from, to).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_column_takes_the_loudest_peak_under_it() {
        // Ten seconds at ten peaks a second, quiet but for one loud peak.
        let mut peaks = vec![0.1f32; 100];
        peaks[57] = 0.9;
        // Ten columns: a second each.
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 5, 10), Some(0.9));
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 4, 10), Some(0.1));
        // Many more columns than peaks: each still finds one.
        assert!(column_peak(&peaks, None, (10.0, 10.0), 999, 1000).is_some());
    }

    #[test]
    fn peaks_still_being_read_stop_early() {
        let peaks = vec![0.5f32; 30];
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 2, 10), Some(0.5));
        assert_eq!(column_peak(&peaks, None, (10.0, 10.0), 3, 10), None);
    }

    #[test]
    fn long_peaks_get_levels_made_in_the_background_and_kept() {
        let ctx = Context::default();
        let id = Id::unique("a stream");
        let short: Arc<[f32]> = vec![0.5f32; 100].into();
        assert!(
            levels_for(&ctx, id, &short).is_none(),
            "a few are folded as they are"
        );
        let long: Arc<[f32]> = (0..100_000u32).map(|i| (i % 97) as f32 / 97.0).collect();
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let levels = loop {
            if let Some(levels) = levels_for(&ctx, id, &long) {
                break levels;
            }
            assert!(std::time::Instant::now() < give_up, "made in time");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(levels.loudest(&long, 0, 100_000), 96.0 / 97.0);
        let again = levels_for(&ctx, id, &long).unwrap();
        assert!(Arc::ptr_eq(&levels, &again), "kept, not made again");
    }

    #[test]
    fn the_levels_find_the_loudest_between_any_two_peaks() {
        // Not a power of two long, with loud peaks here and there.
        let peaks: Vec<f32> = (0..1_000u32)
            .map(|i| ((i * 7_919) % 1_009) as f32 / 1_009.0)
            .collect();
        let levels = Levels::of(&peaks);
        for from in (0..1_000).step_by(37) {
            for to in [
                from,
                from + 1,
                from + 2,
                from + 63,
                from + 64,
                from + 65,
                from + 500,
                1_000,
                2_000,
            ] {
                let want = peaks[from.min(1_000)..to.clamp(from, 1_000)]
                    .iter()
                    .fold(0f32, |m, v| m.max(*v));
                assert_eq!(levels.loudest(&peaks, from, to), want, "{from}..{to}");
            }
        }
        assert_eq!(Levels::of(&[]).loudest(&[], 0, 5), 0.0);
        assert_eq!(
            loudest(&peaks, Some(&levels), 10, 900),
            loudest(&peaks, None, 10, 900)
        );
        assert_eq!(Levels::of(&[0.3]).loudest(&[0.3], 0, 1), 0.3);
    }

    #[test]
    fn levels_are_seen_only_once_their_worker_is_counted_out() {
        let ctx = Context::default();
        let peaks: Arc<[f32]> = (0..FEW as u32).map(|i| i as f32).collect();
        for _ in 0..500 {
            let workers = Arc::new(AtomicUsize::new(0));
            let Some((_, cell)) = make(&ctx, &peaks, &workers, &Kept::default().spawn) else {
                panic!("worker started");
            };
            // Look as often as possible, so the moment the levels appear is
            // caught: the count must already be down then.
            let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while cell.get().is_none() {
                assert!(std::time::Instant::now() < give_up, "made in time");
                std::hint::spin_loop();
            }
            assert_eq!(workers.load(Ordering::SeqCst), 0, "worker still counted");
        }
    }

    #[test]
    fn peaks_growing_as_they_are_read_keep_one_entry_and_one_worker() {
        let ctx = Context::default();
        let id = Id::unique("a stream being read");
        let mut kept = Kept::default();
        let mut now = 0.0;
        let mut most_workers = 0;
        // A long recording read a little at a time: a new, longer copy of
        // its peaks every pass, as the page hands them over while reading.
        let all: Vec<f32> = (0..400_000u32).map(|i| (i % 89) as f32 / 89.0).collect();
        let mut last: Arc<[f32]> = Arc::from(&all[..FEW]);
        for read in (FEW..=all.len()).step_by(2_000) {
            last = Arc::from(&all[..read]);
            let _ = kept.levels(&ctx, id, &last, now);
            most_workers = most_workers.max(kept.workers.load(Ordering::SeqCst));
            now += 0.1;
        }
        assert_eq!(kept.waveforms.len(), 1, "one entry for the stream");
        assert!(most_workers <= 1, "{most_workers} workers at once");
        // Read to the end: its levels come, made of all of it.
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let levels = loop {
            if let Some(levels) = kept.levels(&ctx, id, &last, now) {
                break levels;
            }
            assert!(std::time::Instant::now() < give_up, "made in time");
            assert!(kept.workers.load(Ordering::SeqCst) <= 1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(levels.loudest(&last, 0, last.len()), 88.0 / 89.0);
        assert_eq!(kept.workers.load(Ordering::SeqCst), 0, "worker done");
        let entry = &kept.waveforms[&id];
        assert!(entry.making.is_none(), "nothing else being made");
        // Another stream drawn a minute later: the first is let go.
        let other: Arc<[f32]> = Arc::from(&all[..FEW]);
        let _ = kept.levels(&ctx, Id::unique("another"), &other, now + KEPT_FOR + 1.0);
        assert_eq!(kept.waveforms.len(), 1);
        assert!(
            !kept.waveforms.contains_key(&id),
            "not drawn for a minute: let go"
        );
    }

    #[test]
    fn a_worker_that_could_not_start_is_tried_again_shortly() {
        let ctx = Context::default();
        let id = Id::unique("a stream on a strained machine");
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&tries);
        let mut kept = Kept {
            spawn: Arc::new(move |_work| {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("no more threads"))
            }),
            ..Kept::default()
        };
        let peaks: Arc<[f32]> = (0..100_000u32).map(|i| (i % 97) as f32 / 97.0).collect();
        assert!(kept.levels(&ctx, id, &peaks, 10.0).is_none());
        assert_eq!(tries.load(Ordering::SeqCst), 1);
        assert_eq!(kept.workers.load(Ordering::SeqCst), 0, "not counted");
        assert!(kept.waveforms[&id].making.is_none(), "nothing waits on it");
        // Drawn every frame meanwhile: no new thread asked for each time.
        assert!(kept.levels(&ctx, id, &peaks, 10.5).is_none());
        assert_eq!(tries.load(Ordering::SeqCst), 1);
        // A little later threads start again: its levels come.
        kept.spawn = Kept::default().spawn;
        let now = 10.0 + RETRY_AFTER;
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let levels = loop {
            if let Some(levels) = kept.levels(&ctx, id, &peaks, now) {
                break levels;
            }
            assert!(std::time::Instant::now() < give_up, "made in time");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(levels.loudest(&peaks, 0, peaks.len()), 96.0 / 97.0);
    }

    #[test]
    fn loudest_of_nothing_is_nothing_whichever_way_round() {
        let peaks = vec![0.2f32, 0.7, 0.4];
        assert_eq!(loudest(&peaks, None, 2, 1), 0.0);
        assert_eq!(loudest(&peaks, None, 9, 1), 0.0);
        let levels = Levels::of(&peaks);
        assert_eq!(loudest(&peaks, Some(&levels), 2, 1), 0.0);
    }
}
