//! Showing explained errors: an inline message with a fix button, and the
//! Diagnostics window that lists the session's errors.
//!
//! An error equal to one listed already (the same what and why) is folded
//! into it, counted, instead of added: something failing every frame is one
//! line, not thousands. The list keeps the newest [`ErrorCenter::KEPT`].

use crate::theme::{Theme, status};
use diag::{AppError, FixAction};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use std::time::{Duration, SystemTime};

/// An error as the interface shows it.
#[derive(Debug, Clone)]
pub struct ShownError {
    pub what: String,
    pub why: Option<String>,
    pub fix: String,
    pub action: Option<FixAction>,
    /// Everything, for "Copy details" and bug reports.
    pub details: String,
    /// When it last happened.
    pub at: SystemTime,
    /// How many times it happened (1 for once).
    pub count: u32,
    pub dismissed: bool,
}

impl ShownError {
    /// The same problem: the same what and why.
    fn same_as(&self, other: &ShownError) -> bool {
        self.what == other.what && self.why == other.why
    }

    /// The headline, with how many times it happened when more than once.
    fn headline(&self) -> String {
        if self.count > 1 {
            format!("{} (×{})", self.what, self.count)
        } else {
            self.what.clone()
        }
    }
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
            count: 1,
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
    /// How many errors the list keeps; the oldest go first.
    pub const KEPT: usize = 200;

    /// How long after its last time an error must come again to show again
    /// once dismissed. A repeat sooner (something failing every frame) only
    /// adds to its count; a repeat after a pause is the person trying again
    /// (or a fix that failed the same way), and must not fail silently.
    pub const QUIET: Duration = Duration::from_secs(1);

    /// Records an error, logs it, and shows it inline until dismissed. An
    /// error equal to one listed already is counted on that one, which
    /// becomes the newest; dismissed, it shows again when it comes at least
    /// [`ErrorCenter::QUIET`] after its last time.
    pub fn push(&mut self, err: &AppError) {
        if self.record(ShownError::from(err)) {
            tracing::warn!("{}", err.report());
        } else {
            tracing::debug!("again: {}", err.what());
        }
    }

    /// Adds `shown`, or folds it into the same error listed already. True
    /// when it is new, or a repeat [`ErrorCenter::QUIET`] or more after its
    /// last time.
    fn record(&mut self, mut shown: ShownError) -> bool {
        let fresh = match self.errors.iter().rposition(|e| e.same_as(&shown)) {
            Some(index) => {
                let old = self.errors.remove(index);
                let quiet = shown
                    .at
                    .duration_since(old.at)
                    .is_ok_and(|gap| gap >= Self::QUIET);
                shown.count = old.count.saturating_add(1);
                shown.dismissed = old.dismissed && !quiet;
                quiet
            }
            None => true,
        };
        self.errors.push(shown);
        if self.errors.len() > Self::KEPT {
            let over = self.errors.len() - Self::KEPT;
            self.errors.drain(..over);
        }
        fresh
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
        let errors = self.errors.iter().map(|e| match e.count {
            1 => e.details.clone(),
            n => format!("{}\n(happened {n} times)", e.details),
        });
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

    /// The error the strip shows: the newest one not yet dismissed.
    fn showing(&mut self) -> Option<&mut ShownError> {
        self.errors.iter_mut().rev().find(|e| !e.dismissed)
    }

    /// The newest error not yet dismissed, as a strip across the page.
    pub fn show_inline(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<ErrorResponse> {
        let error = self.showing()?;
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
                        ui.label(RichText::new(error.headline()).family(crate::fonts::semibold()));
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

    /// The Diagnostics window: the session's errors, newest first, with
    /// copyable details. Only the rows in sight are laid out, so it costs
    /// the same however long the list is.
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
                // One row an error: when, what (and how often), and a button
                // that copies its details, which show as the row's tooltip.
                let row_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
                let errors = &self.errors;
                egui::ScrollArea::vertical().max_height(420.0).show_rows(
                    ui,
                    row_height,
                    errors.len(),
                    |ui, rows| {
                        for error in errors.iter().rev().skip(rows.start).take(rows.len()) {
                            ui.horizontal(|ui| {
                                ui.set_height(ui.spacing().interact_size.y);
                                ui.label(
                                    RichText::new(humantime_ago(error.at))
                                        .font(crate::fonts::mono(11.0))
                                        .color(theme.muted),
                                );
                                if ui.small_button("Copy details").clicked() {
                                    ctx.copy_text(error.details.clone());
                                }
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(error.headline()).family(crate::fonts::semibold()),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(RichText::new(&error.details).font(crate::fonts::mono(11.0)));
                            });
                        }
                    },
                );
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

    fn at(err: &AppError, secs: u64) -> ShownError {
        ShownError {
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            ..ShownError::from(err)
        }
    }

    #[test]
    fn a_repeat_is_counted_on_the_error_listed_already() {
        let mut center = ErrorCenter::default();
        let save = AppError::new("Couldn't save")
            .why("The disk is full")
            .fix("Free some room");
        let other = AppError::new("Couldn't save")
            .why("The folder is gone")
            .fix("Save elsewhere");
        assert!(center.record(at(&save, 0)));
        assert!(center.record(at(&other, 1)));
        for frame in 2..1000 {
            center.record(at(&save, frame / 60 + 1));
        }
        let all = center.all();
        assert_eq!(all.len(), 2, "one line per problem");
        assert_eq!(
            all[1].why.as_deref(),
            Some("The disk is full"),
            "the repeat is the newest"
        );
        assert_eq!(all[1].count, 999);
        assert_eq!(all[1].at, SystemTime::UNIX_EPOCH + Duration::from_secs(17));
        assert_eq!(all[0].count, 1);
        assert!(center.all_details().contains("(happened 999 times)"));
    }

    #[test]
    fn the_list_keeps_the_newest_errors() {
        let mut center = ErrorCenter::default();
        for n in 0..ErrorCenter::KEPT + 50 {
            center.push(&AppError::new(format!("Problem {n}")).fix("None"));
        }
        let all = center.all();
        assert_eq!(all.len(), ErrorCenter::KEPT);
        assert_eq!(all[0].what, "Problem 50");
        assert_eq!(
            all[ErrorCenter::KEPT - 1].what,
            format!("Problem {}", ErrorCenter::KEPT + 49)
        );
    }

    fn at_ms(err: &AppError, millis: u64) -> ShownError {
        ShownError {
            at: SystemTime::UNIX_EPOCH + Duration::from_millis(millis),
            ..ShownError::from(err)
        }
    }

    /// A dismissed error happening again every frame stays dismissed, for
    /// as long as it keeps happening; after a pause it shows again.
    #[test]
    fn a_quick_repeat_does_not_show_a_dismissed_error_again() {
        let mut center = ErrorCenter::default();
        let err = AppError::new("The preview can't be drawn").fix("Restart");
        assert!(center.record(at_ms(&err, 100_000)));
        center.errors[0].dismissed = true;
        // Every frame at 60 a second, for 20 seconds.
        let frames = 20 * 60;
        for frame in 1..=frames {
            assert!(
                !center.record(at_ms(&err, 100_000 + frame * 1000 / 60)),
                "only counted"
            );
            assert!(center.all()[0].dismissed);
        }
        let last = 100_000 + frames * 1000 / 60;
        assert!(
            center.record(at_ms(&err, last + ErrorCenter::QUIET.as_millis() as u64)),
            "shown anew"
        );
        assert!(!center.all()[0].dismissed);
        assert_eq!(center.all()[0].count as u64, frames + 2);
    }

    /// The person dismisses an error, tries again a few seconds later and
    /// it fails the same way: the strip shows it again rather than nothing.
    #[test]
    fn a_dismissed_error_tried_again_later_shows_again() {
        let mut center = ErrorCenter::default();
        let other = AppError::new("Couldn't save").fix("Save elsewhere");
        let clash = AppError::new("There is already a clip there").fix("Move it elsewhere.");
        assert!(center.record(at(&other, 0)));
        assert!(center.record(at(&clash, 10)));
        center.errors[1].dismissed = true;
        assert!(center.record(at(&clash, 13)), "a retry is not silent");
        assert!(!center.all()[1].dismissed);
        assert_eq!(center.all()[1].count, 2);
        assert_eq!(
            center.showing().map(|e| e.what.clone()).as_deref(),
            Some("There is already a clip there"),
            "the strip shows the retry, not the older error"
        );
    }

    /// Only the rows in sight are laid out: the window's cost stays put.
    #[test]
    fn the_diagnostics_window_draws_a_long_list() {
        let ctx = egui::Context::default();
        let mut center = ErrorCenter::default();
        for n in 0..ErrorCenter::KEPT {
            center.push(
                &AppError::new(format!("Problem {n}"))
                    .why("x".repeat(500))
                    .fix("None"),
            );
        }
        let mut open = true;
        for _ in 0..2 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                center.diagnostics_window(ui.ctx(), &mut open, &Theme::ASH);
            });
            output.drop_without_applying_deltas();
        }
        assert!(open);
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
