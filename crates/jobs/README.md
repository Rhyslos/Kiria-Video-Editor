# jobs

Worker pools, priorities and resource budgets.

## Status

Done (Phase 0.5). GPU use in the readout stays empty until the GPU layer exists (Phase 0.6 onwards).

## Using it

```rust,ignore
use jobs::{Budgets, JobSystem, Machine, PoolKind, Priority};

let jobs = JobSystem::new(Budgets::default().resolve(&Machine::detect()));
let handle = jobs.submit(PoolKind::Analysis, Priority::Background, "waveform", |ctx| {
    // check ctx.is_cancelled() now and then in long loops
    Ok(42)
});
// On the interface thread: poll, never wait.
if let Some(result) = handle.try_result() { /* ... */ }
```

## How it works

| Part | Behaviour |
|---|---|
| Pools | Decode, render, analysis and export, each with its own threads. The worker budget is split 40 / 30 / 15 / 15 %, at least one thread each. The analysis pool's threads (import checks, waveforms, clip pictures, proxies, loudness: work nobody watches frame by frame) run below normal priority (`os::lower_this_thread`), so on a machine with few cores the interface thread is not kept waiting behind them; decode, render and export stay at normal priority, since playback and an export someone started wait for them |
| Interface thread | Never part of a pool. Auto budgets leave one CPU thread free for it |
| Priorities | `Playback` runs before `Interactive`, which runs before `Background`; equal priorities run oldest first |
| Background policy | `Always`, `PauseWhilePlaying` (default) or `OnlyWhenIdle`. Tell it with `set_playing` and `set_idle`; `closing` lets everything held back run, so closing the app can wait for a save started while playing |
| Results | `JobHandle::try_result()` (never blocks) or `wait()` (workers and tests only). Errors and panics come back as explained `AppError`s |
| Cancelling | `JobHandle::cancel()` sets a flag the job reads with `ctx.is_cancelled()` |
| Closing | When the pools close, the jobs running finish; queued jobs never start (their handles say the work was given up), except those submitted with `submit_write`: jobs that write something that must be on disk when the app ends (saves, the timeline layout, a saved frame, version or cheat sheet, clearing the cache). So closing never waits for reading nobody will look at |
| Held back | A background job that is already running reads `ctx.is_held_back()` (background work is paused now, by the policy) and can wait it out with `ctx.wait_while_held_back()`: a long job such as a proxy build gives the machine back while the timeline plays, also half way through |
| Progress | A job says how far it is with `ctx.set_progress(0.0..=1.0)`; `JobHandle::progress()` reads it without blocking: `Waiting` until the job starts, then `Running(share done)`. One atomic number per job |
| Budgets | `Budgets` is what Settings stores (`Auto` or a manual number). `resolve(&Machine)` turns it into safe numbers: workers 1 to CPU threads − 1, RAM cache ¼ of memory by default (at most ¾), disk cache 20 GB or ¼ of free space |
| Usage readout | `UsageMonitor` samples CPU and memory once a second on its own thread; `latest()` is cheap to call every frame |

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | `JobSystem`, `Priority`, `PoolKind`, `JobHandle`, `JobContext`, `JobProgress` |
| `src/pool.rs` | One pool: threads and a priority queue that can hold back background work |
| `src/budget.rs` | `Budgets`, `Limit`, `BackgroundPolicy`, `Machine`, `Resolved`; `free_space(path)`: the free space on the drive a folder is on (the innermost mount point it lies under), for packaging's check before copying |
| `src/usage.rs` | `UsageMonitor` and `Usage` for the top-bar readout |
