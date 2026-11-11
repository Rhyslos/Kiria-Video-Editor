//! Which graphics system draws the window: the Graphics setting, the order
//! the choices are tried in, and the safety net around Vulkan.
//!
//! On Windows the window prefers Vulkan, so that video can later be decoded
//! straight into the window's own device (`docs/research/decoding.md`), and
//! falls back to Direct3D 12. Each choice is tried on its own, in the order
//! [`plan`] gives; one that reports an error is followed by the next in the
//! same start.
//!
//! A driver can also fail without reporting anything: a crash or a freeze
//! while the window opens. [`StartMarker`] catches that. It is written before
//! a start with Vulkan and removed once frames have been drawn, so a marker
//! found at the next start means Vulkan never got that far. The Graphics
//! setting then becomes Direct3D 12, which never loads the Vulkan driver.

use crate::settings::Settings;
use diag::{AppError, Explain};
use eframe::{egui_wgpu, wgpu};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Frames drawn with Vulkan before the start counts as working.
pub const FRAMES_BEFORE_TRUSTED: u64 = 3;

const MARKER_FILE: &str = "vulkan.starting";

/// The Graphics setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GraphicsChoice {
    /// The app decides: on Windows Vulkan, and Direct3D 12 if Vulkan doesn't start.
    #[default]
    Automatic,
    Vulkan,
    Direct3D12,
}

impl GraphicsChoice {
    pub const ALL: [GraphicsChoice; 3] = [
        GraphicsChoice::Automatic,
        GraphicsChoice::Vulkan,
        GraphicsChoice::Direct3D12,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GraphicsChoice::Automatic => "Automatic",
            GraphicsChoice::Vulkan => "Vulkan",
            GraphicsChoice::Direct3D12 => "Direct3D 12",
        }
    }
}

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

/// What one start does about graphics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// What to try, in order, until a window opens.
    pub order: &'static [Backend],
    /// The last start with Vulkan never drew a frame: the Graphics setting
    /// becomes Direct3D 12, and the person is told once.
    pub switch_to_direct3d12: bool,
}

/// Decides what a start tries. `vulkan_start_unfinished` is whether the
/// marker of an earlier start with Vulkan is still there.
pub fn plan(platform: Platform, choice: GraphicsChoice, vulkan_start_unfinished: bool) -> Plan {
    const VULKAN_FIRST: &[Backend] = &[Backend::Vulkan, Backend::Direct3D12, Backend::ToolkitDefault];
    const WITHOUT_VULKAN: &[Backend] = &[Backend::Direct3D12, Backend::ToolkitDefault];
    if platform != Platform::Windows {
        return Plan {
            order: &[Backend::ToolkitDefault],
            switch_to_direct3d12: false,
        };
    }
    let wants_vulkan = choice != GraphicsChoice::Direct3D12;
    Plan {
        order: if wants_vulkan && !vulkan_start_unfinished {
            VULKAN_FIRST
        } else {
            WITHOUT_VULKAN
        },
        switch_to_direct3d12: wants_vulkan && vulkan_start_unfinished,
    }
}

/// The note in the data folder that a start with Vulkan is under way.
#[derive(Debug, Clone)]
pub struct StartMarker {
    path: PathBuf,
}

impl StartMarker {
    pub fn in_dir(data_dir: &Path) -> StartMarker {
        StartMarker {
            path: data_dir.join(MARKER_FILE),
        }
    }

    pub fn is_present(&self) -> bool {
        self.path.exists()
    }

    /// Call before opening a window with Vulkan.
    pub fn write(&self) -> diag::Result<()> {
        std::fs::write(&self.path, format!("pid {}\n", std::process::id())).or_explain(|| {
            AppError::new("The editor can't note that it is starting with Vulkan")
                .why(format!("Writing {} failed.", self.path.display()))
                .fix("Check that the disk is not full. Direct3D 12 is used meanwhile.")
        })
    }

    /// Call once Vulkan has drawn its first frames, or has reported an error.
    pub fn clear(&self) -> diag::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::new("The editor can't note that Vulkan started")
                .why(format!("Removing {} failed.", self.path.display()))
                .fix("Nothing is lost; the next start may switch to Direct3D 12 without need.")
                .source(err)),
        }
    }
}

/// Applies a [`Plan`] that gives up on Vulkan: saves Direct3D 12 as the
/// Graphics setting and only then removes the marker, so a setting that could
/// not be saved is caught again at the next start. Returns what to tell the person.
pub fn apply_switch(
    plan: &Plan,
    settings: &mut Settings,
    marker: &StartMarker,
    data_dir: &Path,
) -> Vec<AppError> {
    if !plan.switch_to_direct3d12 {
        // A marker with nothing to switch is left over from another setting.
        return marker.clear().err().into_iter().collect();
    }
    settings.graphics = GraphicsChoice::Direct3D12;
    let mut notices = vec![
        AppError::new("Graphics switched to Direct3D 12 because Vulkan didn't start")
            .why("The last start stopped before the window was drawn, which usually means a problem in the graphics driver.")
            .fix("Nothing to do now. To try Vulkan again, update the graphics driver, then choose Automatic under Settings, Graphics."),
    ];
    if let Err(err) = settings.save(data_dir).and_then(|()| marker.clear()) {
        notices.push(err);
    }
    notices
}

/// A graphics system that was tried in this start and did not open a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedTry {
    pub backend: Backend,
    pub reason: String,
}

impl FailedTry {
    /// The toolkit's error in words. Its text for a graphics system that is
    /// missing from the computer says little by itself.
    pub fn new(backend: Backend, error: &eframe::Error) -> FailedTry {
        let missing = matches!(
            error,
            eframe::Error::Wgpu(egui_wgpu::WgpuError::CreateSurfaceError(_))
        );
        let reason = if missing {
            format!("not available on this computer, or it can't draw to a window ({error})")
        } else {
            error.to_string()
        };
        FailedTry { backend, reason }
    }
}

/// How the window that opened got its graphics.
#[derive(Debug, Clone)]
pub struct Opened {
    pub backend: Backend,
    /// Tried before `backend` in this start.
    pub failed: Vec<FailedTry>,
    /// Present while Vulkan has not drawn its first frames yet.
    pub marker: Option<StartMarker>,
}

impl Opened {
    /// What to tell someone who asked for Vulkan and got something else.
    /// Automatic falls back quietly: Diagnostics and the log say what happened.
    pub fn notice(&self, choice: GraphicsChoice) -> Option<AppError> {
        let vulkan = self.failed.iter().find(|f| f.backend == Backend::Vulkan)?;
        (choice == GraphicsChoice::Vulkan).then(|| {
            AppError::new(format!(
                "Vulkan didn't start, so the window is drawn with {}",
                self.backend.label()
            ))
            .why(format!("The graphics system reported: {}", vulkan.reason))
            .fix("Update the graphics driver, or choose Automatic under Settings, Graphics.")
        })
    }

    /// One line per graphics system that was tried first, for Diagnostics.
    pub fn failed_text(&self) -> Option<String> {
        let lines: Vec<String> = self
            .failed
            .iter()
            .map(|f| format!("{}: {}", f.backend.label(), f.reason))
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// The window toolkit's graphics set-up for one entry of the plan.
///
/// Vulkan and Direct3D 12 each get an instance with only that system in it,
/// so falling back to Direct3D 12 never loads the Vulkan driver.
pub fn toolkit_options(backend: Backend) -> egui_wgpu::WgpuConfiguration {
    let mut options = eframe::NativeOptions::default().wgpu_options;
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup else {
        return options;
    };
    match backend {
        Backend::Vulkan => {
            setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
            let prefer_integrated = setup.power_preference == wgpu::PowerPreference::LowPower;
            setup.native_adapter_selector = Some(Arc::new(
                move |adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>| {
                    pick_card(adapters, surface, prefer_integrated)
                },
            ));
        }
        Backend::Direct3D12 => setup.instance_descriptor.backends = wgpu::Backends::DX12,
        Backend::ToolkitDefault => {}
    }
    options
}

/// The best real graphics card that can draw to the window. Software drawing
/// through Vulkan is refused: Direct3D 12 on a real card beats it.
fn pick_card(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
    prefer_integrated: bool,
) -> Result<wgpu::Adapter, String> {
    adapters
        .iter()
        .filter(|adapter| surface.is_none_or(|s| adapter.is_surface_supported(s)))
        .filter_map(|adapter| {
            card_rank(adapter.get_info().device_type, prefer_integrated).map(|rank| (rank, adapter))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, adapter)| adapter.clone())
        .ok_or_else(|| {
            format!(
                "no graphics card here can draw this window with Vulkan ({} found, software drawing not counted)",
                adapters.len()
            )
        })
}

/// Lower is better; `None` for software drawing.
fn card_rank(kind: wgpu::DeviceType, prefer_integrated: bool) -> Option<u8> {
    match kind {
        wgpu::DeviceType::DiscreteGpu => Some(u8::from(prefer_integrated)),
        wgpu::DeviceType::IntegratedGpu => Some(u8::from(!prefer_integrated)),
        wgpu::DeviceType::Other => Some(2),
        wgpu::DeviceType::VirtualGpu => Some(3),
        wgpu::DeviceType::Cpu => None,
    }
}

/// The graphics system and card in use, as one line: for Diagnostics, the
/// log and crash reports.
pub fn in_use(info: &wgpu::AdapterInfo) -> String {
    let backend = match info.backend {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "Direct3D 12",
        wgpu::Backend::Gl => "OpenGL",
        other => other.to_str(),
    };
    let mut text = format!("{backend} · {}", info.name);
    if info.device_type == wgpu::DeviceType::Cpu {
        text.push_str(" · software drawing");
    }
    let driver = format!("{} {}", info.driver, info.driver_info);
    if !driver.trim().is_empty() {
        text.push_str(&format!(" · driver {}", driver.trim()));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use Backend::{Direct3D12, ToolkitDefault, Vulkan};

    #[test]
    fn windows_tries_vulkan_first_unless_direct3d_12_is_chosen() {
        for choice in [GraphicsChoice::Automatic, GraphicsChoice::Vulkan] {
            let plan = plan(Platform::Windows, choice, false);
            assert_eq!(plan.order, [Vulkan, Direct3D12, ToolkitDefault]);
            assert!(!plan.switch_to_direct3d12);
        }
        let plan = plan(Platform::Windows, GraphicsChoice::Direct3D12, false);
        assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
        assert!(!plan.switch_to_direct3d12);
    }

    #[test]
    fn an_unfinished_vulkan_start_switches_to_direct3d_12() {
        for choice in [GraphicsChoice::Automatic, GraphicsChoice::Vulkan] {
            let plan = plan(Platform::Windows, choice, true);
            assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
            assert!(plan.switch_to_direct3d12);
        }
    }

    #[test]
    fn a_stale_marker_changes_nothing_once_direct3d_12_is_chosen() {
        let plan = plan(Platform::Windows, GraphicsChoice::Direct3D12, true);
        assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
        assert!(!plan.switch_to_direct3d12);
    }

    #[test]
    fn other_systems_keep_the_toolkits_choice_whatever_the_setting() {
        for choice in GraphicsChoice::ALL {
            for marker in [false, true] {
                let plan = plan(Platform::Other, choice, marker);
                assert_eq!(plan.order, [ToolkitDefault]);
                assert!(!plan.switch_to_direct3d12);
            }
        }
    }

    #[test]
    fn the_marker_is_written_found_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(tmp.path());
        assert!(!marker.is_present());
        marker.clear().unwrap(); // nothing to clear is fine
        marker.write().unwrap();
        assert!(StartMarker::in_dir(tmp.path()).is_present());
        marker.clear().unwrap();
        assert!(!marker.is_present());
    }

    #[test]
    fn a_crashed_vulkan_start_ends_with_direct3d_12_saved_and_the_next_start_quiet() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(tmp.path());
        marker.write().unwrap(); // the start that never drew a frame

        let (mut settings, _) = Settings::load(tmp.path());
        let next = plan(Platform::Windows, settings.graphics, marker.is_present());
        let notices = apply_switch(&next, &mut settings, &marker, tmp.path());
        assert_eq!(notices.len(), 1);
        assert!(notices[0].what().contains("switched to Direct3D 12"));
        assert_eq!(Settings::load(tmp.path()).0.graphics, GraphicsChoice::Direct3D12);
        assert!(!marker.is_present());

        let (mut settings, _) = Settings::load(tmp.path());
        let later = plan(Platform::Windows, settings.graphics, marker.is_present());
        assert_eq!(later.order, [Direct3D12, ToolkitDefault]);
        assert!(apply_switch(&later, &mut settings, &marker, tmp.path()).is_empty());

        // The way back: choosing Automatic (or Vulkan) again tries Vulkan.
        let retry = plan(Platform::Windows, GraphicsChoice::Automatic, marker.is_present());
        assert_eq!(retry.order[0], Vulkan);
    }

    #[test]
    fn the_marker_stays_when_the_switch_can_not_be_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(tmp.path());
        marker.write().unwrap();
        let missing = tmp.path().join("gone");
        let mut settings = Settings::default();
        let next = plan(Platform::Windows, settings.graphics, true);
        let notices = apply_switch(&next, &mut settings, &marker, &missing);
        assert_eq!(
            notices.len(),
            2,
            "the switch and the settings that could not be saved"
        );
        assert_eq!(settings.graphics, GraphicsChoice::Direct3D12);
        assert!(marker.is_present(), "so the next start switches again");
    }

    #[test]
    fn a_stale_marker_is_removed_when_there_is_nothing_to_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = StartMarker::in_dir(tmp.path());
        marker.write().unwrap();
        let mut settings = Settings {
            graphics: GraphicsChoice::Direct3D12,
            ..Settings::default()
        };
        let next = plan(Platform::Windows, settings.graphics, true);
        assert!(apply_switch(&next, &mut settings, &marker, tmp.path()).is_empty());
        assert!(!marker.is_present());
    }

    #[test]
    fn only_someone_who_chose_vulkan_is_told_when_it_did_not_start() {
        let opened = Opened {
            backend: Direct3D12,
            failed: vec![FailedTry {
                backend: Vulkan,
                reason: "no driver".into(),
            }],
            marker: None,
        };
        assert!(opened.notice(GraphicsChoice::Automatic).is_none());
        let notice = opened.notice(GraphicsChoice::Vulkan).unwrap();
        assert!(notice.what().contains("Direct3D 12"));
        assert!(notice.why_text().unwrap().contains("no driver"));
        assert_eq!(opened.failed_text().as_deref(), Some("Vulkan: no driver"));

        let first_try = Opened {
            backend: Vulkan,
            failed: Vec::new(),
            marker: None,
        };
        assert!(first_try.notice(GraphicsChoice::Vulkan).is_none());
        assert_eq!(first_try.failed_text(), None);
    }

    #[test]
    fn a_failed_try_keeps_the_toolkits_words() {
        let error = eframe::Error::AppCreation("the driver gave up".into());
        let failed = FailedTry::new(Vulkan, &error);
        assert_eq!(failed.backend, Vulkan);
        assert!(failed.reason.contains("the driver gave up"));
    }

    #[test]
    fn real_cards_are_ranked_and_software_drawing_is_refused() {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};
        assert!(card_rank(DiscreteGpu, false) < card_rank(IntegratedGpu, false));
        assert!(card_rank(IntegratedGpu, true) < card_rank(DiscreteGpu, true));
        for prefer_integrated in [false, true] {
            assert!(card_rank(IntegratedGpu, prefer_integrated) < card_rank(Other, prefer_integrated));
            assert!(card_rank(Other, prefer_integrated) < card_rank(VirtualGpu, prefer_integrated));
            assert_eq!(card_rank(Cpu, prefer_integrated), None);
        }
    }

    #[test]
    fn each_plan_entry_asks_the_toolkit_for_only_its_own_graphics_system() {
        let backends = |backend| match toolkit_options(backend).wgpu_setup {
            egui_wgpu::WgpuSetup::CreateNew(setup) => (
                setup.instance_descriptor.backends,
                setup.native_adapter_selector.is_some(),
            ),
            egui_wgpu::WgpuSetup::Existing(_) => panic!("the toolkit creates the device"),
        };
        assert_eq!(backends(Vulkan), (wgpu::Backends::VULKAN, true));
        assert_eq!(backends(Direct3D12), (wgpu::Backends::DX12, false));
        assert!(!backends(ToolkitDefault).1);
    }

    #[test]
    fn the_line_in_diagnostics_names_the_system_the_card_and_the_driver() {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, wgpu::Backend::Dx12);
        info.name = "Test Card".into();
        info.driver_info = "1.2.3".into();
        assert_eq!(in_use(&info), "Direct3D 12 · Test Card · driver 1.2.3");
        info.backend = wgpu::Backend::Vulkan;
        info.device_type = wgpu::DeviceType::Cpu;
        info.driver_info.clear();
        assert_eq!(in_use(&info), "Vulkan · Test Card · software drawing");
    }
}
