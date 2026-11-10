//! The top bar, identical on every page: project name on the left, page
//! tabs centred over the viewport, then search, the resource readout and
//! settings on the right.

use crate::fonts;
use crate::layout::{Page, TOP_BAR_HEIGHT, viewport_centre_x};
use crate::theme::Theme;
use egui::{Align, Align2, CornerRadius, FontId, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

/// What the top bar shows.
#[derive(Debug, Clone)]
pub struct TopBar<'a> {
    pub project_name: &'a str,
    /// A quiet line under the name, such as "Main sequence · 1080p 60 · saved 8 s ago".
    pub subtitle: &'a str,
    pub page: Page,
    /// The resource readout, for example "CPU 12%  GPU –  RAM 1.4 GB".
    pub usage: &'a str,
}

/// What was clicked this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TopBarClicks {
    pub page: Option<Page>,
    pub search: bool,
    pub usage: bool,
    pub settings: bool,
}

const TAB_HEIGHT: f32 = 30.0;
const TAB_PADDING: f32 = 16.0;

impl TopBar<'_> {
    /// Draws the bar into `ui`, which should be a top panel of [`TOP_BAR_HEIGHT`].
    pub fn show(&self, ui: &mut Ui, theme: &Theme) -> TopBarClicks {
        let mut clicks = TopBarClicks::default();
        let full = ui.max_rect();
        let window_width = ui.ctx().content_rect().width();

        // Left: logo mark, name and subtitle.
        let left = Rect::from_min_size(full.min, vec2(full.width() / 3.0, TOP_BAR_HEIGHT));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(left.shrink2(vec2(16.0, 0.0))),
            |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (mark, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                    ui.painter().rect_filled(mark, 6.0, theme.accent);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add_space(7.0);
                        ui.label(
                            RichText::new(self.project_name)
                                .family(fonts::semibold())
                                .size(13.0),
                        );
                        ui.label(RichText::new(self.subtitle).size(11.0).color(theme.muted));
                    });
                });
            },
        );

        // Centre: tabs centred on the viewport, not the window.
        let font = FontId::proportional(12.5);
        let widths: Vec<f32> = Page::ALL
            .iter()
            .map(|p| {
                ui.painter()
                    .layout_no_wrap(p.label().to_owned(), font.clone(), theme.text)
                    .size()
                    .x
                    + 2.0 * TAB_PADDING
            })
            .collect();
        let group_width = widths.iter().sum::<f32>() + 2.0 * (Page::ALL.len() as f32 - 1.0) + 6.0;
        let centre = pos2(full.min.x + viewport_centre_x(window_width), full.center().y);
        let group = Rect::from_center_size(centre, vec2(group_width, TAB_HEIGHT + 6.0));
        ui.painter().rect_filled(group, 8.0, theme.raised);
        let mut x = group.min.x + 3.0;
        for (page, width) in Page::ALL.into_iter().zip(widths) {
            let rect = Rect::from_min_size(pos2(x, group.min.y + 3.0), vec2(width, TAB_HEIGHT));
            let response = ui.interact(rect, ui.scope_id().with(("tab", page)), Sense::click());
            let selected = page == self.page;
            if selected || response.hovered() {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(6), theme.active);
            }
            let colour = if selected {
                theme.text
            } else {
                theme.text_secondary
            };
            let family = if selected {
                fonts::semibold()
            } else {
                egui::FontFamily::Proportional
            };
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                page.label(),
                FontId::new(12.5, family),
                colour,
            );
            if response.clicked() {
                clicks.page = Some(page);
            }
            x += width + 2.0;
        }

        // Right: search, readout, settings.
        let right = Rect::from_min_max(pos2(full.max.x - full.width() / 3.0, full.min.y), full.max);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(right.shrink2(vec2(12.0, 0.0))),
            |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("⚙").size(15.0)).frame(false))
                        .on_hover_text("Settings (Ctrl+,)")
                        .clicked()
                    {
                        clicks.settings = true;
                    }
                    ui.add_space(6.0);
                    let readout = ui
                        .add(
                            egui::Button::new(
                                RichText::new(self.usage)
                                    .font(fonts::mono(11.0))
                                    .color(theme.muted),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Resource use. Click to open the resource settings.");
                    if readout.clicked() {
                        clicks.usage = true;
                    }
                    ui.add_space(6.0);
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Search   Ctrl K").color(theme.text_secondary))
                                .stroke(Stroke::new(1.0, theme.line_strong)),
                        )
                        .clicked()
                    {
                        clicks.search = true;
                    }
                });
            },
        );

        ui.painter()
            .hline(full.x_range(), full.max.y - 0.5, Stroke::new(1.0, theme.line));
        clicks
    }
}
