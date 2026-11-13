//! The two operating-system calls the spike needs.

/// This thread's own processor time so far (µs), where the system says.
pub fn thread_cpu_us() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: fills the timespec we own; no other effect.
        let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } == 0;
        ok.then(|| t.tv_sec as f64 * 1e6 + t.tv_nsec as f64 / 1e3)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Asks for this thread to run before ordinary ones (as a sound card's
/// own thread does). Returns whether the system agreed.
pub fn raise_priority() -> bool {
    #[cfg(target_os = "linux")]
    {
        let p = libc::sched_param { sched_priority: 10 };
        // SAFETY: changes only the calling thread's scheduling.
        unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &p) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
        };
        // SAFETY: changes only the calling thread's priority.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) != 0 }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}
