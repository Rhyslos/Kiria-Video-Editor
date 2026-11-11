//! The window's side: the same in the launcher and the editor.

use crate::{
    FRAMES_BEFORE_TRUSTED, GraphicsChoice, GraphicsSettings, Opened, Platform, RunKind, StartMarker,
};
use diag::AppError;
use eframe::wgpu;
use egui::RichText;
use std::path::PathBuf;
use ui_kit::{ErrorCenter, Theme};

/// The answers to "the window didn't open last time".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningAnswer {
    /// The safe choice, already saved when the question appears.
    KeepDirect3D12,
    /// Back to Automatic: the next start tries Vulkan, guarded by the marker.
    TryVulkanAgain,
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
        if let Some(notice) = vulkan_notice(&opened) {
            errors.push(&notice);
        }
        WindowGraphics::from_parts(in_use, opened)
    }

    fn from_parts(in_use: String, opened: Opened) -> WindowGraphics {
        WindowGraphics {
            in_use,
            marker: opened.marker,
            asking: opened.ask,
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

    /// Whether the question about a failed start is on screen.
    pub fn is_asking(&self) -> bool {
        self.asking
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
            return None;
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
        match self.run {
            RunKind::Normal => self.settings.save(&self.data_dir),
            RunKind::Automated => Ok(()),
        }
    }

    /// The Graphics row in Settings: the choice (Windows only), when it
    /// applies, and what is in use now. Returns a problem saving a change.
    pub fn settings_row(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<AppError> {
        let has_choice = self.platform == Platform::Windows;
        let shown = if has_choice {
            self.settings.choice
        } else {
            GraphicsChoice::Automatic
        };
        let mut chosen = shown;
        ui.add_enabled_ui(has_choice && self.run == RunKind::Normal, |ui| {
            egui::ComboBox::from_label("Draw the windows with")
                .selected_text(shown.label())
                .show_ui(ui, |ui| {
                    for choice in GraphicsChoice::ALL {
                        ui.selectable_value(&mut chosen, choice, choice.label());
                    }
                });
        });
        let hint = if has_choice {
            "Automatic uses Vulkan, and Direct3D 12 if Vulkan doesn't start. A change applies to every window, from the next time the app starts."
        } else {
            "The choice between Vulkan and Direct3D 12 is for Windows only."
        };
        ui.label(RichText::new(hint).color(theme.muted));
        ui.label(RichText::new(format!("In use now: {}", self.in_use)).color(theme.muted));
        (chosen != shown).then(|| self.choose(chosen).err()).flatten()
    }
}

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
