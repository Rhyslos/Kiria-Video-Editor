//! The window's side: the same in the launcher and the editor.

use crate::{
    DecodeGuard, FRAMES_BEFORE_TRUSTED, GraphicsChoice, GraphicsSettings, Opened, Platform, RunKind,
    StartMarker, VideoDecoding,
};
use diag::AppError;
use eframe::wgpu;
use egui::RichText;
use std::path::PathBuf;
use std::sync::Arc;
use ui_kit::settings::{Row, Section, Setting};
use ui_kit::{ErrorCenter, Theme};
use video_device::VideoDevice;

/// The answers to "the window didn't open last time".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningAnswer {
    /// The safe choice, already saved when the question appears.
    KeepDirect3D12,
    /// Back to Automatic: the next start tries Vulkan, guarded by the marker.
    TryVulkanAgain,
}

/// The answers to "video decoding stopped the app last time".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodingAnswer {
    /// The safe choice, already saved when the question appears.
    KeepOff,
    /// Switched on again: the next start decodes on the window's device,
    /// guarded by its marker.
    TryAgain,
}

/// What the editor needs to decode video on the window's device.
#[derive(Debug, Clone)]
pub struct DeviceDecoding {
    pub device: Arc<VideoDevice>,
    /// Told before the device is asked to decode, and of every picture drawn.
    pub guard: Arc<DecodeGuard>,
}

/// What an open window knows and shows about graphics.
#[derive(Debug)]
pub struct WindowGraphics {
    /// The graphics system and card drawing the window, as Diagnostics shows it.
    in_use: String,
    /// A start with Vulkan that has not drawn its first frames yet.
    marker: Option<StartMarker>,
    /// The question about a failed start with Vulkan is open.
    asking: bool,
    /// Whether the window's device can decode video itself.
    video: VideoDecoding,
    /// Decoding on the window's device in this session.
    decoding: Option<DeviceDecoding>,
    /// Why not, when the device could: for Settings and Diagnostics.
    decoding_off: Option<&'static str>,
    /// The question about a session that stopped while decoding there is open.
    asking_decoding: bool,
    platform: Platform,
    settings: GraphicsSettings,
    data_dir: PathBuf,
    run: RunKind,
}

impl WindowGraphics {
    /// Finds out which graphics system and card draw the window and records
    /// it where it will be looked for: the log, crash reports and Diagnostics.
    pub fn new(cc: &eframe::CreationContext<'_>, opened: Opened, errors: &mut ErrorCenter) -> WindowGraphics {
        let in_use = cc.wgpu_render_state.as_ref().map_or_else(
            || "not known".to_owned(),
            |state| in_use(&state.adapter.get_info()),
        );
        tracing::info!(graphics = %in_use, "the window is open");
        diag::crash::set_gpu_info(in_use.clone());
        errors.set_fact("Graphics", in_use.clone());
        if let Some(failed) = failed_text(&opened) {
            errors.set_fact("Tried first", failed);
        }
        if opened.ask {
            errors.set_fact(
                "Last start",
                "the window never appeared while using Vulkan, so Direct3D 12 took over",
            );
        }
        let video = opened.video.describe();
        tracing::info!(video_decoding = %video, "video decoding on the window's device");
        errors.set_fact(VIDEO_DECODING_FACT, video);
        if let Some(notice) = vulkan_notice(&opened) {
            errors.push(&notice);
        }
        let window = WindowGraphics::from_parts(in_use, opened);
        let used = window.decoding_in_use();
        tracing::info!(in_use = %used, "decoding video on the window's device");
        errors.set_fact(DECODING_THERE_FACT, used);
        window
    }

    fn from_parts(in_use: String, opened: Opened) -> WindowGraphics {
        let decoding_off = if opened.ask_decoding {
            Some("off, because it stopped the app last time")
        } else if !opened.settings.decode_on_window_device {
            Some("off, as chosen in Settings")
        } else if !opened.decode_on_device {
            Some("off for now, because a session did not get through its first pictures")
        } else {
            None
        };
        let decoding = match (opened.video.device(), opened.decode_on_device) {
            (Some(device), true) => Some(DeviceDecoding {
                device: device.clone(),
                guard: Arc::clone(&opened.decode_guard),
            }),
            _ => None,
        };
        WindowGraphics {
            in_use,
            marker: opened.marker,
            asking: opened.ask,
            video: opened.video,
            decoding,
            decoding_off,
            asking_decoding: opened.ask_decoding,
            platform: opened.platform,
            settings: opened.settings,
            data_dir: opened.data_dir,
            run: opened.run,
        }
    }

    /// The graphics system and card in use, as one line.
    pub fn in_use(&self) -> &str {
        &self.in_use
    }

    /// Whether the window's device was opened for decoding video too, with
    /// the handle the decoder needs, or why not.
    pub fn video_decoding(&self) -> &VideoDecoding {
        &self.video
    }

    /// What the editor decodes video on the window's device with, when the
    /// device can, the setting allows it and it didn't fail last time.
    pub fn device_decoding(&self) -> Option<&DeviceDecoding> {
        self.decoding.as_ref()
    }

    /// Whether video is decoded on the window's device in this session, in
    /// a few words: for Diagnostics, the log and Settings.
    fn decoding_in_use(&self) -> String {
        match (&self.video, self.decoding_off) {
            (VideoDecoding::NotAvailable(why), _) => format!("not possible ({why})"),
            (VideoDecoding::OnDevice(_), Some(why)) => why.to_owned(),
            (VideoDecoding::OnDevice(device), None) => format!("on, for {}", device.codec_names()),
        }
    }

    /// Whether one of the questions about last time is on screen.
    pub fn is_asking(&self) -> bool {
        self.asking || self.asking_decoding
    }

    /// Call at the top of every frame with the number of frames drawn so far.
    /// A start with Vulkan counts as working once its first frames are drawn.
    pub fn frame(&mut self, ctx: &egui::Context, frames_drawn: u64) {
        let Some(marker) = &self.marker else {
            return;
        };
        if frames_drawn < FRAMES_BEFORE_TRUSTED {
            ctx.request_repaint(); // get there without waiting for input
            return;
        }
        if let Err(err) = marker.clear() {
            tracing::warn!("{}", err.report());
        }
        self.marker = None;
    }

    /// Draws the question about a failed start with Vulkan while it is open:
    /// what happened, what is in use now, and the two choices. Clicking
    /// outside or Escape keeps Direct3D 12. Returns a problem saving the answer.
    pub fn show_warning(&mut self, ctx: &egui::Context, theme: &Theme) -> Option<AppError> {
        if !self.asking {
            // One question at a time: this one first.
            return self.show_decoding_warning(ctx, theme);
        }
        let mut answer = None;
        let modal = ui_kit::dialog(ctx, theme, "graphics-warning", |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new("The window didn't open last time")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "It was being drawn with Vulkan and stopped before anything appeared, which usually means a problem in the graphics driver. This time Direct3D 12 is used instead, and it stays that way until you choose otherwise.",
                )
                .color(theme.text_secondary),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(format!("In use now: {}", self.in_use)).color(theme.muted));
            ui.label(
                RichText::new("You can change this later in Settings, under Graphics.").color(theme.muted),
            );
            ui.add_space(14.0);
            // The buttons come last: a row laid out from the right takes the height that is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, theme, "Keep Direct3D 12").clicked() {
                    answer = Some(WarningAnswer::KeepDirect3D12);
                }
                if ui
                    .button("Try Vulkan again next time")
                    .on_hover_text(
                        "The next start uses Vulkan. If the window doesn't open then either, the start after it uses Direct3D 12 and asks again.",
                    )
                    .clicked()
                {
                    answer = Some(WarningAnswer::TryVulkanAgain);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(WarningAnswer::KeepDirect3D12);
        }
        answer.and_then(|answer| self.answer(answer).err())
    }

    /// Takes the answer and closes the question. Nothing is saved in an automated run.
    pub fn answer(&mut self, answer: WarningAnswer) -> diag::Result<()> {
        self.asking = false;
        tracing::info!(?answer, "answered the question about Vulkan");
        match answer {
            WarningAnswer::KeepDirect3D12 => Ok(()),
            WarningAnswer::TryVulkanAgain => self.choose(GraphicsChoice::Automatic),
        }
    }

    fn choose(&mut self, choice: GraphicsChoice) -> diag::Result<()> {
        self.settings.choice = choice;
        self.save()
    }

    fn save(&self) -> diag::Result<()> {
        match self.run {
            RunKind::Normal => self.settings.save(&self.data_dir),
            RunKind::Automated => Ok(()),
        }
    }

    /// Draws the question about a session that stopped while decoding video
    /// on the window's device: what happened, what is done now, and the two
    /// choices. Clicking outside or Escape keeps it off.
    fn show_decoding_warning(&mut self, ctx: &egui::Context, theme: &Theme) -> Option<AppError> {
        if !self.asking_decoding {
            return None;
        }
        let mut answer = None;
        let modal = ui_kit::dialog(ctx, theme, "decoding-warning", |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new("The app stopped while showing video last time")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "It was decoding video on the window's graphics device, the fastest way, and stopped before the first pictures were shown, which usually means a problem in the graphics driver. That way of decoding is switched off now, and stays off until you choose otherwise. Video still plays: it is decoded the way it was before.",
                )
                .color(theme.text_secondary),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("You can change this later in Settings, under Graphics.").color(theme.muted),
            );
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, theme, "Keep it off").clicked() {
                    answer = Some(DecodingAnswer::KeepOff);
                }
                if ui
                    .button("Try it again next time")
                    .on_hover_text(
                        "The next start decodes on the graphics device again. If the app stops then too, the start after it switches this off and asks again.",
                    )
                    .clicked()
                {
                    answer = Some(DecodingAnswer::TryAgain);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(DecodingAnswer::KeepOff);
        }
        answer.and_then(|answer| self.answer_decoding(answer).err())
    }

    /// Takes the answer and closes the question. Nothing is saved in an automated run.
    pub fn answer_decoding(&mut self, answer: DecodingAnswer) -> diag::Result<()> {
        self.asking_decoding = false;
        tracing::info!(
            ?answer,
            "answered the question about decoding on the window's device"
        );
        match answer {
            DecodingAnswer::KeepOff => Ok(()),
            DecodingAnswer::TryAgain => self.decode_on_device(true),
        }
    }

    /// Saves the setting. This session keeps decoding the way it started.
    fn decode_on_device(&mut self, wanted: bool) -> diag::Result<()> {
        self.settings.decode_on_window_device = wanted;
        self.save()
    }

    /// Draws the control of one of [`SETTINGS`]' rows of Settings (the
    /// same in the launcher and the editor); other rows are left alone.
    /// Returns a problem saving a change.
    pub fn settings_row(&mut self, row: &mut Row<'_>) -> Option<AppError> {
        // An automated run shows the rows as they are; a change there is
        // never saved (`save`).
        match row.id() {
            GRAPHICS_ROW => {
                let has_choice = self.platform == Platform::Windows;
                let shown = if has_choice {
                    self.settings.choice
                } else {
                    GraphicsChoice::Automatic
                };
                let mut chosen = shown;
                if !has_choice {
                    row.disable("The choice between Vulkan and Direct3D 12 is for Windows only.");
                    row.describe("The choice between Vulkan and Direct3D 12 is for Windows only.");
                }
                let options: Vec<(GraphicsChoice, &str)> =
                    GraphicsChoice::ALL.iter().map(|c| (*c, c.label())).collect();
                ui_kit::settings::segmented(row.ui, row.theme, &mut chosen, &options);
                row.note(format!("In use now: {}", self.in_use));
                (chosen != shown).then(|| self.choose(chosen).err()).flatten()
            }
            DECODING_ROW => {
                let mut wanted = self.settings.decode_on_window_device;
                let changed = ui_kit::settings::toggle(row.ui, row.theme, &mut wanted).changed();
                row.note(format!("In this window: {}", self.decoding_in_use()));
                changed.then(|| self.decode_on_device(wanted).err()).flatten()
            }
            _ => None,
        }
    }
}

/// The id of the row that chooses the graphics system.
const GRAPHICS_ROW: &str = "graphics-system";
/// The id of the switch for decoding on the window's device.
const DECODING_ROW: &str = "decode-on-device";

/// The words on the switch in Settings.
pub const DECODING_SWITCH: &str = "Decode video on the window's graphics device";

/// Settings' Graphics card, the same in the launcher and the editor; the
/// controls are [`WindowGraphics::settings_row`].
pub const SETTINGS: Section = Section {
    id: "graphics-drawing",
    title: "Drawing and decoding",
    settings: &[
        Setting::new(
            GRAPHICS_ROW,
            "Draw the windows with",
            "Automatic uses Vulkan, and Direct3D 12 if Vulkan doesn't start. For every window.",
        )
        .keywords("vulkan direct3d d3d12 graphics card gpu driver renderer")
        .restart(),
        Setting::new(
            DECODING_ROW,
            DECODING_SWITCH,
            "Fastest: pictures go from the decoder to the screen without leaving the card. Turn it off if video shows wrong colours, stutters or stops the app.",
        )
        .keywords("hardware decoding gpu acceleration video card")
        .restart(),
    ],
};

/// The name of the Diagnostics line that says whether video is decoded on
/// the window's device in this session.
const DECODING_THERE_FACT: &str = "Viewer decodes on this device";

/// The name of the Diagnostics line that says which kinds of video the
/// window's device decodes, or why none.
const VIDEO_DECODING_FACT: &str = "Video decoding on this device";

/// One line per graphics system that was tried first, for Diagnostics.
fn failed_text(opened: &Opened) -> Option<String> {
    let lines: Vec<String> = opened
        .failed
        .iter()
        .map(|f| format!("{}: {}", f.backend.label(), f.reason))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// What to tell someone who chose Vulkan and got something else. Automatic
/// falls back quietly: Diagnostics and the log say what happened.
fn vulkan_notice(opened: &Opened) -> Option<AppError> {
    let vulkan = opened
        .failed
        .iter()
        .find(|f| f.backend == crate::Backend::Vulkan)?;
    (opened.settings.choice == GraphicsChoice::Vulkan).then(|| {
        AppError::new(format!(
            "Vulkan didn't start, so the window is drawn with {}",
            opened.backend.label()
        ))
        .why(format!("The graphics system reported: {}", vulkan.reason))
        .fix("Update the graphics driver, or choose Automatic under Settings, Graphics.")
    })
}

/// The graphics system and card in use, as one line: for Diagnostics, the
/// log and crash reports.
fn in_use(info: &wgpu::AdapterInfo) -> String {
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
    use crate::start::prepare_on;
    use crate::{Backend, FailedTry, Program};
    use Backend::{Direct3D12, ToolkitDefault, Vulkan};

    /// A start on Windows after `program`'s start with Vulkan never drew a
    /// frame, as far as the open window with its question.
    fn asking_after_a_crash(program: Program, dir: &std::path::Path) -> WindowGraphics {
        StartMarker::in_dir(program, dir).write().unwrap();
        let (startup, problems) = prepare_on(Platform::Windows, program, dir, RunKind::Normal);
        assert!(problems.is_empty());
        let window = WindowGraphics::from_parts("Direct3D 12".into(), startup.opened(Direct3D12, Vec::new()));
        assert!(window.is_asking());
        window
    }

    fn next_start(program: Program, dir: &std::path::Path) -> crate::Plan {
        prepare_on(Platform::Windows, program, dir, RunKind::Normal)
            .0
            .plan()
            .clone()
    }

    #[test]
    fn keeping_direct3d_12_holds_for_both_programs_and_asks_no_more() {
        for program in [Program::Launcher, Program::Editor] {
            let tmp = tempfile::tempdir().unwrap();
            let mut window = asking_after_a_crash(program, tmp.path());
            window.answer(WarningAnswer::KeepDirect3D12).unwrap();
            assert!(!window.is_asking());
            for next in [Program::Launcher, Program::Editor] {
                let plan = next_start(next, tmp.path());
                assert_eq!(plan.order, [Direct3D12, ToolkitDefault]);
                assert!(!plan.ask && !plan.guard);
            }
        }
    }

    #[test]
    fn trying_vulkan_again_guards_the_next_start_and_a_second_failure_asks_again() {
        for program in [Program::Launcher, Program::Editor] {
            let tmp = tempfile::tempdir().unwrap();
            let mut window = asking_after_a_crash(program, tmp.path());
            window.answer(WarningAnswer::TryVulkanAgain).unwrap();
            assert_eq!(
                GraphicsSettings::load(tmp.path()).choice,
                GraphicsChoice::Automatic
            );

            let plan = next_start(program, tmp.path());
            assert_eq!(plan.order, [Vulkan, Direct3D12, ToolkitDefault]);
            assert!(plan.guard && !plan.ask);

            // That start crashes too: the marker it wrote brings the question back.
            let again = asking_after_a_crash(program, tmp.path());
            assert_eq!(
                GraphicsSettings::load(tmp.path()).choice,
                GraphicsChoice::Direct3D12
            );
            drop(again);
        }
    }

    #[test]
    fn an_automated_run_shows_the_question_but_saves_no_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = prepare_on(
            Platform::Windows,
            Program::Launcher,
            tmp.path(),
            RunKind::Automated,
        );
        let opened = startup.with_warning_preview().opened(Vulkan, Vec::new());
        let mut window = WindowGraphics::from_parts("Vulkan".into(), opened);
        assert!(window.is_asking());
        window.answer(WarningAnswer::TryVulkanAgain).unwrap();
        assert!(!window.is_asking());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    /// The editor's window on Windows after its last session stopped while
    /// decoding its first pictures on the device.
    fn asking_after_decoding_stopped(dir: &std::path::Path) -> WindowGraphics {
        StartMarker::for_decoding(Program::Editor, dir).write().unwrap();
        let (startup, problems) = prepare_on(Platform::Windows, Program::Editor, dir, RunKind::Normal);
        assert!(problems.is_empty());
        let window = WindowGraphics::from_parts("Vulkan".into(), startup.opened(Vulkan, Vec::new()));
        assert!(window.is_asking() && window.device_decoding().is_none());
        window
    }

    #[test]
    fn keeping_decoding_off_asks_no_more_and_trying_again_guards_the_next_session() {
        let tmp = tempfile::tempdir().unwrap();
        let mut window = asking_after_decoding_stopped(tmp.path());
        window.answer_decoding(DecodingAnswer::KeepOff).unwrap();
        assert!(!window.is_asking());
        let plan = next_start(Program::Editor, tmp.path());
        assert!(!plan.decode_on_device && !plan.ask_decoding);
        assert_eq!(plan.order[0], Vulkan, "the window itself was never the problem");

        // Switched on again in Settings: the next session is guarded, and
        // one that stops too brings the question back.
        window.decode_on_device(true).unwrap();
        assert!(next_start(Program::Editor, tmp.path()).decode_on_device);
        let mut window = asking_after_decoding_stopped(tmp.path());
        assert!(!GraphicsSettings::load(tmp.path()).decode_on_window_device);

        // The other answer switches it on again as well.
        window.answer_decoding(DecodingAnswer::TryAgain).unwrap();
        assert!(GraphicsSettings::load(tmp.path()).decode_on_window_device);
        let plan = next_start(Program::Editor, tmp.path());
        assert!(plan.decode_on_device && !plan.ask_decoding);
    }

    #[test]
    fn the_question_about_vulkan_comes_before_the_one_about_decoding() {
        let tmp = tempfile::tempdir().unwrap();
        StartMarker::for_decoding(Program::Editor, tmp.path())
            .write()
            .unwrap();
        let mut window = asking_after_a_crash(Program::Editor, tmp.path());
        assert!(window.asking && window.asking_decoding);
        window.answer(WarningAnswer::KeepDirect3D12).unwrap();
        assert!(window.is_asking(), "the second question is still open");
        window.answer_decoding(DecodingAnswer::KeepOff).unwrap();
        assert!(!window.is_asking());
    }

    #[test]
    fn the_switch_in_settings_is_saved_and_says_what_this_window_does() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = prepare_on(Platform::Windows, Program::Editor, tmp.path(), RunKind::Normal);
        let mut window = WindowGraphics::from_parts("Vulkan".into(), startup.opened(Vulkan, Vec::new()));
        // The toolkit opened this device, so there is nothing to decode on.
        assert_eq!(
            window.decoding_in_use(),
            "not possible (the device was not opened for it)"
        );
        window.decode_on_device(false).unwrap();
        assert!(!GraphicsSettings::load(tmp.path()).decode_on_window_device);
        assert!(!next_start(Program::Editor, tmp.path()).decode_on_device);
        // The other setting in the same file is left as it was.
        assert_eq!(
            GraphicsSettings::load(tmp.path()).choice,
            GraphicsChoice::Automatic
        );
        window.decode_on_device(true).unwrap();
        assert!(next_start(Program::Editor, tmp.path()).decode_on_device);
    }

    #[test]
    fn a_device_that_can_decode_is_handed_out_only_when_the_plan_allows() {
        let Some(device) = video_device::testing::device() else {
            eprintln!("no Vulkan device here: handing it out isn't checked");
            return;
        };
        let opened = |dir: &std::path::Path, run| {
            let (startup, _) = prepare_on(Platform::Windows, Program::Editor, dir, run);
            let mut opened = startup.opened(Vulkan, Vec::new());
            opened.video = VideoDecoding::OnDevice(device.clone());
            opened
        };
        let tmp = tempfile::tempdir().unwrap();
        let window = WindowGraphics::from_parts("Vulkan".into(), opened(tmp.path(), RunKind::Normal));
        let decoding = window.device_decoding().unwrap();
        assert!(window.decoding_in_use().starts_with("on, for "));
        // The guard it comes with writes the editor's marker.
        decoding.guard.about_to_decode();
        let marker = StartMarker::for_decoding(Program::Editor, tmp.path());
        assert!(marker.is_present());
        decoding.guard.reported_an_error();
        assert!(!marker.is_present());

        // An automated run decodes there too, and writes nothing.
        let quiet = tempfile::tempdir().unwrap();
        let window = WindowGraphics::from_parts("Vulkan".into(), opened(quiet.path(), RunKind::Automated));
        window.device_decoding().unwrap().guard.about_to_decode();
        assert_eq!(std::fs::read_dir(quiet.path()).unwrap().count(), 0);

        // Turned off, or stopped last time: no handle.
        window_off(tmp.path());
        let window = WindowGraphics::from_parts("Vulkan".into(), opened(tmp.path(), RunKind::Normal));
        assert!(window.device_decoding().is_none());
        assert_eq!(window.decoding_in_use(), "off, as chosen in Settings");
        let stopped = tempfile::tempdir().unwrap();
        marker_left(stopped.path());
        let window = WindowGraphics::from_parts("Vulkan".into(), opened(stopped.path(), RunKind::Normal));
        assert!(window.device_decoding().is_none() && window.is_asking());
        assert_eq!(
            window.decoding_in_use(),
            "off, because it stopped the app last time"
        );
    }

    fn window_off(dir: &std::path::Path) {
        GraphicsSettings {
            decode_on_window_device: false,
            ..GraphicsSettings::default()
        }
        .save(dir)
        .unwrap();
    }

    fn marker_left(dir: &std::path::Path) {
        StartMarker::for_decoding(Program::Editor, dir).write().unwrap();
    }

    #[test]
    fn a_change_in_settings_is_saved_for_both_programs() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = prepare_on(Platform::Windows, Program::Editor, tmp.path(), RunKind::Normal);
        let mut window = WindowGraphics::from_parts("Vulkan".into(), startup.opened(Vulkan, Vec::new()));
        window.choose(GraphicsChoice::Direct3D12).unwrap();
        assert_eq!(
            next_start(Program::Launcher, tmp.path()).order,
            [Direct3D12, ToolkitDefault]
        );
        window.choose(GraphicsChoice::Vulkan).unwrap();
        assert_eq!(next_start(Program::Launcher, tmp.path()).order[0], Vulkan);
    }

    #[test]
    fn the_marker_goes_after_the_first_frames_and_not_before() {
        let tmp = tempfile::tempdir().unwrap();
        let (startup, _) = prepare_on(Platform::Windows, Program::Launcher, tmp.path(), RunKind::Normal);
        let marker = StartMarker::in_dir(Program::Launcher, tmp.path());
        marker.write().unwrap(); // as `open_window` does before trying Vulkan
        let mut window = WindowGraphics::from_parts("Vulkan".into(), startup.opened(Vulkan, Vec::new()));
        let ctx = egui::Context::default();
        for drawn in 0..FRAMES_BEFORE_TRUSTED {
            window.frame(&ctx, drawn);
            assert!(marker.is_present(), "after {drawn} frames");
        }
        window.frame(&ctx, FRAMES_BEFORE_TRUSTED);
        assert!(!marker.is_present());
    }

    #[test]
    fn only_someone_who_chose_vulkan_is_told_when_it_did_not_start() {
        let tmp = tempfile::tempdir().unwrap();
        let failed = vec![FailedTry {
            backend: Vulkan,
            reason: "no driver".into(),
        }];
        let (startup, _) = prepare_on(Platform::Windows, Program::Editor, tmp.path(), RunKind::Normal);
        let automatic = startup.opened(Direct3D12, failed.clone());
        assert!(vulkan_notice(&automatic).is_none());
        assert_eq!(failed_text(&automatic).as_deref(), Some("Vulkan: no driver"));

        GraphicsSettings {
            choice: GraphicsChoice::Vulkan,
            ..GraphicsSettings::default()
        }
        .save(tmp.path())
        .unwrap();
        let (startup, _) = prepare_on(Platform::Windows, Program::Editor, tmp.path(), RunKind::Normal);
        let notice = vulkan_notice(&startup.opened(Direct3D12, failed)).unwrap();
        assert!(notice.what().contains("Direct3D 12"));
        assert!(notice.why_text().unwrap().contains("no driver"));
        let first_try = startup.opened(Vulkan, Vec::new());
        assert!(vulkan_notice(&first_try).is_none());
        assert_eq!(failed_text(&first_try), None);
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
