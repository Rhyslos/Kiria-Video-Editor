//! Deciding what a start tries: a pure function of the system, the Graphics
//! setting, the marker and the kind of run.

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
}

/// Decides what a start tries.
pub fn plan(platform: Platform, choice: GraphicsChoice, marker: MarkerState, run: RunKind) -> Plan {
    const VULKAN_FIRST: &[Backend] = &[Backend::Vulkan, Backend::Direct3D12, Backend::ToolkitDefault];
    const WITHOUT_VULKAN: &[Backend] = &[Backend::Direct3D12, Backend::ToolkitDefault];
    if platform != Platform::Windows {
        return Plan {
            order: &[Backend::ToolkitDefault],
            guard: false,
            ask: false,
        };
    }
    let wants_vulkan = choice != GraphicsChoice::Direct3D12;
    let normal = run == RunKind::Normal;
    let tries_vulkan = wants_vulkan && marker == MarkerState::Absent;
    Plan {
        order: if tries_vulkan {
            VULKAN_FIRST
        } else {
            WITHOUT_VULKAN
        },
        guard: tries_vulkan && normal,
        ask: wants_vulkan && marker == MarkerState::LeftBehind && normal,
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
            let plan = plan(Platform::Windows, choice, Absent, Normal);
            assert_eq!(plan.order, [Vulkan, Direct3D12, ToolkitDefault]);
            assert!(plan.guard && !plan.ask);
        }
        let plan = plan(Platform::Windows, GraphicsChoice::Direct3D12, Absent, Normal);
        assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
        assert!(!plan.guard && !plan.ask);
    }

    #[test]
    fn a_vulkan_start_that_never_drew_a_frame_means_direct3d_12_and_a_question() {
        for choice in WANTS_VULKAN {
            let plan = plan(Platform::Windows, choice, LeftBehind, Normal);
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
            let plan = plan(Platform::Windows, GraphicsChoice::Direct3D12, marker, Normal);
            assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
            assert!(!plan.ask && !plan.guard);
        }
    }

    #[test]
    fn while_another_copy_is_starting_this_one_stays_off_vulkan_and_decides_nothing() {
        for choice in WANTS_VULKAN {
            let plan = plan(Platform::Windows, choice, StartUnderWay, Normal);
            assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
            assert!(!plan.ask && !plan.guard);
        }
    }

    #[test]
    fn automated_runs_follow_the_setting_and_the_marker_but_write_and_ask_nothing() {
        for choice in GraphicsChoice::ALL {
            for marker in [Absent, LeftBehind, StartUnderWay] {
                let automated = plan(Platform::Windows, choice, marker, Automated);
                assert_eq!(
                    automated.order,
                    plan(Platform::Windows, choice, marker, Normal).order
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
                    let plan = plan(Platform::Other, choice, marker, run);
                    assert_eq!(plan.order, [ToolkitDefault]);
                    assert!(!plan.guard && !plan.ask);
                }
            }
        }
    }
}
