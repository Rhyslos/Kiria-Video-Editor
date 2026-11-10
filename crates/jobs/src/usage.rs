//! Live CPU, GPU and memory use for the top-bar readout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// One reading. Percentages are 0 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    /// The whole machine's CPU use.
    pub cpu_percent: f32,
    /// This app's share of the CPU, scaled so 100 means every core is busy.
    pub app_cpu_percent: f32,
    pub app_ram_mb: u64,
    pub total_ram_mb: u64,
    /// Filled in by the GPU layer once it exists; `None` until then.
    pub gpu_percent: Option<f32>,
}

/// Samples usage on its own thread, so reading it never blocks the interface.
#[derive(Debug)]
pub struct UsageMonitor {
    latest: Arc<Mutex<Usage>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl UsageMonitor {
    /// Starts sampling every `interval` (one second suits a readout).
    pub fn start(interval: Duration) -> UsageMonitor {
        let latest = Arc::new(Mutex::new(Usage::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (latest, stop) = (Arc::clone(&latest), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("usage-monitor".into())
                .spawn(move || sample_loop(&latest, &stop, interval))
                .map_err(|err| tracing::warn!("the usage readout could not start: {err}"))
                .ok()
        };
        UsageMonitor { latest, stop, thread }
    }

    /// The most recent reading. Cheap; call it every frame if needed.
    pub fn latest(&self) -> Usage {
        *self.latest.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Drop for UsageMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

fn sample_loop(latest: &Mutex<Usage>, stop: &AtomicBool, interval: Duration) {
    let mut system = match System::new() {
        Ok(system) => system,
        Err(err) => {
            tracing::warn!("the usage readout is unavailable on this system: {err}");
            return;
        }
    };
    let pid = Pid::from_u32(std::process::id());
    while !stop.load(Ordering::Relaxed) {
        system.refresh_cpu_usage();
        system.refresh_memory();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let cores = system.cpus().len().max(1) as f32;
        let process = system.process(pid);
        let usage = Usage {
            cpu_percent: system.global_cpu_usage(),
            app_cpu_percent: process.map_or(0.0, |p| p.cpu_usage() / cores),
            app_ram_mb: process.map_or(0, |p| p.memory() / (1024 * 1024)),
            total_ram_mb: system.total_memory() / (1024 * 1024),
            gpu_percent: None,
        };
        *latest.lock().unwrap_or_else(|p| p.into_inner()) = usage;
        std::thread::park_timeout(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_arrive_and_the_monitor_stops_cleanly() {
        let monitor = UsageMonitor::start(Duration::from_millis(20));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while monitor.latest().total_ram_mb == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(monitor.latest().total_ram_mb > 0);
        drop(monitor);
    }
}
