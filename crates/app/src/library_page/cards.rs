//! Files with a picture (video, images, and anything in a custom list) as
//! a grid of thumbnails, or as rows. Each card shows the file's picture
//! (the pointer moving across it skims through the file), a coloured edge
//! for its labels, "In timeline" and proxy badges, its length, name and
//! what it holds. A sound file in a list gets its waveform in place of a
//! picture. Click to see it in the detail panel, double-click to add it,
//! right-click for the rest.

use super::arrange::{Facts, Sizes};
use super::{About, LibraryPage, Picked, Readers, View};
use crate::proxies::{self, Status};
use project::{MediaId, MediaKind, MediaRef};
use ui_kit::Theme;
use ui_kit::timeline::LoadState;

/// The usual width of a thumbnail, and the least and most it can be.
pub const USUAL_SIZE: f32 = 176.0;
pub const SMALLEST: f32 = 120.0;
pub const LARGEST: f32 = 300.0;

/// Room between cards.
const SPACING: f32 = 12.0;
/// Room under a card's picture for its name and details.
const TEXT: f32 = 40.0;
/// How tall a row is in the rows view.
const ROW: f32 = 46.0;
/// How tall a group's heading is.
pub const HEADING: f32 = 34.0;

/// The lines' sizes for `view` at thumbnails `size` wide in `width`.
pub fn sizes(view: View, size: f32, width: f32) -> Sizes {
    match view {
        View::Grid => {
            let (columns, card) = columns(size, width);
            Sizes {
                columns,
                row: card * 9.0 / 16.0 + TEXT + SPACING,
                heading: HEADING,
            }
        }
        View::Rows => Sizes {
            columns: 1,
            row: ROW,
            heading: HEADING,
        },
    }
}

/// How many cards about `size` wide fit side by side in `width`, and how
/// wide each then is (they share out what is left).
pub fn columns(size: f32, width: f32) -> (usize, f32) {
    let columns = ((width + SPACING) / (size + SPACING)).floor().max(1.0) as usize;
    let card = (width - SPACING * (columns as f32 - 1.0)) / columns as f32;
    (columns, card.max(1.0))
}

/// One line of files: a row of cards, or one row of the rows view.
#[allow(clippy::too_many_arguments)]
pub fn line(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    readers: &mut Readers<'_>,
    rect: egui::Rect,
    files: &[MediaId],
    sizes: Sizes,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let (_, card) = columns(page.size, rect.width());
    for (i, id) in files.iter().enumerate() {
        let Some(media) = about.project.media.get(id) else {
            continue;
        };
        let asked = if page.view == View::Rows {
            let row = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), sizes.row - 4.0));
            file_row(page, ui, theme, about, facts, readers, row, media)
        } else {
            let left = rect.left() + i as f32 * (card + SPACING);
            let cell = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(card, sizes.row - SPACING),
            );
            file_card(page, ui, theme, about, facts, readers, cell, media)
        };
        picked.extend(asked);
    }
    picked
}

/// What a file shows as its picture: a texture to draw (and its shape), a
/// waveform, or how far reading it has come.
enum Shown {
    Picture(egui::TextureId, f32),
    Sound(crate::waveforms::Look),
    Reading,
    Nothing,
}

/// What to draw for `media` in a card or row `width` wide, the pointer at
/// `skim` of the way across it (a share, while it is over it).
fn shown(page: &mut LibraryPage, readers: &mut Readers<'_>, media: &MediaRef, skim: Option<f32>) -> Shown {
    if media.kind == MediaKind::Audio {
        let Some(stream) = media.info.audio.first().map(|a| a.index) else {
            return Shown::Nothing;
        };
        let look = (readers.waveform)(media, stream);
        if look.state.is_some_and(|s| s != LoadState::Failed) {
            page.waiting += 1;
        }
        return Shown::Sound(look);
    }
    let Some(stream) = super::picture_stream(media) else {
        return Shown::Nothing;
    };
    match (readers.pictures)(media, stream) {
        (Some(pictures), _) if !pictures.textures.is_empty() => {
            let count = pictures.textures.len();
            // Resting: a third of the way in, past dark openings.
            let i = skim.map_or(count / 3, |share| (share * count as f32) as usize);
            Shown::Picture(pictures.textures[i.min(count - 1)], pictures.aspect)
        }
        (_, Some(LoadState::Failed)) | (Some(_), _) => Shown::Nothing,
        (None, _) => {
            page.waiting += 1;
            Shown::Reading
        }
    }
}

/// Draws what `shown` says in `rect`, the picture fitted inside it.
fn draw_shown(
    painter: &egui::Painter,
    theme: &Theme,
    rect: egui::Rect,
    shown: &Shown,
    colour: egui::Color32,
    length: f64,
) {
    painter.rect_filled(rect, 5.0, theme.viewport);
    match shown {
        Shown::Picture(texture, aspect) => {
            let fitted = fit(rect, *aspect);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(*texture, fitted, uv, egui::Color32::WHITE);
        }
        Shown::Sound(look) => {
            if let Some(peaks) = &look.peaks {
                let inner = rect.shrink2(egui::vec2(6.0, rect.height() * 0.2));
                ui_kit::waveform::paint(
                    painter,
                    inner,
                    &peaks.values,
                    peaks.per_second as f32,
                    length,
                    colour,
                );
            }
        }
        Shown::Reading => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(16.0),
                theme.faint,
            );
        }
        Shown::Nothing => {}
    }
}

/// `aspect` (width over height) fitted inside `rect`, centred.
pub fn fit(rect: egui::Rect, aspect: f32) -> egui::Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let size = if rect.width() / rect.height().max(1.0) > aspect {
        egui::vec2(rect.height() * aspect, rect.height())
    } else {
        egui::vec2(rect.width(), rect.width() / aspect)
    };
    egui::Rect::from_center_size(rect.center(), size)
}

/// The colour a file is drawn in: its first label's, or the interface's own.
pub fn file_colour(project: &project::Project, media: MediaId, theme: &Theme) -> egui::Color32 {
    project
        .library
        .labels_of(media)
        .first()
        .map_or(theme.text_secondary, |l| {
            crate::timeline_panel::label_colour(l.color)
        })
}

/// One card of the grid.
#[allow(clippy::too_many_arguments)]
fn file_card(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    readers: &mut Readers<'_>,
    rect: egui::Rect,
    media: &MediaRef,
) -> Vec<Picked> {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("library-file", media.id.0)),
        egui::Sense::click(),
    );
    let picture = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), rect.width() * 9.0 / 16.0));
    let skim = response
        .hover_pos()
        .filter(|p| picture.contains(*p))
        .map(|p| ((p.x - picture.left()) / picture.width()).clamp(0.0, 0.999));
    let shown = shown(page, readers, media, skim);
    let project = about.project;
    let missing = about.missing.contains(&media.id);
    let painter = ui.painter_at(rect.expand(3.0));
    let colour = file_colour(project, media.id, theme);
    draw_shown(
        &painter,
        theme,
        picture,
        &shown,
        colour,
        media.duration.as_secs_f64(),
    );
    label_edge(&painter, project, media.id, picture);
    if missing {
        painter.rect_filled(picture, 5.0, theme.viewport.gamma_multiply(0.8));
        painter.text(
            picture.center(),
            egui::Align2::CENTER_CENTER,
            "Can't be found",
            egui::FontId::proportional(12.0),
            ui_kit::status::ERROR,
        );
    }
    let corner = picture.shrink(6.0);
    if facts.used(media.id) {
        badge(
            &painter,
            corner.left_top(),
            egui::Align2::LEFT_TOP,
            "In timeline",
            theme.accent,
            theme.accent_ink,
        );
    }
    if let Some((text, fill)) = proxy_badge(&(about.proxy)(media.id), theme).filter(|_| !missing) {
        badge(
            &painter,
            corner.right_top(),
            egui::Align2::RIGHT_TOP,
            &text,
            fill,
            theme.bg,
        );
    }
    if media.info.stated_length.is_some() {
        badge(
            &painter,
            corner.left_bottom(),
            egui::Align2::LEFT_BOTTOM,
            "End missing",
            ui_kit::status::WARNING,
            theme.bg,
        );
    }
    if media.kind != MediaKind::Image {
        let length = ui_kit::time_text::length(media.duration.as_secs_f64());
        badge(
            &painter,
            corner.right_bottom(),
            egui::Align2::RIGHT_BOTTOM,
            &length,
            theme.viewport.gamma_multiply(0.85),
            theme.text,
        );
    }
    let selected = page.selected == Some(media.id);
    if selected || response.hovered() {
        let stroke = if selected {
            egui::Stroke::new(2.0, theme.accent)
        } else {
            egui::Stroke::new(1.0, theme.line_strong)
        };
        painter.rect_stroke(picture.expand(1.0), 6.0, stroke, egui::StrokeKind::Outside);
    }
    let name = library::file_name(&media.path);
    text_line(
        &painter,
        egui::pos2(rect.left() + 2.0, picture.bottom() + 6.0),
        &name,
        12.5,
        true,
        rect.width() - 4.0,
        theme.text,
    );
    text_line(
        &painter,
        egui::pos2(rect.left() + 2.0, picture.bottom() + 23.0),
        &super::details(media),
        11.0,
        false,
        rect.width() - 4.0,
        theme.muted,
    );
    let response = response.on_hover_text(tip(media, about, facts));
    let mut picked: Vec<Picked> = page.clicked(media, &response).into_iter().collect();
    picked.extend(page.file_menu(&response, about, media));
    picked
}

/// One row of the rows view: a small picture, the name and details, the
/// labels, badges and length.
#[allow(clippy::too_many_arguments)]
fn file_row(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    readers: &mut Readers<'_>,
    rect: egui::Rect,
    media: &MediaRef,
) -> Vec<Picked> {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("library-file", media.id.0)),
        egui::Sense::click(),
    );
    let selected = page.selected == Some(media.id);
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 6.0, theme.active);
    } else if response.hovered() {
        painter.rect_filled(rect, 6.0, theme.line);
    }
    let picture = egui::Rect::from_min_size(
        rect.min + egui::vec2(8.0, 4.0),
        egui::vec2((rect.height() - 8.0) * 16.0 / 9.0, rect.height() - 8.0),
    );
    let skim = response
        .hover_pos()
        .filter(|p| picture.contains(*p))
        .map(|p| ((p.x - picture.left()) / picture.width()).clamp(0.0, 0.999));
    let shown = shown(page, readers, media, skim);
    let project = about.project;
    let colour = file_colour(project, media.id, theme);
    draw_shown(
        &painter,
        theme,
        picture,
        &shown,
        colour,
        media.duration.as_secs_f64(),
    );
    label_edge(&painter, project, media.id, picture);
    let x = picture.right() + 10.0;
    let right_room = 220.0;
    let width = (rect.right() - right_room - x).max(40.0);
    let missing = about.missing.contains(&media.id);
    let name = library::file_name(&media.path);
    text_line(
        &painter,
        egui::pos2(x, rect.top() + 6.0),
        &name,
        12.5,
        true,
        width,
        if missing { theme.muted } else { theme.text },
    );
    let details = if missing {
        "can't be found".to_owned()
    } else {
        super::details(media)
    };
    text_line(
        &painter,
        egui::pos2(x, rect.top() + 24.0),
        &details,
        11.0,
        false,
        width,
        theme.muted,
    );
    // From the right: length, then the badges.
    let mut right = rect.right() - 10.0;
    let mid = rect.center().y;
    if media.kind != MediaKind::Image {
        let r = painter.text(
            egui::pos2(right, mid),
            egui::Align2::RIGHT_CENTER,
            ui_kit::time_text::length(media.duration.as_secs_f64()),
            egui::FontId::monospace(11.5),
            theme.text_secondary,
        );
        right = r.left() - 10.0;
    }
    if let Some((text, fill)) = proxy_badge(&(about.proxy)(media.id), theme).filter(|_| !missing) {
        let r = badge(
            &painter,
            egui::pos2(right, mid),
            egui::Align2::RIGHT_CENTER,
            &text,
            fill,
            theme.bg,
        );
        right = r.left() - 6.0;
    }
    if facts.used(media.id) {
        badge(
            &painter,
            egui::pos2(right, mid),
            egui::Align2::RIGHT_CENTER,
            "In timeline",
            theme.accent,
            theme.accent_ink,
        );
    }
    let response = response.on_hover_text(tip(media, about, facts));
    let mut picked: Vec<Picked> = page.clicked(media, &response).into_iter().collect();
    picked.extend(page.file_menu(&response, about, media));
    picked
}

/// The edge of a file's picture in its labels' colours, shared between them.
pub fn label_edge(painter: &egui::Painter, project: &project::Project, media: MediaId, rect: egui::Rect) {
    let labels = project.library.labels_of(media);
    if labels.is_empty() {
        return;
    }
    let part = rect.height() / labels.len() as f32;
    for (i, label) in labels.iter().enumerate() {
        let top = rect.top() + i as f32 * part;
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(4.0, part)),
            1.0,
            crate::timeline_panel::label_colour(label.color),
        );
    }
}

/// The proxy badge's words and colour: needed, or being built.
pub fn proxy_badge(status: &Status, theme: &Theme) -> Option<(String, egui::Color32)> {
    match status {
        Status::Needed => Some(("Needs proxy".into(), ui_kit::status::WARNING)),
        Status::Building(_) => proxies::label(status).map(|t| (t, theme.text_secondary)),
        Status::Failed(_) => Some(("Proxy failed".into(), ui_kit::status::ERROR)),
        _ => None,
    }
}

/// A small filled pill with `text`, placed by `align` at `at`. Returns where it went.
pub fn badge(
    painter: &egui::Painter,
    at: egui::Pos2,
    align: egui::Align2,
    text: &str,
    fill: egui::Color32,
    ink: egui::Color32,
) -> egui::Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(10.5), ink);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = align.anchor_size(at, size);
    painter.rect_filled(rect, size.y / 2.0, fill);
    painter.galley(rect.min + egui::vec2(5.0, 2.0), galley, ink);
    rect
}

/// One line of text cut to `width` with an ellipsis.
pub fn text_line(
    painter: &egui::Painter,
    at: egui::Pos2,
    text: &str,
    size: f32,
    strong: bool,
    width: f32,
    colour: egui::Color32,
) {
    let font = if strong {
        egui::FontId::new(size, ui_kit::fonts::semibold())
    } else {
        egui::FontId::proportional(size)
    };
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    let galley = painter.layout_job(job);
    painter.galley(at, galley, colour);
}

/// A file's tooltip: where it is, how it is used, what its proxy is up to,
/// its note, and what the mouse does.
pub fn tip(media: &MediaRef, about: &About<'_>, facts: &Facts) -> String {
    let mut lines = vec![media.path.display().to_string()];
    match facts.clips.get(&media.id) {
        Some(clips) => lines.push(format!(
            "In {} timeline{} ({clips} clip{})",
            facts.timelines.get(&media.id).copied().unwrap_or(1),
            if facts.timelines.get(&media.id).copied().unwrap_or(1) == 1 {
                ""
            } else {
                "s"
            },
            if *clips == 1 { "" } else { "s" },
        )),
        None => lines.push("Not in a timeline".into()),
    }
    if let Some(text) = proxies::explanation(&(about.proxy)(media.id), about.proxy_mode) {
        lines.push(text);
    }
    let note = about.project.library.note(media.id);
    if !note.is_empty() {
        lines.push(format!("Note: {note}"));
    }
    lines.push("Click to see it on the right; double-click to add it to the end of the timeline; right-click for more.".into());
    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_share_out_the_width() {
        assert_eq!(columns(176.0, 1000.0).0, 5);
        let (n, card) = columns(176.0, 1000.0);
        assert!((card * n as f32 + SPACING * (n as f32 - 1.0) - 1000.0).abs() < 0.01);
        assert_eq!(
            columns(300.0, 100.0),
            (1, 100.0),
            "at least one, as wide as there is"
        );
        let grid = sizes(View::Grid, 176.0, 1000.0);
        assert!(grid.row > card * 9.0 / 16.0);
        assert_eq!(sizes(View::Rows, 176.0, 1000.0).columns, 1);
    }

    #[test]
    fn pictures_fit_inside_without_stretching() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(160.0, 90.0));
        let tall = fit(rect, 9.0 / 16.0);
        assert_eq!(tall.height(), 90.0);
        assert!((tall.width() - 50.625).abs() < 0.01);
        assert_eq!(tall.center(), rect.center());
        assert_eq!(fit(rect, 16.0 / 9.0), rect);
        assert_eq!(fit(rect, f32::NAN), rect, "an unknown shape counts as 16:9");
    }
}
