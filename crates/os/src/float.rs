//! The processor's handling of very small numbers, for the sound card's
//! thread.
//!
//! Filters and envelopes that die away pass through numbers so small the
//! processor handles them in a slow way ("denormals"): on some processors
//! a block that should take a millisecond then takes ten, and the sound
//! drops out. Asking the processor to treat them as zero makes every block
//! cost the same; the difference is far below anything that can be heard.
//!
//! The setting belongs to the calling thread, so the sound card's callback
//! asks for it at its start (it costs a few cycles) and the threads the
//! system hands it keep it.
#![allow(unsafe_code)]

/// Makes the calling thread treat very small numbers as zero (flush to
/// zero, denormals are zero). True when the processor has such a setting
/// and it is now on.
// The register's intrinsics are deprecated in favour of inline assembly,
// which would be just as unsafe and harder to read.
#[allow(deprecated)]
pub fn denormals_as_zero() -> bool {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::{_mm_getcsr, _mm_setcsr};
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        /// Flush to zero (bit 15) and denormals are zero (bit 6).
        const FTZ_DAZ: u32 = 0x8040;
        // SAFETY: reads and writes this thread's floating-point control
        // register (MXCSR), which every x86-64 processor has (SSE2 is part
        // of the architecture), setting only the two bits that make very
        // small numbers count as zero. No memory is touched and nothing
        // else in the register changes.
        unsafe {
            _mm_setcsr(_mm_getcsr() | FTZ_DAZ);
        }
        true
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn very_small_numbers_become_zero_on_the_thread_that_asks() {
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        std::thread::spawn(move || {
            assert!(std::hint::black_box(tiny) / 4.0 > 0.0, "a denormal before");
            if denormals_as_zero() {
                assert_eq!(std::hint::black_box(tiny) / 4.0, 0.0);
            }
        })
        .join()
        .expect("the thread ran");
    }
}
