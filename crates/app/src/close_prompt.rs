//! The question before closing the window while exports are unfinished.
//!
//! Closing stops a running export and drops the ones waiting in the queue,
//! so a close request (the window's close button, Alt+F4, the task bar) is
//! held back first and the person is asked: keep exporting, or stop and
//! close. Closes the app asks for itself (the automated checks) are never
//! held back.

use crate::export_page::Unfinished;
use ui_kit::Theme;

#[derive(Debug, Default)]
pub struct ClosePrompt {
    /// The question is on screen.
    asking: bool,
    /// The next close request goes through: answered "Stop and close", or
    /// the app is closing itself.
    allowed: bool,
}

impl ClosePrompt {
    /// Lets the next close through without asking.
    pub fn allow(&mut self) {
        self.allowed = true;
    }

    /// Asks as if the window's close button had been pressed
    /// (`--close-request`, for screenshots of the question).
    pub fn ask(&mut self) {
        self.asking = true;
    }

    /// Call every frame: holds a close request back while exports are
    /// unfinished, and shows the question until it is answered.
    pub fn frame(&mut self, ctx: &egui::Context, theme: &Theme, unfinished: Option<Unfinished>) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if requested && !self.allowed && unfinished.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.asking = true;
        }
        if !self.asking {
            return;
        }
        // Everything finished while the question was open: nothing is lost
        // by closing now, which is what was asked for.
        let Some(unfinished) = unfinished else {
            self.close(ctx);
            return;
        };
        let (title, body) = words(unfinished);
        let (mut keep, mut stop) = (false, false);
        let modal = ui_kit::dialog(ctx, theme, "close-while-exporting", |ui| {
            ui.set_width(400.0);
            ui.label(
                egui::RichText::new(title)
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(body).color(theme.text_secondary));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                keep = ui_kit::accent_button(ui, theme, keep_words(unfinished)).clicked();
                stop = ui.button("Stop and close").clicked();
            });
        });
        if stop {
            self.close(ctx);
        } else if keep || modal.should_close() {
            self.asking = false;
        }
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.asking = false;
        self.allowed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// The question's title and what closing would do.
fn words(unfinished: Unfinished) -> (String, String) {
    let Unfinished { running, waiting } = unfinished;
    let title = match (running, waiting) {
        (true, 0) => "An export is running".to_owned(),
        (true, n) => format!("An export is running ({n} waiting)"),
        (false, 1) => "An export is waiting in the queue".to_owned(),
        (false, n) => format!("{n} exports are waiting in the queue"),
    };
    let body = match (running, waiting) {
        (true, 0) => "Closing stops it, and the part already written is taken away.",
        (true, _) => {
            "Closing stops it, and the part already written is taken away. The exports waiting are taken off the queue."
        }
        (false, 1) => "Closing takes it off the queue.",
        (false, _) => "Closing takes them off the queue.",
    };
    (title, body.to_owned())
}

/// The button that keeps the window open.
fn keep_words(unfinished: Unfinished) -> &'static str {
    if unfinished.running {
        "Keep exporting"
    } else {
        "Keep the queue"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unfinished(running: bool, waiting: usize) -> Unfinished {
        Unfinished { running, waiting }
    }

    #[test]
    fn the_question_says_what_closing_would_lose() {
        assert_eq!(words(unfinished(true, 0)).0, "An export is running");
        assert_eq!(words(unfinished(true, 2)).0, "An export is running (2 waiting)");
        assert!(words(unfinished(true, 2)).1.contains("waiting"));
        assert_eq!(words(unfinished(false, 1)).0, "An export is waiting in the queue");
        assert_eq!(
            words(unfinished(false, 3)).0,
            "3 exports are waiting in the queue"
        );
        assert_eq!(keep_words(unfinished(true, 1)), "Keep exporting");
        assert_eq!(keep_words(unfinished(false, 1)), "Keep the queue");
    }

    #[test]
    fn a_close_request_is_held_back_only_while_exports_are_unfinished() {
        let theme = ui_kit::ThemeName::default().theme();
        let close = |prompt: &mut ClosePrompt, unfinished: Option<Unfinished>| {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
            let mut output = ctx.run_ui(input, |ui| prompt.frame(ui.ctx(), &theme, unfinished));
            output.textures_delta.clear();
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose))
        };
        let mut prompt = ClosePrompt::default();
        assert!(!close(&mut prompt, None), "nothing to lose: it closes");
        assert!(
            close(&mut prompt, Some(unfinished(true, 1))),
            "held back and asked"
        );
        assert!(prompt.asking);
        let mut allowed = ClosePrompt::default();
        allowed.allow();
        assert!(
            !close(&mut allowed, Some(unfinished(true, 1))),
            "the app closing itself"
        );
    }
}
