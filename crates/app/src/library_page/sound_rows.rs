//! The Audio tab: one row per sound file, not thumbnails. Each row has a
//! play button (the detail panel's player plays it), the name and format,
//! the waveform across the row in the colour of the file's first label,
//! whether a timeline uses it, and its length. The file in the panel shows
//! where the player is and its marked part on its waveform, and a click or
//! drag there moves the player.

use super::arrange::{Facts, Sizes};
use super::cards::{self, HEADING};
use super::{About, LibraryPage, Picked, Readers};
use project::{MediaId, MediaRef};
use ui_kit::Theme;
use ui_kit::timeline::LoadState;

/// The lines' sizes for rows as tall as thumbnails `size` wide make them.
pub fn sizes(size: f32) -> Sizes {
    Sizes {
        columns: 1,
        row: row_height(size) + 6.0,
        heading: HEADING,
    }
}

/// How tall a row is: the thumbnail size slider makes waveforms taller too.
pub fn row_height(size: f32) -> f32 {
    (size * 0.3).clamp(40.0, 90.0)
}

/// What a sound file holds, in a few words: "MP3 · 44.1 kHz · stereo".
pub fn format(media: &MediaRef) -> String {
    let kind = media
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    let mut parts = vec![kind];
    if let Some(sound) = media.info.audio.first() {
        let rate = f64::from(sound.sample_rate) / 1000.0;
        parts.push(if rate.fract() == 0.0 {
            format!("{rate:.0} kHz")
        } else {
            format!("{rate:.1} kHz")
        });
        parts.push(match sound.channels {
            1 => "mono".into(),
            2 => "stereo".into(),
            n => format!("{n} channels"),
        });
        if media.info.audio.len() > 1 {
            parts.push(format!("{} streams", media.info.audio.len()));
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// One sound file's row in `rect`.
#[allow(clippy::too_many_arguments)]
pub fn row(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    readers: &mut Readers<'_>,
    rect: egui::Rect,
    id: MediaId,
) -> Vec<Picked> {
    let Some(media) = about.project.media.get(&id) else {
        return Vec::new();
    };
    let rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), rect.height() - 6.0));
    let response = ui.interact(
        rect,
        ui.scope_id().with(("library-file", id.0)),
        egui::Sense::click(),
    );
    let selected = page.selected == Some(id);
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        6.0,
        if selected {
            theme.active
        } else if response.hovered() {
            theme.line
        } else {
            theme.panel
        },
    );
    cards::label_edge(&painter, about.project, id, rect.shrink2(egui::vec2(0.0, 4.0)));
    let mut picked = Vec::new();
    // The play button.
    let button = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        egui::vec2(28.0, 28.0),
    );
    let play = ui
        .interact(
            button,
            ui.scope_id().with(("library-play", id.0)),
            egui::Sense::click(),
        )
        .on_hover_text(about.shortcuts.tip(ui_kit::Action::PlayPause));
    let playing = page.player.media() == Some(id) && page.player.playing();
    painter.circle_filled(
        button.center(),
        14.0,
        if play.hovered() {
            theme.accent
        } else {
            theme.raised
        },
    );
    let ink = if play.hovered() {
        theme.accent_ink
    } else {
        theme.text
    };
    if playing {
        for dx in [-3.5, 3.5] {
            painter.rect_filled(
                egui::Rect::from_center_size(button.center() + egui::vec2(dx, 0.0), egui::vec2(3.0, 11.0)),
                1.0,
                ink,
            );
        }
    } else {
        let c = button.center() + egui::vec2(1.5, 0.0);
        painter.add(egui::Shape::convex_polygon(
            vec![
                c + egui::vec2(-4.5, -6.0),
                c + egui::vec2(6.5, 0.0),
                c + egui::vec2(-4.5, 6.0),
            ],
            ink,
            egui::Stroke::NONE,
        ));
    }
    if play.clicked() {
        page.selected = Some(id);
        let marks = page.marks(id);
        page.player.show(Some(media), marks);
        page.player.play_pause(marks);
    }
    // Name and format.
    let x = button.right() + 12.0;
    let name_width = 190.0f32.min(rect.width() * 0.3);
    cards::text_line(
        &painter,
        egui::pos2(x, rect.center().y - 15.0),
        &library::file_name(&media.path),
        12.5,
        true,
        name_width,
        theme.text,
    );
    let missing = about.missing.contains(&id);
    let line = if missing {
        "can't be found".to_owned()
    } else {
        format(media)
    };
    cards::text_line(
        &painter,
        egui::pos2(x, rect.center().y + 2.0),
        &line,
        11.0,
        false,
        name_width,
        if missing {
            ui_kit::status::ERROR
        } else {
            theme.muted
        },
    );
    // From the right: length, then whether a timeline uses it.
    let length_text = ui_kit::time_text::length(media.duration.as_secs_f64());
    let length = painter.text(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        length_text,
        egui::FontId::monospace(11.5),
        theme.text_secondary,
    );
    let used_room = 78.0;
    if facts.used(id) {
        cards::badge(
            &painter,
            egui::pos2(length.left() - 12.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "In timeline",
            theme.accent,
            theme.accent_ink,
        );
    }
    // The waveform, across what is left.
    let wave = egui::Rect::from_min_max(
        egui::pos2(x + name_width + 12.0, rect.top() + 6.0),
        egui::pos2(length.left() - 12.0 - used_room, rect.bottom() - 6.0),
    );
    if wave.width() > 8.0 {
        draw_waveform(page, ui, theme, about, readers, media, wave, selected);
    }
    let response = response.on_hover_text(cards::tip(media, about, facts));
    picked.extend(page.clicked(media, &response));
    picked.extend(page.file_menu(&response, about, media));
    picked
}

/// The row's waveform; for the file in the panel, with the player's place
/// and the marked part, and a click or drag on it moving the player.
#[allow(clippy::too_many_arguments)]
fn draw_waveform(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    readers: &mut Readers<'_>,
    media: &MediaRef,
    rect: egui::Rect,
    selected: bool,
) {
    let painter = ui.painter_at(rect);
    let length = media.duration.as_secs_f64();
    let colour = cards::file_colour(about.project, media.id, theme);
    if let Some(stream) = media.info.audio.first().map(|a| a.index) {
        let look = (readers.waveform)(media, stream);
        if look.state.is_some_and(|s| s != LoadState::Failed) {
            page.waiting += 1;
        }
        match &look.peaks {
            Some(peaks) => {
                ui_kit::waveform::paint(
                    &painter,
                    rect,
                    &peaks.values,
                    peaks.per_second as f32,
                    length,
                    colour,
                );
            }
            None if look.state == Some(LoadState::Failed) => {
                painter.text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    "no waveform",
                    egui::FontId::proportional(11.0),
                    theme.muted,
                );
            }
            None => {}
        }
    }
    if selected && page.player.media() == Some(media.id) {
        let marks = page.marks(media.id);
        let at = page.player.at;
        let look = ui_kit::scrub::Look::Over;
        if let Some(to) = ui_kit::scrub::scrub(
            ui,
            theme,
            ("library-row", media.id.0),
            rect,
            length,
            at,
            marks,
            look,
        ) {
            page.player.go_to(to, marks);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{AudioStream, Fingerprint, MediaInfo, MediaKind, Time};

    #[test]
    fn the_format_in_a_few_words() {
        let mut media = MediaRef {
            id: MediaId(1),
            path: "/music/song.mp3".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: Time::seconds(3),
            info: MediaInfo::default(),
        };
        assert_eq!(format(&media), "MP3");
        media.info.audio.push(AudioStream {
            index: 0,
            codec: "mp3".into(),
            channels: 2,
            sample_rate: 44_100,
            name: "Audio 1".into(),
        });
        assert_eq!(format(&media), "MP3 · 44.1 kHz · stereo");
        media.info.audio[0].sample_rate = 48_000;
        media.info.audio[0].channels = 1;
        assert_eq!(format(&media), "MP3 · 48 kHz · mono");
        assert_eq!(row_height(10.0), 40.0);
        assert_eq!(row_height(1_000.0), 90.0);
    }
}
