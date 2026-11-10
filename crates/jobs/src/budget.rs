//! Resource budgets: what the app may use, from Settings, with Auto defaults.

use serde::{Deserialize, Serialize};

/// A setting that is either automatic or set by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Limit<T> {
    #[default]
    Auto,
    Manual(T),
}

/// When background work (proxies, analysis, pre-rendering) may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackgroundPolicy {
    /// Always run.
    Always,
    /// Pause while the timeline plays, so playback gets the whole machine.
    #[default]
    PauseWhilePlaying,
    /// Run only when nobody is using the app.
    OnlyWhenIdle,
}

/// The resource settings as stored in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Budgets {
    pub worker_threads: Limit<usize>,
    pub ram_cache_mb: Limit<u64>,
    pub disk_cache_gb: Limit<u64>,
    pub background: BackgroundPolicy,
}

/// What this machine has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    pub cpu_threads: usize,
    pub total_ram_mb: u64,
    /// Free space on the disk that holds the cache, when known.
    pub free_disk_gb: Option<u64>,
}

impl Machine {
    /// Reads the current machine.
    pub fn detect() -> Machine {
        let cpu_threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let total_ram_mb = sysinfo::System::new()
            .map(|mut system| {
                system.refresh_memory();
                system.total_memory() / MB
            })
            .unwrap_or_else(|err| {
                tracing::warn!("total memory could not be read, assuming 4 GB: {err}");
                4 * 1024
            });
        let free_disk_gb = sysinfo::Disks::new_with_refreshed_list()
            .ok()
            .and_then(|disks| disks.iter().map(|d| d.available_space() / GB).max());
        Machine {
            cpu_threads,
            total_ram_mb,
            free_disk_gb,
        }
    }
}

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// Budgets turned into numbers for this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    /// Worker threads in total; one more is always left for the interface.
    pub worker_threads: usize,
    pub ram_cache_mb: u64,
    pub disk_cache_gb: u64,
    pub background: BackgroundPolicy,
}

/// Auto disk cache: 20 GB, or a quarter of the free space if that is less.
const AUTO_DISK_CACHE_GB: u64 = 20;

impl Budgets {
    /// Turns the settings into numbers that fit this machine. Manual values
    /// are kept within safe bounds so a typo can't starve the system.
    pub fn resolve(&self, machine: &Machine) -> Resolved {
        let max_workers = machine.cpu_threads.saturating_sub(1).max(1);
        let worker_threads = match self.worker_threads {
            Limit::Auto => max_workers,
            Limit::Manual(n) => n.clamp(1, max_workers),
        };
        let ram_cache_mb = match self.ram_cache_mb {
            Limit::Auto => machine.total_ram_mb / 4,
            Limit::Manual(mb) => mb.clamp(256, machine.total_ram_mb * 3 / 4),
        };
        let disk_cache_gb = match self.disk_cache_gb {
            Limit::Auto => machine
                .free_disk_gb
                .map_or(AUTO_DISK_CACHE_GB, |free| AUTO_DISK_CACHE_GB.min(free / 4)),
            Limit::Manual(gb) => gb.max(1),
        };
        Resolved {
            worker_threads,
            ram_cache_mb,
            disk_cache_gb,
            background: self.background,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: Machine = Machine {
        cpu_threads: 24,
        total_ram_mb: 32 * 1024,
        free_disk_gb: Some(500),
    };

    #[test]
    fn auto_keeps_one_thread_for_the_interface() {
        let r = Budgets::default().resolve(&MACHINE);
        assert_eq!(r.worker_threads, 23);
        assert_eq!(r.ram_cache_mb, 8 * 1024);
        assert_eq!(r.disk_cache_gb, 20);
        assert_eq!(r.background, BackgroundPolicy::PauseWhilePlaying);
    }

    #[test]
    fn manual_values_stay_within_safe_bounds() {
        let budgets = Budgets {
            worker_threads: Limit::Manual(500),
            ram_cache_mb: Limit::Manual(1_000_000),
            disk_cache_gb: Limit::Manual(0),
            background: BackgroundPolicy::Always,
        };
        let r = budgets.resolve(&MACHINE);
        assert_eq!(r.worker_threads, 23);
        assert_eq!(r.ram_cache_mb, 24 * 1024);
        assert_eq!(r.disk_cache_gb, 1);
    }

    #[test]
    fn a_single_core_machine_still_gets_a_worker() {
        let tiny = Machine {
            cpu_threads: 1,
            total_ram_mb: 2048,
            free_disk_gb: Some(8),
        };
        let r = Budgets::default().resolve(&tiny);
        assert_eq!(r.worker_threads, 1);
        assert_eq!(r.disk_cache_gb, 2);
    }

    #[test]
    fn detecting_the_machine_finds_at_least_one_thread() {
        assert!(Machine::detect().cpu_threads >= 1);
    }
}
