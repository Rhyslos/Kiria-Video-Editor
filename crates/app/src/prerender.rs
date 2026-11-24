//! Pre-rendering the region (its right-click menu, or search): its frames
//! put together ahead of time into the render cache (`cache::render`,
//! `engine::export::prerender`), so it plays smoothly however many layers
//! and effects it has. One pre-render at a time; asking for another stops
//! the one under way (what it did is kept).
//!
//! The cache bar shows what is kept. Which frames of the stretches
//! pre-rendered this session are still kept is worked out on a worker
//! whenever the project changes (an edit changes what a frame shows, and
//! so its key), never on the interface thread.

use cache::render::Store;
use engine::export::prerender::{Done, Prerender, kept_stretches};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{Project, SequenceId, Time};
use std::path::PathBuf;
use std::sync::Arc;
use ui_kit::timeline::{CacheRange, CacheState};

/// How much disk pre-rendered frames may take: about 3 hours at 1080p and
/// 60 frames a second.
pub const BUDGET: u64 = 20 * 1024 * 1024 * 1024;

/// Which version of the project the stretches kept were worked out for.
type Version = (u64, SequenceId);

/// Stretches of timelines.
type Stretches = Vec<(SequenceId, (Time, Time))>;

/// The render cache and the pre-render under way.
#[derive(Debug)]
pub struct Prerendering {
    store: Arc<Store>,
    /// Asked to draw again as a pre-render goes on.
    ctx: egui::Context,
    running: Option<(SequenceId, Prerender)>,
    /// The stretches pre-rendered this session, per timeline.
    asked: Stretches,
    /// Of those, what plays from the cache, as known for `known`.
    kept: Stretches,
    known: Option<Version>,
    checking: Option<(Version, JobHandle<Stretches>)>,
    /// When what is kept was last looked at while a pre-render runs.
    looked: Option<std::time::Instant>,
}

impl Prerendering {
    /// The render cache in `folder` (the cache folder's `renders/`),
    /// trimmed to its budget on a worker.
    pub fn new(folder: PathBuf, jobs: &JobSystem, ctx: egui::Context) -> Prerendering {
        let store = Arc::new(Store::open(folder));
        let trimmed = store.clone();
        drop(jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "trim the render cache",
            move |_| {
                let removed = trimmed.trim(BUDGET);
                if removed > 0 {
                    tracing::info!("{removed} pre-rendered stretches removed to stay inside the budget");
                }
                Ok(())
            },
        ));
        Prerendering {
            store,
            ctx,
            running: None,
            asked: Vec::new(),
            kept: Vec::new(),
            known: None,
            checking: None,
            looked: None,
        }
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    /// Starts pre-rendering `range` of `sequence`, stopping one under way.
    pub fn start(
        &mut self,
        project: &Project,
        sequence: SequenceId,
        range: (Time, Time),
        devices: engine::export::Devices,
    ) {
        let ctx = self.ctx.clone();
        let prerender = Prerender::start(
            project.clone(),
            sequence,
            range,
            devices,
            self.store.clone(),
            move || {
                ctx.request_repaint();
            },
        );
        self.running = Some((sequence, prerender));
        if !self.asked.contains(&(sequence, range)) {
            self.asked.push((sequence, range));
        }
        self.known = None;
    }

    /// Stops the pre-render under way, if one is.
    pub fn stop(&mut self) -> bool {
        self.running.take().is_some()
    }

    /// What the status line says while one runs: "Pre-rendering the region:
    /// 120 of 600 frames".
    pub fn progress_text(&self) -> Option<String> {
        let (_, running) = self.running.as_ref()?;
        let (done, all) = running.progress();
        Some(if all == 0 {
            "Pre-rendering the region: looking at what is kept already…".to_owned()
        } else {
            format!("Pre-rendering the region: {done} of {all} frames")
        })
    }

    /// Looks whether the pre-render ended, and what is kept now. Returns
    /// what to tell, once, when it ended.
    pub fn poll(
        &mut self,
        version: Version,
        project: &Project,
        jobs: &JobSystem,
    ) -> Option<diag::Result<String>> {
        let mut told = None;
        if let Some((_, running)) = &self.running
            && let Some(outcome) = running.finished()
        {
            told = Some(outcome.map(|done| ended(done.as_ref())));
            self.running = None;
            self.known = None;
        }
        // While it runs, what it adds is looked at every second.
        if self.running.is_some()
            && self.checking.is_none()
            && self
                .looked
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.known = None;
        }
        if let Some((for_version, checking)) = &self.checking
            && let Some(result) = checking.try_result()
        {
            if let Ok(kept) = result {
                self.kept = kept;
                self.known = Some(*for_version);
            }
            self.checking = None;
        }
        if self.checking.is_none() && self.known != Some(version) && !self.asked.is_empty() {
            let (asked, project, store) = (self.asked.clone(), project.clone(), self.store.clone());
            let job = jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "what is pre-rendered",
                move |_| {
                    Ok(asked
                        .iter()
                        .flat_map(|(sequence, range)| {
                            kept_stretches(&project, *sequence, *range, &store)
                                .into_iter()
                                .map(|stretch| (*sequence, stretch))
                        })
                        .collect())
                },
            );
            self.checking = Some((version, job));
            self.looked = Some(std::time::Instant::now());
        }
        told
    }

    /// The cache bar's stretches on `sequence`: kept (ready), and the rest
    /// of a stretch being pre-rendered (being prepared).
    pub fn ranges(&self, sequence: SequenceId) -> Vec<CacheRange> {
        let mut out: Vec<CacheRange> = Vec::new();
        if let Some((on, running)) = &self.running
            && *on == sequence
        {
            let (from, to) = running.range();
            out.push(CacheRange {
                start: from.as_secs_f64(),
                end: to.as_secs_f64(),
                state: CacheState::Preparing,
            });
        }
        let kept = self.kept.iter().filter(|(on, _)| *on == sequence);
        out.extend(kept.map(|(_, (from, to))| CacheRange {
            start: from.as_secs_f64(),
            end: to.as_secs_f64(),
            state: CacheState::Ready,
        }));
        // Ready wins over being prepared where it is already kept.
        cut_out_ready(out)
    }

    /// Clears the render cache (Settings › Cache), stopping a pre-render.
    pub fn clear(&mut self, jobs: &JobSystem) {
        self.running = None;
        self.kept.clear();
        self.known = None;
        let store = self.store.clone();
        drop(jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "clear the render cache",
            move |_| {
                let removed = store.clear();
                tracing::info!("{removed} pre-rendered stretches cleared");
                Ok(())
            },
        ));
    }
}

/// What the status line says when a pre-render ended.
fn ended(done: Option<&Done>) -> String {
    match done {
        None => "Pre-rendering stopped: what was done is kept".to_owned(),
        Some(done) if done.written == 0 => {
            "The region is pre-rendered already: it plays from the render cache".to_owned()
        }
        Some(done) => format!(
            "The region is pre-rendered ({} frames): it plays from the render cache",
            done.written
        ),
    }
}

/// The ranges with every stretch being prepared cut where a ready one is.
fn cut_out_ready(ranges: Vec<CacheRange>) -> Vec<CacheRange> {
    let ready: Vec<CacheRange> = ranges
        .iter()
        .copied()
        .filter(|r| r.state == CacheState::Ready)
        .collect();
    let mut out = ready.clone();
    for range in ranges.into_iter().filter(|r| r.state != CacheState::Ready) {
        let mut pieces = vec![range];
        for r in &ready {
            pieces = pieces
                .into_iter()
                .flat_map(|p| {
                    let before = CacheRange {
                        end: p.end.min(r.start),
                        ..p
                    };
                    let after = CacheRange {
                        start: p.start.max(r.end),
                        ..p
                    };
                    [before, after].into_iter().filter(|q| q.end > q.start)
                })
                .collect();
        }
        out.extend(pieces);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: f64, end: f64, state: CacheState) -> CacheRange {
        CacheRange { start, end, state }
    }

    #[test]
    fn what_is_kept_is_ready_and_the_rest_being_prepared() {
        let got = cut_out_ready(vec![
            range(0.0, 10.0, CacheState::Preparing),
            range(2.0, 4.0, CacheState::Ready),
        ]);
        assert_eq!(
            got,
            [
                range(2.0, 4.0, CacheState::Ready),
                range(0.0, 2.0, CacheState::Preparing),
                range(4.0, 10.0, CacheState::Preparing),
            ]
        );
    }

    #[test]
    fn the_status_line_says_how_it_ended() {
        assert!(ended(None).contains("stopped"));
        assert!(
            ended(Some(&Done {
                written: 0,
                passed: 9
            }))
            .contains("already")
        );
        assert!(
            ended(Some(&Done {
                written: 12,
                passed: 0
            }))
            .contains("12 frames")
        );
    }
}
