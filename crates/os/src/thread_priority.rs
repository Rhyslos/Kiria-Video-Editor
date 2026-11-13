//! Lowering a worker thread's place in the system's queue for the CPU, so
//! the interface thread is never kept waiting behind work nobody watches.
//!
//! On a machine with few cores, workers that check imported files, read
//! waveforms or build proxies take every core while they run; at the same
//! priority as the interface thread, a frame then waits its turn (measured
//! at the phase 5 gate on two cores: 2–5 ms of interface work took up to
//! 23 ms of wall time). Below it, they only take what the interface leaves.
#![allow(unsafe_code)]

/// How far below normal on Linux (`nice`): 10 gives the thread about a
/// tenth of the CPU a normal thread gets when both want it, and all of it
/// when nothing else does.
#[cfg(target_os = "linux")]
pub const NICER_BY: i32 = 10;

/// Lowers the calling thread's priority below normal, for the rest of its
/// life; threads it starts afterwards inherit it on Linux. False when the
/// system refused (the thread then runs at normal priority, as before).
pub fn lower_this_thread() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: GetCurrentThread returns a pseudo handle for the calling
        // thread that needs no closing; SetThreadPriority only changes that
        // thread's scheduling and touches no memory of ours.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) != 0 }
    }
    #[cfg(target_os = "linux")]
    {
        // Nice values go from -20 to 19; a higher one never needs privileges.
        let Some(now) = this_thread_nice() else {
            return false;
        };
        let lower = (now + NICER_BY).min(19);
        // SAFETY: gettid takes nothing and returns the calling thread's id;
        // setpriority with PRIO_PROCESS and a thread id changes that one
        // thread's nice value on Linux, touching no memory of ours.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, lower) == 0 }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

/// The calling thread's nice value (Linux).
#[cfg(target_os = "linux")]
pub fn this_thread_nice() -> Option<i32> {
    // SAFETY: as above; getpriority only reads the thread's nice value. It
    // can return -1 as a real value, so errno is cleared first and checked.
    unsafe {
        let id = libc::gettid();
        *libc::__errno_location() = 0;
        let nice = libc::getpriority(libc::PRIO_PROCESS, id as libc::id_t);
        (*libc::__errno_location() == 0).then_some(nice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowered_thread_is_lower_and_others_are_not() {
        let lowered = std::thread::spawn(|| {
            let done = lower_this_thread();
            #[cfg(target_os = "linux")]
            {
                let before_child = this_thread_nice();
                let child = std::thread::spawn(this_thread_nice).join().ok().flatten();
                (done, before_child, child)
            }
            #[cfg(not(target_os = "linux"))]
            {
                (done, None::<i32>, None::<i32>)
            }
        })
        .join();
        let Ok((done, _nice, _child)) = lowered else {
            panic!("the thread failed");
        };
        assert!(done, "the system refused a lower priority");
        #[cfg(target_os = "linux")]
        {
            let base = this_thread_nice();
            assert!(base.is_some());
            assert_eq!(_nice, base.map(|b| (b + NICER_BY).min(19)));
            assert_eq!(_child, _nice, "threads it starts inherit it");
        }
    }
}
