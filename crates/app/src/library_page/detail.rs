//! The Library's detail panel, as wide as the Edit page's inspector: the
//! file picked in the middle, to look at and to add.
//!
//! From the top: its name; a preview (a small player for video with the
//! viewer's transport over it and a scrub bar under it, the picture for an
//! image, a large waveform with the transport for sound); marking in and
//! out (the keys for them, or the buttons) and adding to the timeline (the
//! marked part, at the level chosen below); then sections for its sound
//! (loudness and true peak, measured on a worker, with what to do about a
//! problem in one click), what the file is, its labels, its lists and its
//! note.

use super::arrange::Facts;
use super::loudness::{Reading, advice};
use super::{About, AddAt, LibraryPage, Picked, Readers};
use project::{Command, MediaKind, MediaRef};
use ui_kit::scrub::{self, Marks};
use ui_kit::transport::{TransportClick, transport};
use ui_kit::{Action, Theme};

pub fn ui(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    readers: &mut Readers<'_>,
) -> Vec<Picked> {
    let project = about.project;
    let Some(media) = page.selected.and_then(|id| project.media.get(&id)) else {
        nothing_selected(ui, theme, about, facts);
        return Vec::new();
    };
    let mut picked = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("library-detail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(library::file_name(&media.path))
                        .family(ui_kit::fonts::semibold())
                        .size(15.0),
                )
                .truncate(),
            );
            let folder = media
                .path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.add(egui::Label::new(egui::RichText::new(folder).size(11.5).color(theme.muted)).truncate());
            ui.add_space(8.0);
            preview(page, ui, theme, about, readers, media);
            ui.add_space(6.0);
            marking(page, ui, theme, about, media);
            ui.add_space(8.0);
            picked.extend(adding(page, ui, theme, about, media));
            ui.add_space(10.0);
            picked.extend(sections(page, ui, theme, about, facts, media));
        });
    picked
}

/// With no file picked: what the library holds, and how to start.
fn nothing_selected(ui: &mut egui::Ui, theme: &Theme, about: &About<'_>, facts: &Facts) {
    ui.label(
        egui::RichText::new("Library")
            .family(ui_kit::fonts::semibold())
            .size(15.0),
    );
    let files = about.project.media.len();
    let used = facts.clips.len();
    ui.label(
        egui::RichText::new(format!(
            "{files} file{} · {used} in a timeline",
            if files == 1 { "" } else { "s" }
        ))
        .size(12.0)
        .color(theme.muted),
    );
    ui_kit::inspector::note(
        ui,
        theme,
        "Click a file to see it here, mark the part you want and add it to the timeline. Double-click a file to add it all at the end.",
    );
}

/// The preview of the file: the player's picture, the picture, or the waveform.
fn preview(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    readers: &mut Readers<'_>,
    media: &MediaRef,
) {
    let width = ui.available_width();
    let length = media.duration.as_secs_f64();
    let marks = page.marks(media.id);
    match media.kind {
        MediaKind::Image => {
            let size = egui::vec2(width, width * 9.0 / 16.0);
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, 6.0, theme.viewport);
            if let Some(stream) = super::picture_stream(media) {
                match (readers.pictures)(media, stream) {
                    (Some(pictures), _) if !pictures.textures.is_empty() => {
                        let fitted = super::cards::fit(rect, pictures.aspect);
                        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        ui.painter()
                            .image(pictures.textures[0], fitted, uv, egui::Color32::WHITE);
                    }
                    (None, Some(state)) if state != ui_kit::timeline::LoadState::Failed => page.waiting += 1,
                    _ => {}
                }
            }
            return;
        }
        MediaKind::Video if media.info.video.is_some() => {
            let (w, h) = media.info.video.as_ref().map_or((16, 9), |v| v.display_size());
            // Upright recordings get a taller frame, but not past a square.
            let height = (width * h as f32 / w.max(1) as f32).clamp(width * 9.0 / 16.0, width);
            let size = egui::vec2(width, height);
            let rect = egui::Rect::from_min_size(ui.cursor().min, size);
            page.player.picture_ui(ui, theme, size, about.quality);
            ui.add_space(4.0);
            let (bar, _) = ui.allocate_exact_size(egui::vec2(width, scrub::BAR_HEIGHT), egui::Sense::hover());
            let at = page.player.at;
            if let Some(to) = scrub::scrub(
                ui,
                theme,
                ("library-detail", media.id.0),
                bar,
                length,
                at,
                marks,
                scrub::Look::Bar,
            ) {
                page.player.go_to(to, marks);
            }
            transport_under(page, ui, theme, about, rect, marks);
        }
        _ => {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 110.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 6.0, theme.viewport);
            let wave = rect.shrink(8.0);
            let colour = super::cards::file_colour(about.project, media.id, theme);
            if let Some(stream) = media.info.audio.first().map(|a| a.index) {
                let look = (readers.waveform)(media, stream);
                if look
                    .state
                    .is_some_and(|s| s != ui_kit::timeline::LoadState::Failed)
                {
                    page.waiting += 1;
                }
                if let Some(peaks) = &look.peaks {
                    ui_kit::waveform::paint(
                        &ui.painter_at(wave),
                        wave,
                        &peaks.values,
                        peaks.per_second as f32,
                        length,
                        colour,
                    );
                }
            }
            let at = page.player.at;
            if let Some(to) = scrub::scrub(
                ui,
                theme,
                ("library-detail", media.id.0),
                wave,
                length,
                at,
                marks,
                scrub::Look::Over,
            ) {
                page.player.go_to(to, marks);
            }
            transport_under(page, ui, theme, about, rect, marks);
        }
    }
    // Where the player is, of how long.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} / {}", clock(page.player.at), clock(length)))
                .font(egui::FontId::monospace(11.5))
                .color(theme.text_secondary),
        );
    });
}

/// How much room the transport takes under the preview.
const TRANSPORT_ROOM: f32 = 60.0;

/// The shared transport, under the preview in `above` (the panel's preview
/// is too small to float it over the picture as the viewer does); its
/// buttons work the player.
fn transport_under(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    above: egui::Rect,
    marks: Marks,
) {
    let (strip, _) = ui.allocate_exact_size(egui::vec2(above.width(), TRANSPORT_ROOM), egui::Sense::hover());
    // The transport sits at the bottom of the rectangle it is given.
    let rect = egui::Rect::from_min_max(above.min, strip.max);
    match transport(ui, theme, rect, page.player.playing(), about.shortcuts) {
        Some(TransportClick::PlayPause) => page.player.play_pause(marks),
        Some(TransportClick::Stop) => page.player.stop_here(),
        Some(TransportClick::FrameBack) => page.player.step(-1, marks),
        Some(TransportClick::FrameForward) => page.player.step(1, marks),
        None => {}
    }
}

/// Mark in, mark out and clear, with what is marked.
fn marking(page: &mut LibraryPage, ui: &mut egui::Ui, theme: &Theme, about: &About<'_>, media: &MediaRef) {
    if media.kind == MediaKind::Image {
        return;
    }
    let length = media.duration.as_secs_f64();
    ui.horizontal(|ui| {
        if ui
            .button("Mark in")
            .on_hover_text(about.shortcuts.tip(Action::MarkIn))
            .clicked()
        {
            page.command(Action::MarkIn);
        }
        if ui
            .button("Mark out")
            .on_hover_text(about.shortcuts.tip(Action::MarkOut))
            .clicked()
        {
            page.command(Action::MarkOut);
        }
        if page.marks(media.id).is_set()
            && ui
                .button("Clear")
                .on_hover_text("Add the whole file again")
                .clicked()
        {
            page.uses.change(media.id, length, |u| u.marks = Marks::default());
        }
    });
    let marks = page.marks(media.id);
    let text = if marks.is_set() {
        let (start, end) = marks.part(length);
        format!(
            "Marked {} to {} · {}{}",
            clock(start),
            clock(end),
            ui_kit::time_text::length(end - start),
            if page.uses.saved() {
                " · saved in the project"
            } else {
                ""
            }
        )
    } else {
        "The whole file is added. Mark in and out to add a part of it.".to_owned()
    };
    ui.label(egui::RichText::new(text).size(11.5).color(theme.muted));
}

/// "Add to timeline" (at the end) and "Insert at edit point".
fn adding(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    media: &MediaRef,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let width = ui.available_width();
    let marked = page.marks(media.id).is_set();
    let missing = about.missing.contains(&media.id);
    let (text, tip) = if marked {
        (
            "Add the marked part to the timeline",
            "At the end of the open timeline. Undo takes it out again.",
        )
    } else {
        (
            "Add to timeline",
            "The whole file at the end of the open timeline (double-clicking it does the same). Undo takes it out again.",
        )
    };
    let add = ui
        .add_enabled(
            !missing,
            egui::Button::new(egui::RichText::new(text).color(theme.accent_ink))
                .fill(theme.accent)
                .min_size(egui::vec2(width, 30.0)),
        )
        .on_hover_text(tip)
        .on_disabled_hover_text("The file can't be found. Find it from the Edit page's banner first.");
    if add.clicked() {
        picked.push(Picked::Add(page.adding(media, AddAt::End)));
    }
    if ui
        .add_enabled(
            !missing,
            egui::Button::new("Insert at the edit point").min_size(egui::vec2(width, 26.0)),
        )
        .on_hover_text("Pushes everything after the edit point later to make room")
        .clicked()
    {
        picked.push(Picked::Add(page.adding(media, AddAt::EditPoint)));
    }
    let changed: Vec<String> = page
        .uses
        .get(media.id)
        .levels
        .iter()
        .filter(|(_, l)| **l != 0)
        .map(|(stream, l)| {
            format!(
                "{} {:+.1} dB",
                stream_name(media, *stream),
                f64::from(*l) / 1000.0
            )
        })
        .collect();
    if !changed.is_empty() {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Sound goes in: {}", changed.join(", ")))
                        .size(11.5)
                        .color(theme.text_secondary),
                )
                .truncate(),
            );
            if ui
                .small_button("×")
                .on_hover_text("Add every stream as it is")
                .clicked()
            {
                let length = media.duration.as_secs_f64();
                page.uses.change(media.id, length, |u| u.levels.clear());
            }
        });
    }
    picked
}

/// Sound, File, Labels, Lists and Note.
fn sections(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
    media: &MediaRef,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let project = about.project;
    if !media.info.audio.is_empty() {
        let readings: Vec<(u32, Option<Result<Reading, String>>)> = media
            .info
            .audio
            .iter()
            .map(|a| (a.index, page.loudness.get(about.jobs, ui.ctx(), media, a.index)))
            .collect();
        if page.loudness.measuring(media.id) {
            page.waiting += 1;
        }
        let summary = match readings.as_slice() {
            [(_, Some(Ok(r)))] => format!("{} · {}", lufs(r.integrated), dbtp(r.true_peak)),
            [(_, Some(Err(_)))] => "Couldn't be measured".into(),
            _ if readings.iter().any(|(_, r)| r.is_none()) => "Measuring…".into(),
            several => format!("{} streams", several.len()),
        };
        ui_kit::inspector::section(ui, theme, "Sound", &summary, false, |ui| {
            let several = readings.len() > 1;
            for (i, (stream, reading)) in readings.into_iter().enumerate() {
                if several {
                    if i > 0 {
                        ui.add_space(6.0);
                    }
                    ui.label(
                        egui::RichText::new(stream_name(media, stream))
                            .family(ui_kit::fonts::semibold())
                            .size(12.5)
                            .color(theme.text),
                    );
                }
                sound(page, ui, theme, about, media, stream, reading);
            }
        });
    }
    ui_kit::inspector::section(ui, theme, "File", &super::details(media), false, |ui| {
        for (label, value) in file_rows(media, facts) {
            ui.horizontal(|ui| {
                ui_kit::inspector::label_cell(ui, theme, label, 74.0);
                ui.add(egui::Label::new(egui::RichText::new(value).color(theme.text)).truncate());
            });
        }
    });
    let labels = project.library.labels_of(media.id);
    let summary = if labels.is_empty() {
        "None".to_owned()
    } else {
        labels
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    ui_kit::inspector::section(ui, theme, "Labels", &summary, false, |ui| {
        ui.horizontal_wrapped(|ui| {
            for label in &labels {
                let colour = crate::timeline_panel::label_colour(label.color);
                if ui_kit::tab_strip::chip(ui, theme, &label.name, Some(colour)) {
                    picked.push(Picked::Edit(Box::new(Command::SetMediaLabel {
                        media: vec![media.id],
                        label: label.id,
                        on: false,
                    })));
                }
            }
            ui.menu_button("Labels…", |ui| {
                if let Some(asked) = page.organiser.labels_menu(ui, project, about.global, media.id) {
                    picked.push(asked.into());
                }
            });
        });
    });
    let lists = project.library.lists_of(media.id);
    let summary = if lists.is_empty() {
        "In none".to_owned()
    } else {
        lists
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    ui_kit::inspector::section(ui, theme, "Lists", &summary, false, |ui| {
        if let Some(asked) = page.organiser.lists_menu(ui, project, media.id) {
            picked.push(asked.into());
        }
    });
    let note = project.library.note(media.id);
    let summary = if note.is_empty() { "None" } else { note };
    ui_kit::inspector::section(ui, theme, "Note", summary, false, |ui| {
        if page.note.as_ref().is_none_or(|(id, _)| *id != media.id) {
            page.note = Some((media.id, note.to_owned()));
        }
        if let Some((_, draft)) = &mut page.note {
            let field = ui.add(
                egui::TextEdit::multiline(draft)
                    .hint_text("What is in it, what to use it for")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            // Kept when the field is left.
            if field.lost_focus() && draft.trim() != note {
                picked.push(Picked::Edit(Box::new(Command::SetMediaNote {
                    media: media.id,
                    note: draft.clone(),
                })));
            }
        }
    });
    picked
}

/// One sound stream in the Sound section: its loudness and true peak,
/// and what to do about them (a level for that stream when it is added).
fn sound(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    media: &MediaRef,
    stream: u32,
    reading: Option<Result<Reading, String>>,
) {
    let reading = match reading {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Measuring its loudness…").color(theme.text_secondary));
            });
            return;
        }
        Some(Err(why)) => {
            ui.label(
                egui::RichText::new(format!("Its loudness couldn't be measured: {why}")).color(theme.muted),
            );
            return;
        }
        Some(Ok(reading)) => reading,
    };
    let peak_colour = if reading.true_peak > super::loudness::PEAK_CEILING {
        ui_kit::status::WARNING
    } else {
        theme.text
    };
    for (label, value, colour) in [
        ("Loudness", lufs(reading.integrated), theme.text),
        ("True peak", dbtp(reading.true_peak), peak_colour),
    ] {
        ui.horizontal(|ui| {
            ui_kit::inspector::label_cell(ui, theme, label, 74.0);
            ui.label(
                egui::RichText::new(value)
                    .font(egui::FontId::monospace(12.5))
                    .color(colour),
            );
        });
    }
    let advice = advice(reading, about.target);
    match &advice.problem {
        Some(problem) => {
            ui.label(
                egui::RichText::new(problem)
                    .size(12.0)
                    .color(theme.text_secondary),
            );
        }
        None => {
            let fine = match about.target {
                Some(t) => format!("About right for {t:.0} LUFS, with room under the peaks."),
                None => "Room under the peaks. No loudness is aimed for (Settings › Audio).".to_owned(),
            };
            ui.label(egui::RichText::new(fine).size(12.0).color(theme.muted));
        }
    }
    if let Some((mdb, words)) = advice.fix {
        let chosen = page.uses.get(media.id).levels.get(&stream) == Some(&mdb);
        let button = ui
            .add_enabled(!chosen, egui::Button::new(words))
            .on_hover_text("This stream's clips go into the timeline with their volume changed this much (the file stays as it is). Change it later in the inspector's Audio section.")
            .on_disabled_hover_text("Chosen: it goes in at this level");
        if button.clicked() {
            let length = media.duration.as_secs_f64();
            page.uses.change(media.id, length, |u| {
                u.levels.insert(stream, mdb);
            });
        }
    }
}

/// The File section's rows.
fn file_rows(media: &MediaRef, facts: &Facts) -> Vec<(&'static str, String)> {
    let mut rows = vec![(
        "Type",
        match media.kind {
            MediaKind::Video => "Video",
            MediaKind::Audio => "Sound",
            MediaKind::Image => "Picture",
        }
        .to_owned(),
    )];
    if media.kind != MediaKind::Image {
        rows.push(("Length", ui_kit::time_text::length(media.duration.as_secs_f64())));
    }
    if let Some(stated) = media.info.stated_length {
        rows.push((
            "End missing",
            format!("says {}", ui_kit::time_text::length(stated.as_secs_f64())),
        ));
    }
    if let Some(v) = &media.info.video {
        let (w, h) = v.display_size();
        rows.push(("Picture", format!("{w}×{h} · {} · {}", v.frame_rate, v.codec)));
    }
    for (i, a) in media.info.audio.iter().enumerate() {
        let label = if i == 0 { "Sound" } else { "" };
        rows.push((
            label,
            format!(
                "{} · {} · {:.1} kHz · {} ch",
                a.name,
                a.codec,
                f64::from(a.sample_rate) / 1000.0,
                a.channels
            ),
        ));
    }
    rows.push(("Size", crate::cache_cleanup::size_text(media.fingerprint.size)));
    rows.push((
        "Used",
        match (facts.timelines.get(&media.id), facts.clips.get(&media.id)) {
            (Some(timelines), Some(clips)) => format!(
                "{clips} clip{} on {timelines} timeline{}",
                if *clips == 1 { "" } else { "s" },
                if *timelines == 1 { "" } else { "s" }
            ),
            _ => "Not in a timeline".to_owned(),
        },
    ));
    rows
}

/// A sound stream's name as the import named it ("Game", "Mic", or "Audio 2").
fn stream_name(media: &MediaRef, stream: u32) -> String {
    media
        .info
        .audio
        .iter()
        .find(|a| a.index == stream)
        .map_or_else(|| format!("Stream {stream}"), |a| a.name.clone())
}

/// Seconds as minutes, seconds and tenths: "1:04.2".
pub fn clock(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).round() as u64;
    format!("{}:{:02}.{}", tenths / 600, (tenths / 10) % 60, tenths % 10)
}

fn lufs(value: f32) -> String {
    if value.is_finite() {
        format!("{value:.1} LUFS")
    } else {
        "silent".to_owned()
    }
}

fn dbtp(value: f32) -> String {
    if value.is_finite() {
        format!("{value:.1} dBTP")
    } else {
        "silent".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_a_clock() {
        assert_eq!(clock(0.0), "0:00.0");
        assert_eq!(clock(64.24), "1:04.2");
        assert_eq!(clock(-3.0), "0:00.0");
        assert_eq!(clock(3_599.96), "60:00.0");
    }

    #[test]
    fn levels_read_in_their_units() {
        assert_eq!(lufs(-14.04), "-14.0 LUFS");
        assert_eq!(dbtp(f32::NEG_INFINITY), "silent");
    }
}
