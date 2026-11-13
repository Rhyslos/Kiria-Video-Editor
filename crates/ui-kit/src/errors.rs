//! Showing explained errors: an inline message with a fix button, and the
//! Diagnostics window that lists every error of the session.

use crate::theme::{Theme, status};
use diag::{AppError, FixAction};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use std::time::SystemTime;

/// An error as the interface shows it.
#[derive(Debug, Clone)]
pub struct ShownError {
    pub what: String,
    pub why: Option<String>,
    pub fix: String,
    pub action: Option<FixAction>,
    /// Everything, for "Copy details" and bug reports.
    pub details: String,
    pub at: SystemTime,
    pub dismissed: bool,
}

impl From<&AppError> for ShownError {
    fn from(err: &AppError) -> Self {
        ShownError {
            what: err.what().to_owned(),
            why: err.why_text().map(str::to_owned),
            fix: err.fix_text().to_owned(),
            action: err.fix_action().cloned(),
            details: err.report(),
            at: SystemTime::now(),
            dismissed: false,
        }
    }
}

/// What the person clicked on an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorResponse {
    /// Run this fix.
    Fix(FixAction),
    OpenDiagnostics,
}

/// The session's errors, and a few facts about the session worth having
/// beside them (which graphics system draws the window, for one).
#[derive(Debug, Default)]
pub struct ErrorCenter {
    errors: Vec<ShownError>,
    facts: Vec<(String, String)>,
}

impl ErrorCenter {
    /// Records an error, logs it, and shows it inline until dismissed.
    pub fn push(&mut self, err: &AppError) {
        tracing::warn!("{}", err.report());
        self.errors.push(ShownError::from(err));
    }

    pub fn all(&self) -> &[ShownError] {
        &self.errors
    }

    /// Sets a line at the top of the Diagnostics window that is not a
    /// problem, such as "Graphics" and what is in use. A name set twice
    /// keeps its place and takes the new value.
    pub fn set_fact(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.facts.iter_mut().find(|(n, _)| n == name) {
            Some(fact) => fact.1 = value,
            None => self.facts.push((name.to_owned(), value)),
        }
    }

    /// Everything "Copy all details" copies: the facts, then every error.
    pub fn all_details(&self) -> String {
        let facts: Vec<String> = self
            .facts
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        let facts = (!facts.is_empty()).then(|| facts.join("\n"));
        let errors = self.errors.iter().map(|e| e.details.clone());
        facts.into_iter().chain(errors).collect::<Vec<_>>().join("\n\n")
    }

    /// As [`ErrorCenter::show_inline`], floating over the top of what is
    /// left of `ui` instead of taking room in it: the page under it never
    /// moves when an error comes or goes. For pages whose parts are laid
    /// out to fixed heights (a viewer over a timeline).
    pub fn show_over(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<ErrorResponse> {
        if self.errors.iter().all(|e| e.dismissed) {
            return None;
        }
        let rect = ui.available_rect_before_wrap();
        let mut response = None;
        egui::Area::new(ui.scope_id().with("error-over-the-page"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_width(rect.width());
                response = self.show_inline(ui, theme);
            });
        response
    }

    /// The newest error not yet dismissed, as a strip across the page.
    pub fn show_inline(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<ErrorResponse> {
        let error = self.errors.iter_mut().rev().find(|e| !e.dismissed)?;
        let mut response = None;
        Frame::new()
            .fill(theme.raised)
            .stroke(Stroke::new(1.0, theme.line_strong))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (bar, _) = ui.allocate_exact_size(egui::vec2(4.0, 36.0), egui::Sense::hover());
                    ui.painter().rect_filled(bar, 2.0, status::ERROR);
                    // Room for the buttons on the right, so long text wraps
                    // before them instead of running under them.
                    let buttons = if error.action.is_some() { 340.0 } else { 170.0 };
                    let text_width = (ui.available_width() - buttons).max(200.0);
                    ui.vertical(|ui| {
                        ui.set_max_width(text_width);
                        ui.label(RichText::new(&error.what).family(crate::fonts::semibold()));
                        if let Some(why) = &error.why {
                            ui.label(RichText::new(why).color(theme.text_secondary));
                        }
                        ui.label(RichText::new(format!("Fix: {}", error.fix)).color(theme.muted));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let dismiss = error
                            .action
                            .as_ref()
                            .and_then(|a| a.decline.as_deref())
                            .unwrap_or("Dismiss");
                        if ui.button(dismiss).clicked() {
                            error.dismissed = true;
                        }
                        if ui.button("Details").clicked() {
                            response = Some(ErrorResponse::OpenDiagnostics);
                        }
                        if let Some(action) = &error.action
                            && accent_button(ui, theme, &action.label).clicked()
                        {
                            response = Some(ErrorResponse::Fix(action.clone()));
                            error.dismissed = true;
                        }
                    });
                });
            });
        response
    }

    /// The Diagnostics window: every error this session, with copyable details.
    pub fn diagnostics_window(&mut self, ctx: &egui::Context, open: &mut bool, theme: &Theme) {
        egui::Window::new("Diagnostics")
            .open(open)
            .default_width(560.0)
            .show(ctx, |ui| {
                for (name, value) in &self.facts {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(name).color(theme.muted));
                        ui.label(value);
                    });
                }
                if !self.facts.is_empty() {
                    ui.add_space(4.0);
                }
                if self.errors.is_empty() {
                    ui.label(RichText::new("No problems this session.").color(theme.muted));
                    if !self.facts.is_empty() && ui.button("Copy all details").clicked() {
                        ctx.copy_text(self.all_details());
                    }
                    return;
                }
                if ui.button("Copy all details").clicked() {
                    ctx.copy_text(self.all_details());
                }
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for error in self.errors.iter().rev() {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(humantime_ago(error.at))
                                    .font(crate::fonts::mono(11.0))
                                    .color(theme.muted),
                            );
                            ui.label(RichText::new(&error.what).family(crate::fonts::semibold()));
                        });
                        ui.label(RichText::new(&error.details).font(crate::fonts::mono(11.0)));
                        if ui.small_button("Copy details").clicked() {
                            ctx.copy_text(error.details.clone());
                        }
                    }
                });
            });
    }
}

/// A button filled with the accent colour: the main action of a group.
pub fn accent_button(ui: &mut egui::Ui, theme: &Theme, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .color(theme.accent_ink)
                .family(crate::fonts::semibold()),
        )
        .fill(theme.accent)
        .stroke(Stroke::NONE),
    )
}

fn humantime_ago(at: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(at).map_or(0, |d| d.as_secs());
    crate::time_text::ago(secs)
}

/// A small dot in a status colour, for readouts.
pub fn status_dot(ui: &mut egui::Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, colour);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_errors_keep_every_part() {
        let err = AppError::new("Export can't start")
            .why("Two clips are offline")
            .fix("Relink them")
            .action(FixAction::new("Relink", "media.relink"));
        let shown = ShownError::from(&err);
        assert_eq!(shown.what, "Export can't start");
        assert_eq!(shown.why.as_deref(), Some("Two clips are offline"));
        assert_eq!(shown.fix, "Relink them");
        assert_eq!(shown.action.map(|a| a.command), Some("media.relink".into()));
        assert!(shown.details.contains("Fix: Relink them"));
    }

    #[test]
    fn facts_come_first_in_the_copied_details_and_a_name_is_kept_once() {
        let mut center = ErrorCenter::default();
        assert_eq!(center.all_details(), "");
        center.set_fact("Graphics", "one");
        center.set_fact("Tried first", "another");
        center.set_fact("Graphics", "two");
        assert_eq!(center.all_details(), "Graphics: two\nTried first: another");
        center.push(&AppError::new("Export can't start").fix("Relink them"));
        let details = center.all_details();
        assert!(details.starts_with("Graphics: two\nTried first: another\n\nExport can't start"));
    }

    /// The error floats over the page: what is laid out after it starts
    /// where it would without an error.
    #[test]
    fn an_error_over_the_page_takes_no_room_in_it() {
        let ctx = egui::Context::default();
        let mut center = ErrorCenter::default();
        center.push(&AppError::new("There is already a clip there").fix("Move it elsewhere."));
        let mut tops = Vec::new();
        for over in [true, false] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                if over {
                    center.show_over(ui, &Theme::ASH);
                } else {
                    center.show_inline(ui, &Theme::ASH);
                }
                tops.push(ui.available_rect_before_wrap().top());
            });
            output.drop_without_applying_deltas();
        }
        assert!(tops[0] < 10.0, "nothing pushed down: {tops:?}");
        assert!(tops[1] > 40.0, "inline takes room: {tops:?}");
    }
}
