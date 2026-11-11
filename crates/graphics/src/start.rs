//! Before the window: reading the setting and the marker, and opening the
//! window with the first graphics system that starts.

use crate::{
    Backend, GraphicsChoice, GraphicsSettings, MarkerState, Plan, Platform, Program, RunKind, StartMarker,
    VideoDecoding, plan, vulkan_device,
};
use diag::AppError;
use eframe::{egui_wgpu, wgpu};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use video_device::VideoDevice;

/// What a program decided about graphics before its window opens.
#[derive(Debug, Clone)]
pub struct Startup {
    pub(crate) program: Program,
    pub(crate) platform: Platform,
    pub(crate) plan: Plan,
    pub(crate) marker: StartMarker,
    pub(crate) settings: GraphicsSettings,
    pub(crate) data_dir: PathBuf,
    pub(crate) run: RunKind,
    pub(crate) warning_preview: bool,
}

impl Startup {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Shows the question about a failed start with Vulkan although nothing
    /// failed, for screenshots.
    pub fn with_warning_preview(mut self) -> Startup {
        self.warning_preview = true;
        self
    }

    /// The window's share of this start, for a window opened with `backend`.
    pub(crate) fn opened(&self, backend: Backend, failed: Vec<FailedTry>) -> Opened {
        Opened {
            backend,
            failed,
            marker: (self.plan.guard && backend == Backend::Vulkan).then(|| self.marker.clone()),
            ask: self.plan.ask || self.warning_preview,
            video: VideoDecoding::without_own_device(self.program, backend, self.platform),
            platform: self.platform,
            settings: self.settings,
            data_dir: self.data_dir.clone(),
            run: self.run,
        }
    }
}

/// Reads the Graphics setting and `program`'s marker in `data_dir` and
/// decides what this start tries. Also returns what went wrong meanwhile, to
/// show once the window is open.
///
/// A marker left by a start with Vulkan that never drew a frame makes
/// Direct3D 12 the saved setting here, before anything is drawn: closing the
/// window without answering the question then keeps the safe choice. The
/// marker is removed only once the setting is saved, so a setting that could
/// not be saved is caught again at the next start. Automated runs write nothing.
pub fn prepare(program: Program, data_dir: &Path, run: RunKind) -> (Startup, Vec<AppError>) {
    prepare_on(Platform::current(), program, data_dir, run)
}

/// [`prepare`] for a given system.
pub(crate) fn prepare_on(
    platform: Platform,
    program: Program,
    data_dir: &Path,
    run: RunKind,
) -> (Startup, Vec<AppError>) {
    let mut settings = GraphicsSettings::load(data_dir);
    // A choice still kept in the editor's own settings moves to the shared file.
    let adopt = GraphicsSettings::earlier(data_dir).is_some_and(|e| e != GraphicsSettings::default());
    let marker = StartMarker::in_dir(program, data_dir);
    let found = marker.state();
    let plan = plan(platform, settings.choice, found, run);
    let mut problems = Vec::new();
    if run == RunKind::Normal {
        if plan.ask {
            tracing::warn!(
                "the last start with Vulkan never drew a frame: using Direct3D 12 and asking what to keep"
            );
            settings.choice = GraphicsChoice::Direct3D12;
            problems.extend(settings.save(data_dir).and_then(|()| marker.clear()).err());
        } else {
            if adopt {
                problems.extend(settings.save(data_dir).err());
            }
            if found == MarkerState::LeftBehind {
                // Nothing to ask: left over from before Direct3D 12 was chosen.
                problems.extend(marker.clear().err());
            }
        }
    }
    let startup = Startup {
        program,
        platform,
        plan,
        marker,
        settings,
        data_dir: data_dir.to_path_buf(),
        run,
        warning_preview: false,
    };
    (startup, problems)
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

/// How the window that opened got its graphics. Hand it to
/// [`WindowGraphics::new`](crate::WindowGraphics::new).
#[derive(Debug, Clone)]
pub struct Opened {
    pub backend: Backend,
    /// Tried before `backend` in this start.
    pub failed: Vec<FailedTry>,
    /// Present while Vulkan has not drawn its first frames yet.
    pub(crate) marker: Option<StartMarker>,
    /// Ask whether to keep Direct3D 12 or to try Vulkan again.
    pub(crate) ask: bool,
    /// Whether the window's device was opened for video decoding too.
    pub(crate) video: VideoDecoding,
    pub(crate) platform: Platform,
    pub(crate) settings: GraphicsSettings,
    pub(crate) data_dir: PathBuf,
    pub(crate) run: RunKind,
}

/// Opens a window with the first graphics system of the plan that starts,
/// and returns when the window has closed.
///
/// `window` gives the window's options (size, title, icon); the graphics part
/// is filled in here. `create` builds the app once the graphics exist, from
/// `start` and what [`WindowGraphics`](crate::WindowGraphics) needs.
///
/// A graphics system that reports an error before the window exists is
/// followed by the next one. Vulkan can also crash or freeze instead: the
/// marker written here is what the next start finds then.
pub fn open_window<S>(
    startup: &Startup,
    app_name: &str,
    window: impl Fn() -> eframe::NativeOptions,
    start: S,
    create: impl Fn(&eframe::CreationContext<'_>, S, Opened) -> Box<dyn eframe::App>,
) -> diag::Result<()> {
    let window_name = startup.program.window_name();
    // Handed to the window that opens; still here after a try that didn't.
    let start = Rc::new(RefCell::new(Some(start)));
    let mut failed: Vec<FailedTry> = Vec::new();
    for &backend in startup.plan.order {
        let guarded = startup.plan.guard && backend == Backend::Vulkan;
        if guarded && let Err(err) = startup.marker.write() {
            // Without the marker a crash would repeat at every start: skip Vulkan.
            tracing::warn!("{}", err.report());
            let reason = "not tried: the note that guards against a crash could not be written".into();
            failed.push(FailedTry { backend, reason });
            continue;
        }
        // With Vulkan, the device is first opened here with video decoding
        // (inside the marker's guard, like everything Vulkan). If that
        // can't be done, or the window doesn't open on it, the toolkit
        // opens its own device as it always did.
        let mut tries: Vec<(egui_wgpu::WgpuConfiguration, Option<Arc<VideoDevice>>)> = Vec::new();
        let mut why_not = None;
        if backend == Backend::Vulkan && startup.program.plays_video() {
            match own_vulkan_device() {
                Ok((options, device)) => tries.push((options, Some(device))),
                Err(reason) => {
                    tracing::info!("the window's device is not opened for video decoding: {reason}");
                    why_not = Some(reason);
                }
            }
        }
        tries.push((toolkit_options(backend), None));
        let last = tries.len() - 1;
        for (index, (wgpu_options, device)) in tries.into_iter().enumerate() {
            let own_device = device.is_some();
            let mut opened = startup.opened(backend, failed.clone());
            opened.video = match (device, &why_not) {
                (Some(device), _) => VideoDecoding::OnDevice(device),
                (None, Some(reason)) => VideoDecoding::NotAvailable(reason.clone()),
                (None, None) => opened.video,
            };
            let options = eframe::NativeOptions {
                renderer: eframe::Renderer::Wgpu,
                wgpu_options,
                ..window()
            };
            let (for_window, create) = (Rc::clone(&start), &create);
            let result = eframe::run_native(
                app_name,
                options,
                Box::new(move |cc| match for_window.borrow_mut().take() {
                    Some(start) => Ok(create(cc, start, opened)),
                    None => Err(AppError::new(format!("The {window_name} was opened twice")).into()),
                }),
            );
            let window_existed = start.borrow().is_none();
            match result {
                Ok(()) => {
                    if guarded && let Err(err) = startup.marker.clear() {
                        tracing::warn!("{}", err.report());
                    }
                    return Ok(());
                }
                // The window was open (or opening) when it failed: nothing to fall
                // back to in this start. A marker still there makes the next one
                // use Direct3D 12.
                Err(err) if window_existed => {
                    return Err(AppError::new(format!("The {window_name} stopped working"))
                        .why(format!("The graphics system reported: {err}"))
                        .fix("Start the app again. If it keeps happening, update your graphics driver and report it with the log file."));
                }
                // The device opened for video decoding didn't open the
                // window: the toolkit's own device is tried next.
                Err(err) if own_device && index < last => {
                    let failure = FailedTry::new(backend, &err);
                    tracing::warn!(
                        "{} did not start on the device opened for video decoding: {}",
                        backend.label(),
                        failure.reason
                    );
                    why_not = Some(format!(
                        "the window didn't open on the device opened for it ({})",
                        failure.reason
                    ));
                    let reason = format!("with video decoding: {}", failure.reason);
                    failed.push(FailedTry { backend, reason });
                }
                Err(err) => {
                    if guarded && let Err(err) = startup.marker.clear() {
                        tracing::warn!("{}", err.report());
                    }
                    let failure = FailedTry::new(backend, &err);
                    tracing::warn!("{} did not start: {}", backend.label(), failure.reason);
                    failed.push(failure);
                }
            }
        }
    }
    let tried: Vec<String> = failed
        .iter()
        .map(|f| format!("{}: {}", f.backend.label(), f.reason))
        .collect();
    Err(AppError::new(format!("The {window_name} could not open"))
        .why(format!("No graphics system started. {}", tried.join("; ")))
        .fix("Update your graphics driver. If that doesn't help, report it with the log file."))
}

/// The window toolkit's graphics set-up for one entry of the plan.
///
/// Vulkan and Direct3D 12 each get an instance with only that system in it,
/// so falling back to Direct3D 12 never loads the Vulkan driver.
fn toolkit_options(backend: Backend) -> egui_wgpu::WgpuConfiguration {
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

/// Opens the window's Vulkan device here, with video decoding, the way the
/// toolkit would open it otherwise: the same instance settings, the same
/// choice of card, the same limits. Returns the toolkit's set-up around that
/// device and the handle for the decoder, or the reason it can't be done.
fn own_vulkan_device() -> Result<(egui_wgpu::WgpuConfiguration, Arc<VideoDevice>), String> {
    let mut options = toolkit_options(Backend::Vulkan);
    let egui_wgpu::WgpuSetup::CreateNew(setup) = options.wgpu_setup else {
        return Err("the window toolkit was given a device already".into());
    };
    let prefer_integrated = setup.power_preference == wgpu::PowerPreference::LowPower;
    let device = Arc::clone(&setup.device_descriptor);
    let own = vulkan_device::open(setup.instance_descriptor, prefer_integrated, &*device)?;
    options.wgpu_setup = egui_wgpu::WgpuSetup::Existing(own.setup);
    Ok((options, own.video))
}

/// The best real graphics card that can draw to the window. Software drawing
/// through Vulkan is refused: Direct3D 12 on a real card beats it.
pub(crate) fn pick_card(
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

#[cfg(test)]
mod tests {
    use super::*;
    use Backend::{Direct3D12, ToolkitDefault, Vulkan};

    /// A marker as a crashed start leaves it: the process that wrote it is gone.
    fn leave_marker(program: Program, dir: &Path) -> StartMarker {
        let marker = StartMarker::in_dir(program, dir);
        marker.write().unwrap(); // our own number counts as a finished process
        marker
    }

    fn windows(program: Program, dir: &Path, run: RunKind) -> (Startup, Vec<AppError>) {
        prepare_on(Platform::Windows, program, dir, run)
    }

    #[test]
    fn a_first_start_tries_vulkan_with_the_marker_and_writes_nothing_yet() {
        let tmp = tempfile::tempdir().unwrap();
        for program in [Program::Launcher, Program::Editor] {
            let (startup, problems) = windows(program, tmp.path(), RunKind::Normal);
            assert!(problems.is_empty());
            assert_eq!(startup.plan().order, [Vulkan, Direct3D12, ToolkitDefault]);
            assert!(startup.opened(Vulkan, Vec::new()).marker.is_some());
            assert!(startup.opened(Direct3D12, Vec::new()).marker.is_none());
            assert!(!startup.opened(Vulkan, Vec::new()).ask);
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_crashed_vulkan_start_saves_direct3d_12_removes_the_marker_and_asks() {
        for program in [Program::Launcher, Program::Editor] {
            let tmp = tempfile::tempdir().unwrap();
            let marker = leave_marker(program, tmp.path());
            let (startup, problems) = windows(program, tmp.path(), RunKind::Normal);
            assert!(problems.is_empty());
            assert_eq!(startup.plan().order, [Direct3D12, ToolkitDefault]);
            assert!(startup.opened(Direct3D12, Vec::new()).ask);
            assert_eq!(
                GraphicsSettings::load(tmp.path()).choice,
                GraphicsChoice::Direct3D12
            );
            assert!(!marker.is_present());

            // Closed without answering: Direct3D 12 stays, for both programs, and nothing is asked again.
            for other in [Program::Launcher, Program::Editor] {
                let (later, _) = windows(other, tmp.path(), RunKind::Normal);
                assert_eq!(later.plan().order, [Direct3D12, ToolkitDefault]);
                assert!(!later.plan().ask);
            }
        }
    }

    #[test]
    fn one_programs_failed_start_does_not_make_the_other_ask() {
        let tmp = tempfile::tempdir().unwrap();
        let editor_marker = leave_marker(Program::Editor, tmp.path());
        let (launcher, _) = windows(Program::Launcher, tmp.path(), RunKind::Normal);
        assert_eq!(launcher.plan().order[0], Vulkan);
        assert!(!launcher.plan().ask);
        assert!(editor_marker.is_present(), "left for the editor's own next start");
        let (editor, _) = windows(Program::Editor, tmp.path(), RunKind::Normal);
        assert!(editor.plan().ask);
    }

    #[test]
    fn the_marker_stays_when_the_safe_choice_can_not_be_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = leave_marker(Program::Launcher, tmp.path());
        // A folder where the settings file should be makes saving fail.
        std::fs::create_dir(GraphicsSettings::path(tmp.path())).unwrap();
        let (startup, problems) = windows(Program::Launcher, tmp.path(), RunKind::Normal);
        assert_eq!(problems.len(), 1);
        assert_eq!(startup.plan().order, [Direct3D12, ToolkitDefault]);
        assert!(marker.is_present(), "so the next start is safe again");
    }

    #[test]
    fn automated_runs_leave_the_marker_and_the_setting_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = windows(Program::Launcher, tmp.path(), RunKind::Automated);
        assert_eq!(startup.plan().order[0], Vulkan);
        assert!(
            startup.opened(Vulkan, Vec::new()).marker.is_none(),
            "no marker is written"
        );

        let marker = leave_marker(Program::Launcher, tmp.path());
        let (startup, problems) = windows(Program::Launcher, tmp.path(), RunKind::Automated);
        assert!(problems.is_empty());
        assert_eq!(startup.plan().order, [Direct3D12, ToolkitDefault]);
        assert!(!startup.opened(Direct3D12, Vec::new()).ask);
        assert!(marker.is_present());
        assert!(!GraphicsSettings::path(tmp.path()).exists());
    }

    #[test]
    fn a_stale_marker_is_removed_when_direct3d_12_is_already_chosen() {
        let tmp = tempfile::tempdir().unwrap();
        GraphicsSettings {
            choice: GraphicsChoice::Direct3D12,
        }
        .save(tmp.path())
        .unwrap();
        let marker = leave_marker(Program::Editor, tmp.path());
        let (startup, problems) = windows(Program::Editor, tmp.path(), RunKind::Normal);
        assert!(problems.is_empty() && !startup.plan().ask);
        assert!(!marker.is_present());
    }

    #[test]
    fn the_editors_earlier_choice_moves_to_the_shared_file_on_a_normal_start() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.ron"), "(graphics: Direct3D12)").unwrap();
        let (automated, _) = windows(Program::Launcher, tmp.path(), RunKind::Automated);
        assert_eq!(automated.plan().order, [Direct3D12, ToolkitDefault]);
        assert!(!GraphicsSettings::path(tmp.path()).exists());

        let (startup, problems) = windows(Program::Launcher, tmp.path(), RunKind::Normal);
        assert!(problems.is_empty());
        assert_eq!(startup.plan().order, [Direct3D12, ToolkitDefault]);
        // The editor rewrites its file without the choice; the shared file keeps it.
        std::fs::write(tmp.path().join("settings.ron"), "()").unwrap();
        assert_eq!(
            GraphicsSettings::load(tmp.path()).choice,
            GraphicsChoice::Direct3D12
        );
    }

    #[test]
    fn the_warning_preview_asks_without_anything_having_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = prepare_on(Platform::Other, Program::Launcher, tmp.path(), RunKind::Automated);
        assert!(!startup.opened(ToolkitDefault, Vec::new()).ask);
        assert!(
            startup
                .with_warning_preview()
                .opened(ToolkitDefault, Vec::new())
                .ask
        );
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
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
    fn opening_the_device_here_gives_the_toolkit_a_device_or_a_reason() {
        // Without a real card with video decoding (none on the machines that
        // run the tests) this is a reason in words, never a failed start.
        match own_vulkan_device() {
            Ok((options, device)) => {
                assert!(matches!(options.wgpu_setup, egui_wgpu::WgpuSetup::Existing(_)));
                assert!(!device.codecs().is_empty());
            }
            Err(reason) => assert!(!reason.is_empty()),
        }
        // The launcher never gets there: it plays no video.
        assert!(Program::Editor.plays_video() && !Program::Launcher.plays_video());
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
}
