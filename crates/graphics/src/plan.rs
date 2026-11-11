//! Deciding what a start tries: a pure function of the system, the Graphics
//! settings, the two markers and the kind of run.

use crate::{GraphicsChoice, MarkerState};

/// One way of drawing the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vulkan,
    Direct3D12,
    /// Whatever the window toolkit picks when left alone, from every graphics
    /// system the machine has. The last resort on Windows; the only entry elsewhere.
    ToolkitDefault,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Vulkan => "Vulkan",
            Backend::Direct3D12 => "Direct3D 12",
            Backend::ToolkitDefault => "the system's default graphics",
        }
    }
}

/// The systems that differ in which graphics they offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// Linux: Vulkan is what the toolkit picks anyway, with OpenGL behind it.
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        }
    }
}

/// Whether a start may write anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// Someone opened the program.
    Normal,
    /// A smoke test, a screenshot or a demo: reads the setting and the
    /// marker, but never writes a marker, never changes the setting and
    /// never asks anything.
    Automated,
}

/// What a start found about decoding video on the window's device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodingFound {
    /// The setting: decode there when it can.
    pub wanted: bool,
    /// The marker written while the first pictures are decoded there.
    pub marker: MarkerState,
}

impl DecodingFound {
    /// As on a first start: wanted, and nothing left behind.
    pub const FRESH: DecodingFound = DecodingFound {
        wanted: true,
        marker: MarkerState::Absent,
    };
}

/// What one start does about graphics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// What to try, in order, until a window opens.
    pub order: &'static [Backend],
    /// Write the marker before trying Vulkan and remove it once frames are drawn.
    pub guard: bool,
    /// The last start with Vulkan never drew a frame: Direct3D 12 becomes the
    /// saved setting, and once the window is up the person is asked whether
    /// to keep it or to try Vulkan again.
    pub ask: bool,
    /// Decode video on the window's device in this session, when the window
    /// ends up on a device that can.
    pub decode_on_device: bool,
    /// The last session stopped while decoding its first pictures on the
    /// window's device: that becomes switched off in the saved settings, and
    /// once the window is up the person is asked whether to keep it off or
    /// to try it again.
    pub ask_decoding: bool,
}

/// Decides what a start tries.
pub fn plan(
    platform: Platform,
    choice: GraphicsChoice,
    marker: MarkerState,
    decoding: DecodingFound,
    run: RunKind,
) -> Plan {
    const VULKAN_FIRST: &[Backend] = &[Backend::Vulkan, Backend::Direct3D12, Backend::ToolkitDefault];
    const WITHOUT_VULKAN: &[Backend] = &[Backend::Direct3D12, Backend::ToolkitDefault];
    let normal = run == RunKind::Normal;
    // A marker left behind means the driver took the app down (or froze it)
    // while decoding on the device: not again without being asked. One
    // whose program is still running belongs to another copy in the middle
    // of it: this one stays off the device and decides nothing.
    let decode_on_device = decoding.wanted && decoding.marker == MarkerState::Absent;
    let ask_decoding = decoding.wanted && decoding.marker == MarkerState::LeftBehind && normal;
    if platform != Platform::Windows {
        return Plan {
            order: &[Backend::ToolkitDefault],
            guard: false,
            ask: false,
            decode_on_device,
            ask_decoding,
        };
    }
    let wants_vulkan = choice != GraphicsChoice::Direct3D12;
    let tries_vulkan = wants_vulkan && marker == MarkerState::Absent;
    Plan {
        order: if tries_vulkan {
            VULKAN_FIRST
        } else {
            WITHOUT_VULKAN
        },
        guard: tries_vulkan && normal,
        ask: wants_vulkan && marker == MarkerState::LeftBehind && normal,
        decode_on_device,
        ask_decoding,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Backend::{Direct3D12, ToolkitDefault, Vulkan};
    use MarkerState::{Absent, LeftBehind, StartUnderWay};
    use RunKind::{Automated, Normal};

    const WANTS_VULKAN: [GraphicsChoice; 2] = [GraphicsChoice::Automatic, GraphicsChoice::Vulkan];

    #[test]
    fn windows_tries_vulkan_first_and_guards_it_unless_direct3d_12_is_chosen() {
        for choice in WANTS_VULKAN {
            let plan = plan(Platform::Windows, choice, Absent, DecodingFound::FRESH, Normal);
            assert_eq!(plan.order, [Vulkan, Direct3D12, ToolkitDefault]);
            assert!(plan.guard && !plan.ask);
        }
        let plan = plan(
            Platform::Windows,
            GraphicsChoice::Direct3D12,
            Absent,
            DecodingFound::FRESH,
            Normal,
        );
        assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
        assert!(!plan.guard && !plan.ask);
    }

    #[test]
    fn a_vulkan_start_that_never_drew_a_frame_means_direct3d_12_and_a_question() {
        for choice in WANTS_VULKAN {
            let plan = plan(
                Platform::Windows,
                choice,
                LeftBehind,
                DecodingFound::FRESH,
                Normal,
            );
            assert_eq!(
                plan.order,
                [Direct3D12, ToolkitDefault],
                "the Vulkan driver is not loaded"
            );
            assert!(plan.ask && !plan.guard);
        }
    }

    #[test]
    fn a_stale_marker_asks_nothing_once_direct3d_12_is_chosen() {
        for marker in [LeftBehind, StartUnderWay] {
            let plan = plan(
                Platform::Windows,
                GraphicsChoice::Direct3D12,
                marker,
                DecodingFound::FRESH,
                Normal,
            );
            assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
            assert!(!plan.ask && !plan.guard);
        }
    }

    #[test]
    fn while_another_copy_is_starting_this_one_stays_off_vulkan_and_decides_nothing() {
        for choice in WANTS_VULKAN {
            let plan = plan(
                Platform::Windows,
                choice,
                StartUnderWay,
                DecodingFound::FRESH,
                Normal,
            );
            assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
            assert!(!plan.ask && !plan.guard);
        }
    }

    #[test]
    fn automated_runs_follow_the_setting_and_the_marker_but_write_and_ask_nothing() {
        for choice in GraphicsChoice::ALL {
            for marker in [Absent, LeftBehind, StartUnderWay] {
                let automated = plan(Platform::Windows, choice, marker, DecodingFound::FRESH, Automated);
                assert_eq!(
                    automated.order,
                    plan(Platform::Windows, choice, marker, DecodingFound::FRESH, Normal).order
                );
                assert!(!automated.guard && !automated.ask);
            }
        }
    }

    #[test]
    fn other_systems_keep_the_toolkits_choice_whatever_the_setting() {
        for choice in GraphicsChoice::ALL {
            for marker in [Absent, LeftBehind, StartUnderWay] {
                for run in [Normal, Automated] {
                    let plan = plan(Platform::Other, choice, marker, DecodingFound::FRESH, run);
                    assert_eq!(plan.order, [ToolkitDefault]);
                    assert!(!plan.guard && !plan.ask);
                }
            }
        }
    }

    #[test]
    fn video_is_decoded_on_the_device_unless_turned_off_or_it_failed_last_time() {
        let decoding = |wanted, marker| DecodingFound { wanted, marker };
        for platform in [Platform::Windows, Platform::Other] {
            let plan = |found, run| plan(platform, GraphicsChoice::Automatic, Absent, found, run);
            let fresh = plan(DecodingFound::FRESH, Normal);
            assert!(fresh.decode_on_device && !fresh.ask_decoding);
            // Turned off in Settings: off, and nothing to ask, whatever is lying around.
            for marker in [Absent, LeftBehind, StartUnderWay] {
                let off = plan(decoding(false, marker), Normal);
                assert!(!off.decode_on_device && !off.ask_decoding);
            }
            // The last session never got a picture out of the device.
            let failed = plan(decoding(true, LeftBehind), Normal);
            assert!(!failed.decode_on_device && failed.ask_decoding);
            // Another copy is in the middle of its first pictures.
            let busy = plan(decoding(true, StartUnderWay), Normal);
            assert!(!busy.decode_on_device && !busy.ask_decoding);
            // Automated runs follow the same facts but never ask.
            let automated = plan(decoding(true, LeftBehind), Automated);
            assert!(!automated.decode_on_device && !automated.ask_decoding);
        }
    }

    #[test]
    fn decoding_and_the_windows_graphics_are_decided_apart() {
        // A failed start with Vulkan says nothing against decoding (there
        // will be no device to decode on with Direct3D 12 anyway)...
        let plan = plan(
            Platform::Windows,
            GraphicsChoice::Automatic,
            LeftBehind,
            DecodingFound::FRESH,
            Normal,
        );
        assert!(plan.ask && plan.decode_on_device && !plan.ask_decoding);
        // ...and a failed decoding session doesn't take Vulkan away from the window.
        let found = DecodingFound {
            wanted: true,
            marker: LeftBehind,
        };
        let plan = super::plan(
            Platform::Windows,
            GraphicsChoice::Automatic,
            Absent,
            found,
            Normal,
        );
        assert_eq!(plan.order, [Vulkan, Direct3D12, ToolkitDefault]);
        assert!(plan.guard && !plan.ask && plan.ask_decoding && !plan.decode_on_device);
    }
}
